import { ethers } from 'ethers';

import type { types } from '@hyperlane-xyz/utils';

import { MultiProvider } from './providers/MultiProvider';
import { ChainMap, Connection } from './types';
import { objFilter, objMap, pick } from './utils/objects';

export type HyperlaneFactories = {
  [key: string]: ethers.ContractFactory;
};

export type HyperlaneContracts<Factories extends HyperlaneFactories> = {
  [Property in keyof Factories]: Awaited<
    ReturnType<Factories[Property]['deploy']>
  >;
};

export type HyperlaneContractsMap<Factories extends HyperlaneFactories> =
  ChainMap<HyperlaneContracts<Factories>>;

export type HyperlaneAddresses<Factories extends HyperlaneFactories> = {
  [Property in keyof Factories]: types.Address;
};

export type HyperlaneAddressesMap<Factories extends HyperlaneFactories> =
  ChainMap<HyperlaneAddresses<Factories>>;

export function serializeContractsMap<F extends HyperlaneFactories>(
  contractsMap: HyperlaneContractsMap<F>,
): HyperlaneAddressesMap<F> {
  return objMap(contractsMap, (_, contracts) => {
    return serializeContracts(contracts);
  });
}

export function serializeContracts<F extends HyperlaneFactories>(
  contracts: HyperlaneContracts<F>,
): HyperlaneAddresses<F> {
  return objMap(
    contracts,
    (_, contract) => contract.address,
  ) as HyperlaneAddresses<F>;
}

function getFactory(
  key: string,
  factories: HyperlaneFactories,
): ethers.ContractFactory {
  if (!(key in factories)) {
    throw new Error(`Factories entry missing for ${key}`);
  }
  return factories[key];
}

export function filterAddressesMap(
  addressesMap: HyperlaneAddressesMap<any>,
  factories: HyperlaneFactories,
): HyperlaneAddressesMap<typeof factories> {
  // Filter out addresses that we do not have factories for
  const pickedAddressesMap = objMap(addressesMap, (_, addresses) =>
    pick(addresses, Object.keys(factories)),
  );
  // Filter out chains for which we do not have a complete set of addresses
  return objFilter(
    pickedAddressesMap,
    (_, addresses): addresses is HyperlaneAddresses<typeof factories> => {
      return Object.keys(factories)
        .map((contract) => Object.keys(addresses).includes(contract))
        .every(Boolean);
    },
  );
}

export function attachContracts<F extends HyperlaneFactories>(
  addresses: HyperlaneAddresses<F>,
  factories: F,
): HyperlaneContracts<F> {
  return objMap(addresses, (key, address: types.Address) =>
    getFactory(key, factories).attach(address),
  ) as HyperlaneContracts<F>;
}

export function attachContractsMap<F extends HyperlaneFactories>(
  addressesMap: HyperlaneAddressesMap<F>,
  factories: F,
): HyperlaneContractsMap<F> {
  const filteredAddressesMap = filterAddressesMap(addressesMap, factories);
  return objMap(filteredAddressesMap, (_, addresses) =>
    attachContracts(addresses, factories),
  ) as HyperlaneContractsMap<F>;
}

export function connectContracts<F extends HyperlaneFactories>(
  contracts: HyperlaneContracts<F>,
  connection: Connection,
): HyperlaneContracts<F> {
  return objMap(contracts, (_, contract) =>
    contract.connect(connection),
  ) as HyperlaneContracts<F>;
}

export function connectContractsMap<F extends HyperlaneFactories>(
  contractsMap: ChainMap<HyperlaneContracts<F>>,
  multiProvider: MultiProvider,
): ChainMap<HyperlaneContracts<F>> {
  return objMap(contractsMap, (chain, contracts) =>
    connectContracts(contracts, multiProvider.getSignerOrProvider(chain)),
  );
}
