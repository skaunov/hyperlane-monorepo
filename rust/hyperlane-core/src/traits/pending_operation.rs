use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    fmt::{Debug, Display},
    io::Write,
    time::{Duration, Instant},
};

use crate::{
    ChainResult, Decode, Encode, FixedPointNumber, HyperlaneDomain, HyperlaneMessage,
    HyperlaneProtocolError, HyperlaneRocksDB, TryBatchAs, TxOutcome, H256, U256,
};
use async_trait::async_trait;
use num::CheckedDiv;
use strum::Display;
use tracing::warn;

/// Boxed operation that can be stored in an operation queue
pub type QueueOperation = Box<dyn PendingOperation>;

/// A pending operation that will be run by the submitter and cause a
/// transaction to be sent.
///
/// There are three stages to the lifecycle of a pending operation:
///
/// 1) Prepare: This is called before every submission and will usually have a
/// very short gap between it and the submit call. It can be used to confirm it
/// is ready to be submitted and it can also prepare any data that will be
/// needed for the submission. This way, the preparation can be done while
/// another transaction is being submitted.
///
/// 2) Submit: This is called to submit the operation to the destination
/// blockchain and report if it was successful or not. This is usually the act
/// of submitting a transaction. Ideally this step only sends the transaction
/// and waits for it to be included.
///
/// 3) Confirm: This is called after the operation has been submitted and is
/// responsible for checking if the operation has reached a point at which we
/// consider it safe from reorgs.
#[async_trait]
pub trait PendingOperation: Send + Sync + Debug + TryBatchAs<HyperlaneMessage> {
    /// Get the unique identifier for this operation.
    fn id(&self) -> H256;

    /// A lower value means a higher priority, such as the message nonce
    /// As new types of PendingOperations are added, an idea is to just use the
    /// current length of the queue as this item's priority.
    /// Overall this method isn't critical, since it's only used to compare
    /// operations when neither of them have a `next_attempt_after`
    fn priority(&self) -> u32;

    /// The domain this originates from.
    fn origin_domain_id(&self) -> u32;

    /// Get the database for the origin domain of this operation.
    fn origin_db(&self) -> &HyperlaneRocksDB;

    /// The domain this operation will take place on.
    fn destination_domain(&self) -> &HyperlaneDomain;

    /// Label to use for metrics granularity.
    fn app_context(&self) -> Option<String>;

    /// The status of the operation, which should explain why it is in the
    /// queue.
    fn status(&self) -> PendingOperationStatus;

    /// Set the status of the operation.
    fn set_status(&mut self, status: PendingOperationStatus);

    /// Get tuple of labels for metrics.
    fn get_operation_labels(&self) -> (String, String) {
        let app_context = self.app_context().unwrap_or("Unknown".to_string());
        let destination = self.destination_domain().to_string();
        (destination, app_context)
    }

    /// Prepare to submit this operation. This will be called before every
    /// submission and will usually have a very short gap between it and the
    /// submit call.
    async fn prepare(&mut self) -> PendingOperationResult;

    /// Submit this operation to the blockchain
    async fn submit(&mut self);

    /// Set the outcome of the `submit` call
    fn set_submission_outcome(&mut self, outcome: TxOutcome);

    /// Get the estimated the cost of the `submit` call
    fn get_tx_cost_estimate(&self) -> Option<U256>;

    /// This will be called after the operation has been submitted and is
    /// responsible for checking if the operation has reached a point at
    /// which we consider it safe from reorgs.
    async fn confirm(&mut self) -> PendingOperationResult;

    /// Record the outcome of the operation
    fn set_operation_outcome(
        &mut self,
        submission_outcome: TxOutcome,
        submission_estimated_cost: U256,
    );

    /// Get the earliest instant at which this should next be attempted.
    ///
    /// This is only used for sorting, the functions are responsible for
    /// returning `NotReady` if it is too early and matters.
    fn next_attempt_after(&self) -> Option<Instant>;

    /// Set the next time this operation should be attempted.
    fn set_next_attempt_after(&mut self, delay: Duration);

    /// Reset the number of attempts this operation has made, causing it to be
    /// retried immediately.
    fn reset_attempts(&mut self);

    /// Set the number of times this operation has been retried.
    #[cfg(any(test, feature = "test-utils"))]
    fn set_retries(&mut self, retries: u32);
}

#[derive(Debug, Display, Clone, Serialize, Deserialize, PartialEq)]
/// Status of a pending operation
pub enum PendingOperationStatus {
    /// The operation is ready to be prepared for the first time, or has just been loaded from storage
    FirstPrepareAttempt,
    /// The operation is ready to be prepared again, with the given reason
    #[strum(to_string = "Retry({0})")]
    Retry(ReprepareReason),
    /// The operation is ready to be submitted
    ReadyToSubmit,
    /// The operation has been submitted and is awaiting confirmation
    #[strum(to_string = "Confirm({0})")]
    Confirm(ConfirmReason),
}

impl Encode for PendingOperationStatus {
    fn write_to<W>(&self, writer: &mut W) -> std::io::Result<usize>
    where
        W: Write,
    {
        let serialized = serde_json::to_vec(self)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::Other, "Failed to serialize"))?;
        writer.write(&serialized)
    }
}

impl Decode for PendingOperationStatus {
    fn read_from<R>(reader: &mut R) -> Result<Self, HyperlaneProtocolError>
    where
        R: std::io::Read,
        Self: Sized,
    {
        serde_json::from_reader(reader).map_err(|_| {
            HyperlaneProtocolError::IoError(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Failed to deserialize",
            ))
        })
    }
}

#[derive(Display, Debug, Clone, Serialize, Deserialize, PartialEq)]
/// Reasons for repreparing an operation
pub enum ReprepareReason {
    #[strum(to_string = "Error checking message delivery status")]
    /// Error checking message delivery status
    ErrorCheckingDeliveryStatus,
    #[strum(to_string = "Error checking if message recipient is a contract")]
    /// Error checking if message recipient is a contract
    ErrorCheckingIfRecipientIsContract,
    #[strum(to_string = "Error fetching ISM address")]
    /// Error fetching ISM address
    ErrorFetchingIsmAddress,
    #[strum(to_string = "Error getting message metadata builder")]
    /// Error getting message metadata builder
    ErrorGettingMetadataBuilder,
    #[strum(to_string = "Error building metadata")]
    /// Error building metadata
    ErrorBuildingMetadata,
    #[strum(to_string = "Could not fetch metadata")]
    /// Could not fetch metadata
    CouldNotFetchMetadata,
    #[strum(to_string = "Error estimating costs for process call")]
    /// Error estimating costs for process call
    ErrorEstimatingGas,
    #[strum(to_string = "Error checking if message meets gas payment requirement")]
    /// Error checking if message meets gas payment requirement
    ErrorCheckingGasRequirement,
    #[strum(to_string = "Gas payment requirement not met")]
    /// Gas payment requirement not met
    GasPaymentRequirementNotMet,
    #[strum(to_string = "Message delivery estimated gas exceeds max gas limit")]
    /// Message delivery estimated gas exceeds max gas limit
    ExceedsMaxGasLimit,
    #[strum(to_string = "Delivery transaction reverted or reorged")]
    /// Delivery transaction reverted or reorged
    RevertedOrReorged,
}

#[derive(Display, Debug, Clone, Serialize, Deserialize, PartialEq)]
/// Reasons for repreparing an operation
pub enum ConfirmReason {
    #[strum(to_string = "Submitted by this relayer")]
    /// Error checking message delivery status
    SubmittedBySelf,
    #[strum(to_string = "Already submitted, awaiting confirmation")]
    /// Error checking message delivery status
    AlreadySubmitted,
    /// Error checking message delivery status
    ErrorConfirmingDelivery,
    /// Error storing delivery outcome
    ErrorRecordingProcessSuccess,
}

/// Utility fn to calculate the total estimated cost of an operation batch
pub fn total_estimated_cost(ops: &[Box<dyn PendingOperation>]) -> U256 {
    ops.iter()
        .fold(U256::zero(), |acc, op| match op.get_tx_cost_estimate() {
            Some(cost_estimate) => acc.saturating_add(cost_estimate),
            None => {
                warn!(operation=?op, "No cost estimate available for operation, defaulting to 0");
                acc
            }
        })
}

/// Calculate the gas used by an operation (either in a batch or single-submission), by looking at the total cost of the tx,
/// and the estimated cost of the operation compared to the sum of the estimates of all operations in the batch.
/// When using this for single-submission rather than a batch,
/// the `tx_estimated_cost` should be the same as the `tx_estimated_cost`
pub fn gas_used_by_operation(
    tx_outcome: &TxOutcome,
    tx_estimated_cost: U256,
    operation_estimated_cost: U256,
) -> ChainResult<U256> {
    let gas_used_by_tx = FixedPointNumber::try_from(tx_outcome.gas_used)?;
    let operation_gas_estimate = FixedPointNumber::try_from(operation_estimated_cost)?;
    let tx_gas_estimate = FixedPointNumber::try_from(tx_estimated_cost)?;
    let gas_used_by_operation = (gas_used_by_tx * operation_gas_estimate)
        .checked_div(&tx_gas_estimate)
        .ok_or(eyre::eyre!("Division by zero"))?;
    gas_used_by_operation.try_into()
}

impl Display for QueueOperation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "QueueOperation(id: {}, origin: {}, destination: {}, priority: {})",
            self.id(),
            self.origin_domain_id(),
            self.destination_domain(),
            self.priority()
        )
    }
}

impl PartialOrd for QueueOperation {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for QueueOperation {
    fn eq(&self, other: &Self) -> bool {
        self.id().eq(&other.id())
    }
}

impl Eq for QueueOperation {}

impl Ord for QueueOperation {
    fn cmp(&self, other: &Self) -> Ordering {
        use Ordering::*;
        match (self.next_attempt_after(), other.next_attempt_after()) {
            (Some(a), Some(b)) => a.cmp(&b),
            // No time means it should come before
            (None, Some(_)) => Less,
            (Some(_), None) => Greater,
            (None, None) => {
                if self.origin_domain_id() == other.origin_domain_id() {
                    // Should execute in order of nonce for the same origin
                    self.priority().cmp(&other.priority())
                } else {
                    // There is no priority between these messages, so arbitrarily use the id
                    self.id().cmp(&other.id())
                }
            }
        }
    }
}

/// Possible outcomes of performing an action on a pending operation (such as `prepare`, `submit` or `confirm`).
#[derive(Debug)]
pub enum PendingOperationResult {
    /// Promote to the next step
    Success,
    /// This operation is not ready to be attempted again yet
    NotReady,
    /// Operation needs to be started from scratch again
    Reprepare(ReprepareReason),
    /// Do not attempt to run the operation again, forget about it
    Drop,
    /// Send this message straight to the confirm queue
    Confirm(ConfirmReason),
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_encoding_pending_operation_status() {
        let status = PendingOperationStatus::Retry(ReprepareReason::CouldNotFetchMetadata);
        let encoded = status.to_vec();
        let decoded = PendingOperationStatus::read_from(&mut &encoded[..]).unwrap();
        assert_eq!(status, decoded);
    }
}
