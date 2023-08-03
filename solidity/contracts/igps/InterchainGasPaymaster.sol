// SPDX-License-Identifier: MIT OR Apache-2.0
pragma solidity >=0.8.0;

/*@@@@@@@       @@@@@@@@@
 @@@@@@@@@       @@@@@@@@@
  @@@@@@@@@       @@@@@@@@@
   @@@@@@@@@       @@@@@@@@@
    @@@@@@@@@@@@@@@@@@@@@@@@@
     @@@@@  HYPERLANE  @@@@@@@
    @@@@@@@@@@@@@@@@@@@@@@@@@
   @@@@@@@@@       @@@@@@@@@
  @@@@@@@@@       @@@@@@@@@
 @@@@@@@@@       @@@@@@@@@
@@@@@@@@@       @@@@@@@@*/

// ============ Internal Imports ============
import {Message} from "../libs/Message.sol";
import {IGPHookMetadata} from "../libs/hooks/IGPHookMetadata.sol";
import {IGasOracle} from "../interfaces/IGasOracle.sol";
import {IInterchainGasPaymaster} from "../interfaces/IInterchainGasPaymaster.sol";
import {AbstractHook} from "../hooks/AbstractHook.sol";

// ============ External Imports ============
import {Address} from "@openzeppelin/contracts/utils/Address.sol";
import {OwnableUpgradeable} from "@openzeppelin/contracts-upgradeable/access/OwnableUpgradeable.sol";

/**
 * @title InterchainGasPaymaster
 * @notice Manages payments on a source chain to cover gas costs of relaying
 * messages to destination chains.
 */
contract InterchainGasPaymaster is
    IInterchainGasPaymaster,
    IGasOracle,
    AbstractHook,
    OwnableUpgradeable
{
    using Address for address;
    using IGPHookMetadata for bytes;
    using Message for bytes;
    // ============ Constants ============

    /// @notice The scale of gas oracle token exchange rates.
    uint256 internal constant DECIMALS = 1e10;
    /// @notice default for user call if metadata not provided
    uint256 internal constant DEFAULT_GAS_USAGE = 69_420;

    // ============ Public Storage ============

    /// @notice Keyed by remote domain, the gas oracle to use for the domain.
    mapping(uint32 => IGasOracle) public gasOracles;

    /// @notice The benficiary that can receive native tokens paid into this contract.
    address public beneficiary;

    // ============ Events ============

    /**
     * @notice Emitted when the gas oracle for a remote domain is set.
     * @param remoteDomain The remote domain.
     * @param gasOracle The gas oracle.
     */
    event GasOracleSet(uint32 indexed remoteDomain, address gasOracle);

    /**
     * @notice Emitted when the beneficiary is set.
     * @param beneficiary The new beneficiary.
     */
    event BeneficiarySet(address beneficiary);

    struct GasOracleConfig {
        uint32 remoteDomain;
        address gasOracle;
    }

    // ============ Constructor ============

    constructor(address _mailbox) AbstractHook(_mailbox) {}

    // ============ External Functions ============

    /**
     * @param _owner The owner of the contract.
     * @param _beneficiary The beneficiary.
     */
    function initialize(address _owner, address _beneficiary)
        public
        initializer
    {
        __Ownable_init();
        _transferOwnership(_owner);
        _setBeneficiary(_beneficiary);
    }

    /**
     * @notice Deposits msg.value as a payment for the relaying of a message
     * to its destination chain.
     * @dev Overpayment will result in a refund of native tokens to the _refundAddress.
     * Callers should be aware that this may present reentrancy issues.
     * @param _messageId The ID of the message to pay for.
     * @param _destinationDomain The domain of the message's destination chain.
     * @param _gasAmount The amount of destination gas to pay for.
     * @param _refundAddress The address to refund any overpayment to.
     */
    function payForGas(
        bytes32 _messageId,
        uint32 _destinationDomain,
        uint256 _gasAmount,
        address _refundAddress
    ) public payable override {
        uint256 _requiredPayment = quoteGasPayment(
            _destinationDomain,
            _gasAmount
        );
        require(msg.value >= _requiredPayment, "IGP: insufficient gas payment");
        uint256 _overpayment = msg.value - _requiredPayment;
        if (_overpayment > 0) {
            (bool _success, ) = _refundAddress.call{value: _overpayment}("");
            require(_success, "IGP: refund failed");
        }

        emit GasPayment(_messageId, _gasAmount, _requiredPayment);
    }

    /**
     * @notice Transfers the entire native token balance to the beneficiary.
     * @dev The beneficiary must be able to receive native tokens.
     */
    function claim() external {
        // Transfer the entire balance to the beneficiary.
        (bool success, ) = beneficiary.call{value: address(this).balance}("");
        require(success, "!transfer");
    }

    /**
     * @notice Sets the gas oracles for remote domains specified in the config array.
     * @param _configs An array of configs including the remote domain and gas oracles to set.
     */
    function setGasOracles(GasOracleConfig[] calldata _configs)
        external
        onlyOwner
    {
        uint256 _len = _configs.length;
        for (uint256 i = 0; i < _len; i++) {
            _setGasOracle(_configs[i].remoteDomain, _configs[i].gasOracle);
        }
    }

    /**
     * @notice Sets the beneficiary.
     * @param _beneficiary The new beneficiary.
     */
    function setBeneficiary(address _beneficiary) external onlyOwner {
        _setBeneficiary(_beneficiary);
    }

    // ============ Public Functions ============

    /**
     * @notice Quotes the amount of native tokens to pay for interchain gas.
     * @param _destinationDomain The domain of the message's destination chain.
     * @param _gasAmount The amount of destination gas to pay for.
     * @return The amount of native tokens required to pay for interchain gas.
     */
    function quoteGasPayment(uint32 _destinationDomain, uint256 _gasAmount)
        public
        view
        virtual
        override
        returns (uint256)
    {
        // Get the gas data for the destination domain.
        (
            uint128 _tokenExchangeRate,
            uint128 _gasPrice
        ) = getExchangeRateAndGasPrice(_destinationDomain);

        // The total cost quoted in destination chain's native token.
        uint256 _destinationGasCost = _gasAmount * uint256(_gasPrice);

        // Convert to the local native token.
        return (_destinationGasCost * _tokenExchangeRate) / DECIMALS;
    }

    /**
     * @notice Gets the token exchange rate and gas price from the configured gas oracle
     * for a given destination domain.
     * @param _destinationDomain The destination domain.
     * @return tokenExchangeRate The exchange rate of the remote native token quoted in the local native token.
     * @return gasPrice The gas price on the remote chain.
     */
    function getExchangeRateAndGasPrice(uint32 _destinationDomain)
        public
        view
        override
        returns (uint128 tokenExchangeRate, uint128 gasPrice)
    {
        IGasOracle _gasOracle = gasOracles[_destinationDomain];
        require(address(_gasOracle) != address(0), "!gas oracle");

        return _gasOracle.getExchangeRateAndGasPrice(_destinationDomain);
    }

    // ============ Internal Functions ============

    function _postDispatch(bytes calldata _metadata, bytes calldata _message)
        internal
        override
    {
        if (_metadata.length == 0) {
            payForGas(
                _message.id(),
                _message.destination(),
                DEFAULT_GAS_USAGE,
                _message.senderAddress()
            );
        } else {
            address refundAddress = _metadata.refundAddress();
            if (refundAddress != address(0))
                refundAddress = _message.senderAddress();
            payForGas(
                _message.id(),
                _message.destination(),
                _metadata.gasLimit(),
                refundAddress
            );
        }
    }

    /**
     * @notice Sets the beneficiary.
     * @param _beneficiary The new beneficiary.
     */
    function _setBeneficiary(address _beneficiary) internal {
        beneficiary = _beneficiary;
        emit BeneficiarySet(_beneficiary);
    }

    /**
     * @notice Sets the gas oracle for a remote domain.
     * @param _remoteDomain The remote domain.
     * @param _gasOracle The gas oracle.
     */
    function _setGasOracle(uint32 _remoteDomain, address _gasOracle) internal {
        gasOracles[_remoteDomain] = IGasOracle(_gasOracle);
        emit GasOracleSet(_remoteDomain, _gasOracle);
    }
}
