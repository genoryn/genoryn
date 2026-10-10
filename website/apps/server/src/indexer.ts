import { gzipSync } from 'node:zlib';
import { goriToGnr, type BlockSummary, type ChainStats, type NodeInfo } from '@genoryn/shared';
import { MAINNET_GENESIS_HASH, MAX_CIRCULATING, MAX_SUPPLY, NETWORKS, reward, treasuryIssued } from '@genoryn/shared';
import { blockHash, blockSize, bodyHash, encodeTx, hex, txId, type Block } from './codec';
import { toBlockSummary, type BlockRow, type DB, type Queries } from './db';
import type { LiveHub } from './live';
import { rpc } from './rpc';
import { config } from './config';

const log = (msg: string, extra: Record<string, unknown> = {}) =>
  console.log(JSON.stringify({ t: new Date().toISOString(), src: 'indexer', msg, ...extra }));

export class Indexer {
  info: NodeInfo | null = null;
  nodeOnline = false;
  lastError: string | null = null;
  hashSelfCheck: boolean | null = null;
  /** Unix seconds of the last successful node poll (persisted across restarts). */
  lastOkAt: number | null = null;
  private lastPersistAt = 0;
  private lastPersistHeight = -1;
  private running = false;
  private stopped = false;
  private totalFees = 0n;

  nodeStats: () => Record<string, unknown> = () => ({ nodes_connected: 0, nodes_24h: 0, nodes_7d: 0, nodes_all: 0, known_addresses: 0, reachable_now: 0, reachable_24h: 0, crawl_addresses: 0, last_crawl: null, seeds_online: 0, per_seed: {} });

  constructor(private db: DB, private q: Queries, private hub: LiveHub) {
    this.totalFees = this.q.totalFees.get() as bigint;
    // Serve the last known node view after a restart while the node is still unreachable.
    try {
      const saved = this.q.getMeta.get('last_node_info') as string | undefined;
      const parsed = saved ? (JSON.parse(saved) as NodeInfo) : null;
      if (parsed?.chain_id === NETWORKS[config.network].chainId) {
        this.info = parsed;
        const at = Number(this.q.getMeta.get('last_node_ok'));
        this.lastOkAt = Number.isFinite(at) && at > 0 ? at : null;
      }
    } catch {
      this.info = null;
    }
  }

  start() {
    const loop = async () => {
      if (this.stopped) return;
      try {
        await this.tick();
      } catch (e) {
        this.lastError = (e as Error).message;
        log('tick failed', { error: this.lastError });
        this.hub.publish({ type: 'stats', data: this.stats() });
      }
      setTimeout(loop, config.pollMs).unref();
    };
    void loop();
  }

  stop() {
    this.stopped = true;
  }

  private tipHeight(): number {
    const t = this.q.tip.get() as { height: number } | undefined;
    return t?.height ?? 0;
  }

  async tick() {
    if (this.running) return;
    this.running = true;
    try {
      let info: NodeInfo;
      try {
        info = await rpc.info();
        this.nodeOnline = true;
      } catch (e) {
        if (this.nodeOnline && this.lastOkAt !== null) this.q.setMeta.run('last_node_ok', String(this.lastOkAt));
        this.nodeOnline = false;
        throw new Error(`node unreachable: ${(e as Error).message}`);
      }
      this.info = info;
      this.lastOkAt = Math.floor(Date.now() / 1000);
      this.persistNodeView(info);
      this.dropForeignChain(info.genesis_hash);
      let tip = this.tipHeight();

      if (tip > info.height) {
        log('node height below index, rolling back', { tip, node: info.height });
        this.rollbackTo(info.height);
        tip = info.height;
      }

      let fetched = 0;
      while (tip < info.height && !this.stopped) {
        const h = tip + 1;
        const res = await rpc.block(h);
        if (!res) break;
        const { block, rawText } = res;
        const prev = hex(block.header.previous);
        if (h > 1) {
          const ourPrev = this.q.hashAt.get(h - 1) as string | undefined;
          if (ourPrev !== prev) {
            log('reorg detected', { height: h, expected: ourPrev, got: prev });
            this.rollbackTo(h - 2);
            tip = Math.max(0, h - 2);
            continue;
          }
        }
        const summary = this.store(block, rawText);
        tip = h;
        fetched++;
        if (info.height - h < 20) this.hub.publish({ type: 'block', data: summary });
      }

      // Confirm our tip equals the node's best hash; otherwise unwind one block and retry next tick.
      if (tip === info.height && tip > 0) {
        const ours = this.q.hashAt.get(tip) as string | undefined;
        const match = ours === info.best_hash;
        if (this.hashSelfCheck === null) log('hash self-check', { height: tip, match });
        this.hashSelfCheck = match;
        if (!match) {
          log('tip mismatch, unwinding', { tip, ours, node: info.best_hash });
          this.rollbackTo(tip - 1);
          this.hub.publish({ type: 'reorg', data: { height: tip - 1 } });
        }
      }
      if (fetched > 0) log('indexed', { fetched, tip });
      this.lastError = null;
      this.hub.publish({ type: 'stats', data: this.stats() });
    } finally {
      this.running = false;
    }
  }

  private persistNodeView(info: NodeInfo) {
    if (info.height === this.lastPersistHeight && Date.now() - this.lastPersistAt < 60_000) return;
    this.q.setMeta.run('last_node_info', JSON.stringify(info));
    this.q.setMeta.run('last_node_ok', String(this.lastOkAt));
    this.lastPersistAt = Date.now();
    this.lastPersistHeight = info.height;
  }

  /** An index built from another chain (e.g. the retired pre-1.0 mainnet) is discarded, not reorganised block by block. */
  private dropForeignChain(genesis: string | undefined) {
    if (!genesis) return;
    const first = this.q.hashPrevAt.get(1) as string | undefined;
    if (first === undefined || first === genesis) return;
    log('index belongs to another chain, discarding', { indexed_genesis: first, node_genesis: genesis });
    this.q.deleteFrom.run(1);
    this.totalFees = 0n;
    this.hashSelfCheck = null;
    this.hub.publish({ type: 'reorg', data: { height: 0 } });
  }

  private rollbackTo(height: number) {
    const fees = this.db.prepare('SELECT COALESCE(SUM(fees),0) FROM blocks WHERE height > ?').pluck().safeIntegers(true).get(height) as bigint;
    this.q.deleteFrom.run(height + 1);
    this.totalFees -= fees;
    this.hub.publish({ type: 'reorg', data: { height } });
  }

  private store(block: Block, rawText: string): BlockSummary {
    const h = block.header;
    const hash = blockHash(h);
    if (bodyHash(block.body) !== hex(h.body_hash)) throw new Error(`body commitment mismatch at ${h.height}`);
    const prevTime = h.height > 1 ? (this.q.timeAt.get(h.height - 1) as number | undefined) : undefined;
    const all = [block.body.miner, ...(block.body.treasury ? [block.body.treasury] : []), ...block.body.transactions];
    let fees = 0n;
    let actions = 0;
    for (const t of block.body.transactions) fees += BigInt(t.bundle.value_balance);
    for (const t of all) actions += t.bundle.actions.length;
    const minerReward = -BigInt(block.body.miner.bundle.value_balance);
    const treasuryReward = block.body.treasury ? -BigInt(block.body.treasury.bundle.value_balance) : 0n;

    const row: BlockRow = {
      height: h.height,
      hash,
      previous: hex(h.previous),
      version: h.version,
      network: h.network.toLowerCase(),
      timestamp: h.timestamp,
      difficulty: String(h.difficulty),
      nonce: String(h.nonce),
      pow_seed: hex(h.pow_seed),
      body_hash: hex(h.body_hash),
      notes_root: hex(h.notes_root),
      nullifiers_root: hex(h.nullifiers_root),
      size: blockSize(block),
      tx_count: all.length,
      transfer_count: block.body.transactions.length,
      action_count: actions,
      fees,
      miner_reward: minerReward,
      treasury_reward: treasuryReward,
      solve_time: prevTime !== undefined ? h.timestamp - prevTime : null,
      raw: gzipSync(rawText),
    };

    const write = this.db.transaction(() => {
      this.q.insertBlock.run(row);
      all.forEach((tx, idx) => {
        const txid = txId(tx);
        const vb = BigInt(tx.bundle.value_balance);
        this.q.insertTx.run({
          txid,
          height: h.height,
          idx,
          kind: tx.context.kind,
          protocol: tx.context.protocol,
          expires: tx.context.expires,
          nonce: hex(tx.context.nonce),
          flags: tx.bundle.flags,
          value_balance: vb,
          fee: tx.context.kind === 0 ? vb : 0n,
          anchor: hex(tx.bundle.anchor),
          actions: tx.bundle.actions.length,
          proof_bytes: tx.bundle.proof.length,
          binding_signature: hex(tx.bundle.binding_signature),
          size: encodeTx(tx).length,
        });
        tx.bundle.actions.forEach((a, ai) =>
          this.q.insertAction.run({
            txid,
            height: h.height,
            idx: ai,
            nullifier: hex(a.nullifier),
            rk: hex(a.rk),
            cmx: hex(a.cmx),
            ephemeral_key: hex(a.ephemeral_key),
            value_commitment: hex(a.value_commitment),
            signature: hex(a.signature),
            encrypted_note_bytes: a.encrypted_note.length,
            outgoing_ciphertext_bytes: a.outgoing_ciphertext.length,
          }),
        );
      });
    });
    write();
    this.totalFees += fees;
    return toBlockSummary(row);
  }

  stats(): ChainStats {
    const info = this.info;
    const tip = this.q.tip.get() as { height: number; hash: string } | undefined;
    const totals = this.q.totals.get() as { blocks: number; transfers: number; actions: number; bytes: number };
    const avg = this.q.avgSolve.get(60) as number | null;
    const last = tip ? (this.q.timeAt.get(tip.height) as number) : null;
    const height = info?.height ?? tip?.height ?? 0;
    const stale = !this.nodeOnline;
    return {
      network: info?.network ?? config.network,
      chain_id: info?.chain_id ?? NETWORKS[config.network].chainId,
      genesis_hash: info?.genesis_hash ?? (config.network === 'mainnet' ? MAINNET_GENESIS_HASH : ''),
      height,
      indexed_height: tip?.height ?? 0,
      best_hash: info?.best_hash ?? tip?.hash ?? '',
      difficulty: info?.difficulty ?? '0',
      hashrate: info?.estimated_hashrate_hs ?? 0,
      mempool: info?.mempool_count ?? 0,
      peers: info?.peer_count ?? 0,
      inbound: info?.inbound_peers ?? 0,
      outbound: info?.outbound_peers ?? 0,
      issued_supply: info?.issued_supply ?? '0',
      max_supply: MAX_SUPPLY.toString(),
      block_reward: reward(height + 1).total.toString(),
      avg_block_time: avg,
      total_transfers: totals.transfers,
      total_actions: totals.actions,
      total_fees: this.totalFees.toString(),
      usernames: info?.registered_usernames ?? 0,
      ...(this.nodeStats() as Pick<ChainStats, 'nodes_connected' | 'nodes_24h' | 'nodes_7d' | 'nodes_all' | 'known_addresses' | 'reachable_now' | 'reachable_24h' | 'crawl_addresses' | 'last_crawl' | 'seeds_online' | 'per_seed'>),
      // Rough count of mining computers: target-implied hashrate ÷ ~200 H/s for a typical light-mode CPU miner.
      est_miners: Math.round((info?.estimated_hashrate_hs ?? 0) / 200),
      treasury_burned: (config.network === 'mainnet' ? treasuryIssued(height) : 0n).toString(),
      max_circulating: (config.network === 'mainnet' ? MAX_CIRCULATING : MAX_SUPPLY).toString(),
      database_bytes: info?.database_bytes ?? null,
      node_online: this.nodeOnline,
      node_version: config.nodeVersion,
      last_block_time: last,
      updated_at: Math.floor(Date.now() / 1000),
      stale,
      last_updated: this.lastOkAt,
      error: stale ? (this.lastError ?? 'node unreachable') : null,
    };
  }

  describe() {
    const stale = !this.nodeOnline;
    return {
      node_online: this.nodeOnline,
      stale,
      last_updated: this.lastOkAt,
      error: stale ? (this.lastError ?? 'node unreachable') : null,
      last_error: this.lastError,
      hash_self_check: this.hashSelfCheck,
      fees_gnr: goriToGnr(this.totalFees),
    };
  }
}
