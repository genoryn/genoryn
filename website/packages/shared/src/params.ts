/** Protocol constants mirrored from genoryn-core (GENORYN mainnet 1.0). */

export const GENESIS_MESSAGE = 'Privacy is not a feature. Privacy is the default.';
export const SLOGAN = "Privacy is not a crime. It's a right.";
export const TAGLINE = 'Private by design. Finite by protocol.';

export const REPO = 'genoryn/genoryn';
export const REPO_URL = `https://github.com/${REPO}`;
export const RELEASES_URL = `${REPO_URL}/releases`;
export const LATEST_RELEASE_URL = `${REPO_URL}/releases/latest`;

export const TARGET_SECONDS = 60;
export const BLOCKS_PER_YEAR = (365 * 24 * 60 * 60) / TARGET_SECONDS; // 525,600
export const HALVING_INTERVAL = 4 * BLOCKS_PER_YEAR; // 2,102,400
/** Blocks 1–500 pay the early subsidy of 1,000 GNR each, and nothing else. */
export const EARLY_BLOCKS = 500;
export const EARLY_REWARD_GNR = 1_000;
/** First height paying the normal reward and the treasury share. */
export const NORMAL_START = EARLY_BLOCKS + 1;
export const TREASURY_EMISSION_BLOCKS = 210_000;
/** Last height paying the treasury share (heights 501–210,500). */
export const TREASURY_END = EARLY_BLOCKS + TREASURY_EMISSION_BLOCKS;
export const TREASURY_LOCK_HEIGHT = 10 * BLOCKS_PER_YEAR;

export const MIN_FEE_GORI = 10_000_000n;
export const MAX_TRANSACTIONS_PER_BLOCK = 64;
export const MAX_BLOCK_BYTES = 2_000_000;
export const MAX_TX_BYTES = 200_000;
export const MIN_ACTIONS = 2;
export const MAX_ACTIONS = 16;
export const ANCHOR_WINDOW = 120;
export const MTP_WINDOW = 11;
export const FUTURE_LIMIT_SECONDS = 120;
export const LWMA_WINDOW = 60;
export const RANDOMX_EPOCH = 2048;
export const RANDOMX_LAG = 65;
export const NOTE_TREE_DEPTH = 32;
export const MAX_P2P_FRAME = 2_100_000;
export const MAX_CONNECTIONS = 16;
export const MAX_OUTBOUND = 8;

export type NetworkName = 'devnet' | 'testnet' | 'mainnet';

export interface NetworkParams {
  name: NetworkName;
  id: number;
  chainId: string;
  hrp: string;
  p2pPort: number;
  adminRpcPort: number;
  publicRpcPort: number;
  magic: string;
  maturity: number;
  initialDifficulty: number;
  genesisTag: string;
  live: boolean;
  finalized: boolean;
}

const base = (name: NetworkName, id: number, port: number, hrp: string, maturity: number, initialDifficulty: number, live: boolean): NetworkParams => ({
  name,
  id,
  chainId: `genoryn-${name}-1.0`,
  hrp,
  p2pPort: port,
  adminRpcPort: port + 1,
  publicRpcPort: port + 2,
  magic: `0x474e52${(0x10 + id).toString(16)}`,
  maturity,
  initialDifficulty,
  genesisTag: `2026-10-10-${name}-1.0`,
  live,
  finalized: true,
});

export const NETWORKS: Record<NetworkName, NetworkParams> = {
  mainnet: base('mainnet', 3, 29710, 'gnr', 60, 150_000, true),
  testnet: base('testnet', 2, 29510, 'gnrt', 60, 100, false),
  devnet: base('devnet', 1, 29410, 'gnrd', 2, 1, false),
};

export const DEFAULT_NETWORK: NetworkName = 'mainnet';
export const MAINNET_LAUNCH = '2026-10-10';
export const MAINNET_GENESIS_HASH = '2b27dda9c2adcf9d61a58efc43a40213adc7fb2addfdae210f3357d72adf6b2e';
/** Pre-1.0 network (`genoryn-mainnet-v1`, releases 0.4–0.6). Retired; its coins do not carry over. */
export const RETIRED_MAINNET_GENESIS = 'c5fc87d996c7fd9dc69ff0cdf96183c65e1be5c9293748b33c31cd8fb6e1d708';
export const NODE_VERSION = '1.0.0';
export const MAINNET_ONION_P2P_PORT = 29713;
/** Empty until the onion service of genoryn.io is published; every UI hides it while empty. */
export const GENORYN_IO_ONION: string = '2a6gncfqgp56dgd73vbbicav4552lepqzk6rakltyuuz7hqmttq2diad.onion';
/** Seed 1's onion service (P2P 29710, public RPC 29712). */
export const SEED1_ONION = 'ycnowntcndxfvr3rkeqkiyfkfvxoqmee3qsrfsz43hq5rkxkmsklrcad.onion';
export const MAINNET_SEED = '187.7.72.137';
export const GENORYN_IO_SEED = '2.24.162.163';
export const MAINNET_SEEDS_FILE = 'seeds/mainnet-1.0.txt';
export const MAINNET_ONION_SEEDS_FILE = 'seeds/mainnet-1.0-onion.txt';
export const MAINNET_DATA_DIR = '~/.genoryn/mainnet-1.0';
export const MAINNET_DATA_DIR_WINDOWS = '%USERPROFILE%\\.genoryn\\mainnet-1.0';
export const EMISSION_DOC_URL = `${REPO_URL}/blob/main/docs/protocol/emission.md`;
/**
 * The mainnet treasury share (1 GNR per block, heights 501–210,500) is paid to a provably
 * unspendable burn address. Exact ceiling: 20,789,999.999950064 GNR (see LIFETIME_ROUNDING_REMAINDER).
 */
export const MAX_CIRCULATING_GNR = 20_790_000;

export const NAME_PRICES = [
  { lengths: '3–4 characters', gnr: 1000n },
  { lengths: '5–6 characters', gnr: 100n },
  { lengths: '7+ characters', gnr: 10n },
] as const;
export const NAME_UPDATE_PRICE_GNR = 1n;
export const RESERVED_NAMES = ['admin', 'administrator', 'genoryn', 'support', 'security', 'treasury'] as const;
export const nameRegistrationPriceGnr = (name: string): bigint => (name.length <= 4 ? 1000n : name.length <= 6 ? 100n : 10n);

export const genesisPreimage = (n: NetworkParams) =>
  `${n.chainId}|${GENESIS_MESSAGE}|GNR|21000000|1000000000000|RandomX-v1|Ironwood-v3|early:${EARLY_BLOCKS}x${EARLY_REWARD_GNR}|${n.genesisTag}`;

export const FEE_TIERS = [
  { id: 'economy', label: 'Economy', multiplier: 1n },
  { id: 'standard', label: 'Standard', multiplier: 2n },
  { id: 'priority', label: 'Priority', multiplier: 5n },
] as const;

export const TX_KINDS: Record<number, string> = {
  0: 'Shielded transfer',
  1: 'Miner issuance',
  2: 'Treasury issuance',
};
