/** API contract between apps/server and apps/web. All gori values are decimal strings. */

export interface NodeInfo {
  network: string;
  chain_id: string;
  genesis_hash: string;
  version: number;
  height: number;
  best_hash: string;
  difficulty: string;
  estimated_hashrate_hs: number;
  peer_count: number;
  inbound_peers: number;
  outbound_peers: number;
  database_bytes: number | null;
  mempool_count: number;
  issued_supply: string;
  registered_usernames: number;
  max_registered_usernames: number;
  public_rpc: boolean;
  mainnet_finalized: boolean;
  privacy_status: string;
}

export interface BlockSummary {
  height: number;
  hash: string;
  previous: string;
  timestamp: number;
  difficulty: string;
  nonce: string;
  size: number;
  tx_count: number;
  transfer_count: number;
  action_count: number;
  fees: string;
  miner_reward: string;
  treasury_reward: string;
  solve_time: number | null;
}

export interface BlockDetail extends BlockSummary {
  version: number;
  network: string;
  pow_seed: string;
  body_hash: string;
  notes_root: string;
  nullifiers_root: string;
  confirmations: number;
  next: string | null;
  transactions: TxSummary[];
}

export interface TxSummary {
  txid: string;
  height: number;
  index: number;
  kind: number;
  kind_label: string;
  fee: string;
  value_balance: string;
  actions: number;
  size: number;
  timestamp: number;
}

export interface ActionDetail {
  index: number;
  nullifier: string;
  rk: string;
  cmx: string;
  ephemeral_key: string;
  value_commitment: string;
  encrypted_note_bytes: number;
  outgoing_ciphertext_bytes: number;
  signature: string;
}

export interface TxDetail extends TxSummary {
  block_hash: string;
  confirmations: number;
  protocol: number;
  network: string;
  expires: number;
  nonce: string;
  flags: number;
  anchor: string;
  proof_bytes: number;
  binding_signature: string;
  actions_detail: ActionDetail[];
}

export interface ChainStats {
  network: string;
  chain_id: string;
  genesis_hash: string;
  height: number;
  indexed_height: number;
  best_hash: string;
  difficulty: string;
  hashrate: number;
  mempool: number;
  peers: number;
  inbound: number;
  outbound: number;
  issued_supply: string;
  max_supply: string;
  block_reward: string;
  avg_block_time: number | null;
  total_transfers: number;
  total_actions: number;
  total_fees: string;
  usernames: number;
  treasury_burned: string;
  nodes_connected: number;
  nodes_24h: number;
  nodes_7d: number;
  nodes_all: number;
  known_addresses: number;
  reachable_now: number;
  reachable_24h: number;
  crawl_addresses: number;
  last_crawl: number | null;
  est_miners: number;
  seeds_online: number;
  per_seed: Record<string, number>;
  max_circulating: string;
  database_bytes: number | null;
  node_online: boolean;
  node_version: string;
  last_block_time: number | null;
  updated_at: number;
  /** True when the node did not answer the last poll; the other fields are the last known values. */
  stale: boolean;
  /** Unix seconds of the last successful node poll, null if the node has never answered. */
  last_updated: number | null;
  /** Last node/indexer error while stale. */
  error: string | null;
}

export interface MinerStats {
  online: boolean;
  threads: number | null;
  hashrate: number | null;
  hashes: number | null;
  accepted: number | null;
  rejected: number | null;
  height: number | null;
  last_event_at: number | null;
  address: string | null;
  raw: Record<string, unknown> | null;
}

export interface PeerSummary {
  address: string;
  inbound: boolean;
  height: number;
  link?: 'clearnet' | 'tor' | string;
  network?: 'ipv4' | 'ipv6' | 'onion' | string;
}

export interface ReleaseAsset {
  name: string;
  url: string;
  size: number;
  downloads: number;
  sha256: string | null;
  platform: 'windows' | 'linux' | 'macos' | 'other';
}

export interface Release {
  tag: string;
  name: string;
  published_at: string;
  url: string;
  body: string;
  prerelease: boolean;
  assets: ReleaseAsset[];
}

export type SearchResult =
  | { type: 'block'; height: number; hash: string }
  | { type: 'tx'; txid: string; height: number }
  | { type: 'nullifier'; nullifier: string; txid: string; height: number }
  | { type: 'commitment'; cmx: string; txid: string; height: number }
  | { type: 'address'; address: string; valid: boolean; network: string | null; error: string | null }
  | { type: 'username'; username: string }
  | { type: 'none'; query: string; reason: string };

export interface NameRecord {
  name: string;
  receiver: string | null;
  owner: string;
  sequence: number;
  registered_height: number;
  updated_height: number;
  txid: string;
  burned_atomic: string;
}

export interface NameStatus {
  name: string;
  registered: NameRecord | null;
  pending: boolean;
  available: boolean;
  price_atomic: string;
  update_price_atomic: string;
  height: number;
}

export interface ChartPoint {
  height: number;
  timestamp: number;
  value: number;
}

export interface LiveEvent {
  type: 'stats' | 'block' | 'miner' | 'reorg';
  data: unknown;
}
