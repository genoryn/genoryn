import Database from 'better-sqlite3';
import { mkdirSync } from 'node:fs';
import { dirname } from 'node:path';
import { TX_KINDS, type BlockSummary, type ChartPoint, type TxSummary } from '@genoryn/shared';

export type DB = Database.Database;

export function openDb(path: string): DB {
  mkdirSync(dirname(path), { recursive: true });
  const db = new Database(path);
  db.pragma('journal_mode = WAL');
  db.pragma('synchronous = NORMAL');
  db.pragma('foreign_keys = ON');
  db.exec(`
    CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
    CREATE TABLE IF NOT EXISTS blocks (
      height INTEGER PRIMARY KEY,
      hash TEXT NOT NULL UNIQUE,
      previous TEXT NOT NULL,
      version INTEGER NOT NULL,
      network TEXT NOT NULL,
      timestamp INTEGER NOT NULL,
      difficulty TEXT NOT NULL,
      nonce TEXT NOT NULL,
      pow_seed TEXT NOT NULL,
      body_hash TEXT NOT NULL,
      notes_root TEXT NOT NULL,
      nullifiers_root TEXT NOT NULL,
      size INTEGER NOT NULL,
      tx_count INTEGER NOT NULL,
      transfer_count INTEGER NOT NULL,
      action_count INTEGER NOT NULL,
      fees INTEGER NOT NULL,
      miner_reward INTEGER NOT NULL,
      treasury_reward INTEGER NOT NULL,
      solve_time INTEGER,
      raw BLOB NOT NULL
    );
    CREATE INDEX IF NOT EXISTS blocks_time ON blocks(timestamp);
    CREATE TABLE IF NOT EXISTS txs (
      txid TEXT PRIMARY KEY,
      height INTEGER NOT NULL REFERENCES blocks(height) ON DELETE CASCADE,
      idx INTEGER NOT NULL,
      kind INTEGER NOT NULL,
      protocol INTEGER NOT NULL,
      expires INTEGER NOT NULL,
      nonce TEXT NOT NULL,
      flags INTEGER NOT NULL,
      value_balance INTEGER NOT NULL,
      fee INTEGER NOT NULL,
      anchor TEXT NOT NULL,
      actions INTEGER NOT NULL,
      proof_bytes INTEGER NOT NULL,
      binding_signature TEXT NOT NULL,
      size INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS txs_height ON txs(height, idx);
    CREATE INDEX IF NOT EXISTS txs_kind ON txs(kind, height);
    CREATE TABLE IF NOT EXISTS actions (
      txid TEXT NOT NULL REFERENCES txs(txid) ON DELETE CASCADE,
      height INTEGER NOT NULL,
      idx INTEGER NOT NULL,
      nullifier TEXT NOT NULL,
      rk TEXT NOT NULL,
      cmx TEXT NOT NULL,
      ephemeral_key TEXT NOT NULL,
      value_commitment TEXT NOT NULL,
      signature TEXT NOT NULL,
      encrypted_note_bytes INTEGER NOT NULL,
      outgoing_ciphertext_bytes INTEGER NOT NULL,
      PRIMARY KEY (txid, idx)
    );
    CREATE INDEX IF NOT EXISTS actions_nf ON actions(nullifier);
    CREATE INDEX IF NOT EXISTS actions_cmx ON actions(cmx);
  `);
  return db;
}

export interface BlockRow {
  height: number;
  hash: string;
  previous: string;
  version: number;
  network: string;
  timestamp: number;
  difficulty: string;
  nonce: string;
  pow_seed: string;
  body_hash: string;
  notes_root: string;
  nullifiers_root: string;
  size: number;
  tx_count: number;
  transfer_count: number;
  action_count: number;
  fees: number | bigint;
  miner_reward: number | bigint;
  treasury_reward: number | bigint;
  solve_time: number | null;
  raw?: Buffer;
}

export interface TxRow {
  txid: string;
  height: number;
  idx: number;
  kind: number;
  protocol: number;
  expires: number;
  nonce: string;
  flags: number;
  value_balance: number | bigint;
  fee: number | bigint;
  anchor: string;
  actions: number;
  proof_bytes: number;
  binding_signature: string;
  size: number;
  timestamp?: number;
}

const SUMMARY_COLS = 'height, hash, previous, timestamp, difficulty, nonce, size, tx_count, transfer_count, action_count, fees, miner_reward, treasury_reward, solve_time';

export const toBlockSummary = (r: BlockRow): BlockSummary => ({
  height: r.height,
  hash: r.hash,
  previous: r.previous,
  timestamp: r.timestamp,
  difficulty: r.difficulty,
  nonce: r.nonce,
  size: r.size,
  tx_count: r.tx_count,
  transfer_count: r.transfer_count,
  action_count: r.action_count,
  fees: String(r.fees),
  miner_reward: String(r.miner_reward),
  treasury_reward: String(r.treasury_reward),
  solve_time: r.solve_time,
});

export const toTxSummary = (r: TxRow): TxSummary => ({
  txid: r.txid,
  height: r.height,
  index: r.idx,
  kind: r.kind,
  kind_label: TX_KINDS[r.kind] ?? `Kind ${r.kind}`,
  fee: String(r.fee),
  value_balance: String(r.value_balance),
  actions: r.actions,
  size: r.size,
  timestamp: r.timestamp ?? 0,
});

export function queries(db: DB) {
  const s = {
    tip: db.prepare('SELECT height, hash FROM blocks ORDER BY height DESC LIMIT 1'),
    hashAt: db.prepare('SELECT hash FROM blocks WHERE height = ?').pluck(),
    hashPrevAt: db.prepare('SELECT previous FROM blocks WHERE height = ?').pluck(),
    deleteFrom: db.prepare('DELETE FROM blocks WHERE height >= ?'),
    timeAt: db.prepare('SELECT timestamp FROM blocks WHERE height = ?').pluck(),
    insertBlock: db.prepare(`INSERT INTO blocks (height, hash, previous, version, network, timestamp, difficulty, nonce, pow_seed, body_hash, notes_root, nullifiers_root, size, tx_count, transfer_count, action_count, fees, miner_reward, treasury_reward, solve_time, raw)
      VALUES (@height, @hash, @previous, @version, @network, @timestamp, @difficulty, @nonce, @pow_seed, @body_hash, @notes_root, @nullifiers_root, @size, @tx_count, @transfer_count, @action_count, @fees, @miner_reward, @treasury_reward, @solve_time, @raw)`),
    insertTx: db.prepare(`INSERT OR REPLACE INTO txs (txid, height, idx, kind, protocol, expires, nonce, flags, value_balance, fee, anchor, actions, proof_bytes, binding_signature, size)
      VALUES (@txid, @height, @idx, @kind, @protocol, @expires, @nonce, @flags, @value_balance, @fee, @anchor, @actions, @proof_bytes, @binding_signature, @size)`),
    insertAction: db.prepare(`INSERT OR REPLACE INTO actions (txid, height, idx, nullifier, rk, cmx, ephemeral_key, value_commitment, signature, encrypted_note_bytes, outgoing_ciphertext_bytes)
      VALUES (@txid, @height, @idx, @nullifier, @rk, @cmx, @ephemeral_key, @value_commitment, @signature, @encrypted_note_bytes, @outgoing_ciphertext_bytes)`),
    latestBlocks: db.prepare(`SELECT ${SUMMARY_COLS} FROM blocks WHERE height <= ? ORDER BY height DESC LIMIT ?`),
    blockByHeight: db.prepare('SELECT * FROM blocks WHERE height = ?'),
    blockByHash: db.prepare('SELECT * FROM blocks WHERE hash = ?'),
    txsInBlock: db.prepare('SELECT t.*, b.timestamp FROM txs t JOIN blocks b ON b.height = t.height WHERE t.height = ? ORDER BY idx'),
    tx: db.prepare('SELECT t.*, b.timestamp FROM txs t JOIN blocks b ON b.height = t.height WHERE t.txid = ?'),
    actionsOf: db.prepare('SELECT * FROM actions WHERE txid = ? ORDER BY idx'),
    byNullifier: db.prepare('SELECT txid, height, idx FROM actions WHERE nullifier = ? LIMIT 1'),
    byCmx: db.prepare('SELECT txid, height, idx FROM actions WHERE cmx = ? LIMIT 1'),
    latestTransfers: db.prepare('SELECT t.*, b.timestamp FROM txs t JOIN blocks b ON b.height = t.height WHERE t.kind = 0 ORDER BY t.height DESC, t.idx DESC LIMIT ?'),
    latestTxs: db.prepare('SELECT t.*, b.timestamp FROM txs t JOIN blocks b ON b.height = t.height ORDER BY t.height DESC, t.idx DESC LIMIT ? OFFSET ?'),
    totals: db.prepare('SELECT COUNT(*) AS blocks, COALESCE(SUM(transfer_count),0) AS transfers, COALESCE(SUM(action_count),0) AS actions, COALESCE(SUM(size),0) AS bytes FROM blocks'),
    totalFees: db.prepare('SELECT COALESCE(SUM(fees),0) FROM blocks').pluck().safeIntegers(true),
    avgSolve: db.prepare('SELECT AVG(solve_time) FROM (SELECT solve_time FROM blocks WHERE solve_time IS NOT NULL ORDER BY height DESC LIMIT ?)').pluck(),
    chart: db.prepare('SELECT height, timestamp, difficulty, solve_time, transfer_count, fees, size, action_count FROM blocks ORDER BY height DESC LIMIT ?'),
    rawBlock: db.prepare('SELECT raw FROM blocks WHERE height = ?').pluck(),
    getMeta: db.prepare('SELECT v FROM meta WHERE k = ?').pluck(),
    setMeta: db.prepare('INSERT INTO meta (k, v) VALUES (?, ?) ON CONFLICT(k) DO UPDATE SET v = excluded.v'),
    heightsForSitemap: db.prepare('SELECT height FROM blocks ORDER BY height DESC LIMIT ?').pluck(),
  };
  return {
    ...s,
    chartSeries(limit: number, metric: 'difficulty' | 'blocktime' | 'txs' | 'fees' | 'size' | 'actions'): ChartPoint[] {
      const rows = s.chart.all(limit) as Array<{ height: number; timestamp: number; difficulty: string; solve_time: number | null; transfer_count: number; fees: number; size: number; action_count: number }>;
      return rows
        .reverse()
        .filter((r) => metric !== 'blocktime' || r.solve_time !== null)
        .map((r) => ({
          height: r.height,
          timestamp: r.timestamp,
          value:
            metric === 'difficulty' ? Number(r.difficulty)
            : metric === 'blocktime' ? (r.solve_time as number)
            : metric === 'txs' ? r.transfer_count
            : metric === 'fees' ? Number(r.fees)
            : metric === 'actions' ? r.action_count
            : r.size,
        }));
    },
  };
}

export type Queries = ReturnType<typeof queries>;
