import type { FastifyInstance, FastifyReply } from 'fastify';
import { gunzipSync } from 'node:zlib';
import {
  MAINNET_ONION_SEEDS_FILE,
  MAINNET_SEEDS_FILE,
  NETWORKS,
  REPO_URL,
  TX_KINDS,
  canonicalUsername,
  emissionCurve,
  isHex64,
  supply,
  validateAddress,
  type ActionDetail,
  type BlockDetail,
  type SearchResult,
  type TxDetail,
} from '@genoryn/shared';
import { parseBlocksJson } from './codec';
import { toBlockSummary, toTxSummary, type BlockRow, type Queries, type TxRow } from './db';
import { getReleases } from './github';
import type { Indexer } from './indexer';
import type { LiveHub } from './live';
import type { MinerWatcher } from './miner';
import { RpcError, rpc } from './rpc';
import { config } from './config';

interface Ctx {
  q: Queries;
  indexer: Indexer;
  hub: LiveHub;
  miner: MinerWatcher;
}

const notFound = (reply: FastifyReply, what: string) => reply.code(404).send({ error: `${what} not found` });
const clampInt = (v: unknown, def: number, min: number, max: number) => {
  const n = Number.parseInt(String(v ?? ''), 10);
  return Number.isFinite(n) ? Math.min(max, Math.max(min, n)) : def;
};

function maskIp(addr: string): string {
  const m = addr.match(/^(\d+)\.(\d+)\.\d+\.\d+:(\d+)$/);
  if (m) return `${m[1]}.${m[2]}.x.x:${m[3]}`;
  const v6 = addr.match(/^\[([0-9a-f:]+)\]:(\d+)$/i);
  if (v6) return `[${v6[1]!.split(':').slice(0, 2).join(':')}::x]:${v6[2]}`;
  return 'hidden';
}

export function registerApi(app: FastifyInstance, ctx: Ctx) {
  const { q, indexer, hub, miner } = ctx;
  const tipHeight = () => (q.tip.get() as { height: number } | undefined)?.height ?? 0;

  const findBlock = (id: string): BlockRow | undefined => {
    if (/^\d{1,12}$/.test(id)) return q.blockByHeight.get(Number(id)) as BlockRow | undefined;
    if (isHex64(id)) return q.blockByHash.get(id.toLowerCase()) as BlockRow | undefined;
    return undefined;
  };

  const blockDetail = (b: BlockRow): BlockDetail => {
    const tip = tipHeight();
    const next = q.hashAt.get(b.height + 1) as string | undefined;
    return {
      ...toBlockSummary(b),
      version: b.version,
      network: b.network,
      pow_seed: b.pow_seed,
      body_hash: b.body_hash,
      notes_root: b.notes_root,
      nullifiers_root: b.nullifiers_root,
      confirmations: tip - b.height + 1,
      next: next ?? null,
      transactions: (q.txsInBlock.all(b.height) as TxRow[]).map(toTxSummary),
    };
  };

  const txDetail = (t: TxRow): TxDetail => {
    const actions = (q.actionsOf.all(t.txid) as Array<Record<string, unknown>>).map(
      (a): ActionDetail => ({
        index: a.idx as number,
        nullifier: a.nullifier as string,
        rk: a.rk as string,
        cmx: a.cmx as string,
        ephemeral_key: a.ephemeral_key as string,
        value_commitment: a.value_commitment as string,
        encrypted_note_bytes: a.encrypted_note_bytes as number,
        outgoing_ciphertext_bytes: a.outgoing_ciphertext_bytes as number,
        signature: a.signature as string,
      }),
    );
    return {
      ...toTxSummary(t),
      block_hash: q.hashAt.get(t.height) as string,
      confirmations: tipHeight() - t.height + 1,
      protocol: t.protocol,
      network: config.network,
      expires: t.expires,
      nonce: t.nonce,
      flags: t.flags,
      anchor: t.anchor,
      proof_bytes: t.proof_bytes,
      binding_signature: t.binding_signature,
      actions_detail: actions,
    };
  };

  app.get('/api/v1/health', async () => ({ ok: true, ...indexer.describe(), sse_clients: hub.size, indexed: tipHeight() }));

  app.get('/api/v1/stats', async () => indexer.stats());

  app.get('/api/v1/live', (req, reply) => {
    if (!hub.attach(reply)) return reply.code(503).send({ error: 'too many live connections' });
    reply.hijack();
  });

  app.get('/api/v1/blocks', async (req) => {
    const qs = req.query as Record<string, string>;
    const limit = clampInt(qs.limit, 20, 1, 100);
    const before = clampInt(qs.before, tipHeight(), 0, Number.MAX_SAFE_INTEGER);
    const rows = q.latestBlocks.all(before, limit) as BlockRow[];
    return { tip: tipHeight(), blocks: rows.map(toBlockSummary) };
  });

  app.get('/api/v1/block/:id', async (req, reply) => {
    const b = findBlock((req.params as { id: string }).id);
    return b ? blockDetail(b) : notFound(reply, 'block');
  });

  app.get('/api/v1/block/:id/raw', async (req, reply) => {
    const b = findBlock((req.params as { id: string }).id);
    if (!b) return notFound(reply, 'block');
    const raw = gunzipSync(q.rawBlock.get(b.height) as Buffer);
    reply.header('content-type', 'application/json; charset=utf-8');
    reply.header('content-disposition', `attachment; filename="genoryn-${config.network}-block-${b.height}.json"`);
    return reply.send(raw);
  });

  app.get('/api/v1/txs', async (req) => {
    const qs = req.query as Record<string, string>;
    const limit = clampInt(qs.limit, 25, 1, 100);
    const offset = clampInt(qs.offset, 0, 0, 1_000_000);
    const rows = qs.kind === 'transfer' ? (q.latestTransfers.all(limit) as TxRow[]) : (q.latestTxs.all(limit, offset) as TxRow[]);
    return { txs: rows.map(toTxSummary) };
  });

  app.get('/api/v1/tx/:txid', async (req, reply) => {
    const id = (req.params as { txid: string }).txid.toLowerCase();
    if (!isHex64(id)) return reply.code(400).send({ error: 'transaction id must be 64 hex characters' });
    const t = q.tx.get(id) as TxRow | undefined;
    return t ? txDetail(t) : notFound(reply, 'transaction');
  });

  app.get('/api/v1/tx/:txid/raw', async (req, reply) => {
    const id = (req.params as { txid: string }).txid.toLowerCase();
    const t = isHex64(id) ? (q.tx.get(id) as TxRow | undefined) : undefined;
    if (!t) return notFound(reply, 'transaction');
    const block = parseBlocksJson(`[${gunzipSync(q.rawBlock.get(t.height) as Buffer).toString('utf8')}]`)[0]!;
    const all = [block.body.miner, ...(block.body.treasury ? [block.body.treasury] : []), ...block.body.transactions];
    reply.header('content-disposition', `attachment; filename="genoryn-${config.network}-tx-${id.slice(0, 16)}.json"`);
    return { txid: id, height: t.height, index: t.idx, transaction: all[t.idx] };
  });

  app.get('/api/v1/nullifier/:hex', async (req, reply) => {
    const nf = (req.params as { hex: string }).hex.toLowerCase();
    if (!isHex64(nf)) return reply.code(400).send({ error: 'nullifier must be 64 hex characters' });
    const r = q.byNullifier.get(nf) as { txid: string; height: number; idx: number } | undefined;
    return { nullifier: nf, spent: !!r, txid: r?.txid ?? null, height: r?.height ?? null, action: r?.idx ?? null, note: 'A revealed nullifier proves a note was spent; it does not reveal which note, its amount or owner.' };
  });

  app.get('/api/v1/commitment/:hex', async (req, reply) => {
    const cmx = (req.params as { hex: string }).hex.toLowerCase();
    if (!isHex64(cmx)) return reply.code(400).send({ error: 'commitment must be 64 hex characters' });
    const r = q.byCmx.get(cmx) as { txid: string; height: number; idx: number } | undefined;
    return { commitment: cmx, found: !!r, txid: r?.txid ?? null, height: r?.height ?? null, action: r?.idx ?? null, confirmations: r ? tipHeight() - r.height + 1 : null };
  });

  app.get('/api/v1/search', async (req): Promise<SearchResult> => {
    const raw = String((req.query as Record<string, string>).q ?? '').trim();
    const query = raw.slice(0, 200);
    if (!query) return { type: 'none', query, reason: 'empty query' };
    if (/^\d{1,12}$/.test(query.replace(/,/g, ''))) {
      const h = Number(query.replace(/,/g, ''));
      const hash = q.hashAt.get(h) as string | undefined;
      return hash ? { type: 'block', height: h, hash } : { type: 'none', query, reason: `block ${h} has not been mined yet (tip ${tipHeight()})` };
    }
    if (query.startsWith('@') || (/^[a-z][a-z0-9_]{2,31}$/i.test(query) && !/^gnr[dt]?1/i.test(query))) {
      const c = canonicalUsername(query);
      if (c.ok && (query.startsWith('@') || !/^[0-9a-f]+$/i.test(query))) return { type: 'username', username: c.name };
    }
    if (/^gnr[dt]?1[0-9a-z]{20,}$/i.test(query)) {
      const v = validateAddress(query);
      return { type: 'address', address: query, valid: v.valid, network: v.network, error: v.error };
    }
    if (isHex64(query)) {
      const h = query.toLowerCase();
      const b = q.blockByHash.get(h) as BlockRow | undefined;
      if (b) return { type: 'block', height: b.height, hash: b.hash };
      const t = q.tx.get(h) as TxRow | undefined;
      if (t) return { type: 'tx', txid: t.txid, height: t.height };
      const nf = q.byNullifier.get(h) as { txid: string; height: number } | undefined;
      if (nf) return { type: 'nullifier', nullifier: h, txid: nf.txid, height: nf.height };
      const cm = q.byCmx.get(h) as { txid: string; height: number } | undefined;
      if (cm) return { type: 'commitment', cmx: h, txid: cm.txid, height: cm.height };
      return { type: 'none', query, reason: 'no block, transaction, nullifier or note commitment with this hash' };
    }
    return { type: 'none', query, reason: 'enter a block height, block hash, transaction id, nullifier, note commitment, address or @username' };
  });

  app.get('/api/v1/supply', async (req, reply) => {
    try {
      return await rpc.supply();
    } catch {
      reply.header('x-source', 'computed');
      return { ...supply(tipHeight()), stale: true, last_updated: indexer.lastOkAt };
    }
  });

  app.get('/api/v1/emission/:height', async (req, reply) => {
    const h = clampInt((req.params as { height: string }).height, -1, -1, Number.MAX_SAFE_INTEGER);
    if (h < 0) return reply.code(400).send({ error: 'height must be a nonnegative integer' });
    return supply(h);
  });

  app.get('/api/v1/emission-curve', async () =>
    emissionCurve(40, 1).map((p) => ({ year: p.year, height: p.height, issued: p.issued.toString(), reward: p.reward.toString() })),
  );

  app.get('/api/v1/charts/:metric', async (req, reply) => {
    const metric = (req.params as { metric: string }).metric;
    if (!['difficulty', 'blocktime', 'txs', 'fees', 'size', 'actions'].includes(metric)) return reply.code(400).send({ error: 'unknown metric' });
    const limit = clampInt((req.query as Record<string, string>).limit, 240, 10, 5000);
    return { metric, points: q.chartSeries(limit, metric as 'difficulty') };
  });

  app.get('/api/v1/genesis', async (req, reply) => {
    try {
      return await rpc.genesis();
    } catch {
      return reply.code(503).send({ error: 'node unavailable' });
    }
  });

  // Peer lists can grow without bound: summarize server-side and page the list.
  let peerCache: { at: number; peers: Array<{ address: string; inbound: boolean; height: number; onion: boolean }> } | null = null;
  const loadPeers = async () => {
    if (peerCache && Date.now() - peerCache.at < 5000) return peerCache.peers;
    const raw = await rpc.peers();
    const peers = raw.map((p) => {
      const onion = p.network === 'onion' || /onion|tor/i.test(String(p.link ?? '')) || /\.onion(:\d+)?$/i.test(p.address);
      return { address: onion ? 'onion (Tor)' : maskIp(p.address), inbound: p.inbound, height: p.height, onion };
    });
    peers.sort((a, b) => b.height - a.height || Number(a.inbound) - Number(b.inbound));
    peerCache = { at: Date.now(), peers };
    return peers;
  };
  app.get('/api/v1/peers', async (req) => {
    const qs = req.query as Record<string, string>;
    const limit = clampInt(qs.limit, 50, 1, 200);
    const offset = clampInt(qs.offset, 0, 0, Number.MAX_SAFE_INTEGER);
    const dir = qs.direction === 'inbound' || qs.direction === 'outbound' ? qs.direction : 'all';
    try {
      const all = await loadPeers();
      const tip = tipHeight();
      const filtered = dir === 'all' ? all : all.filter((p) => (dir === 'inbound' ? p.inbound : !p.inbound));
      let synced = 0;
      let onion = 0;
      let maxHeight = 0;
      for (const p of all) {
        if (p.height >= tip - 100) synced++;
        if (p.onion) onion++;
        if (p.height > maxHeight) maxHeight = p.height;
      }
      return {
        count: all.length,
        inbound: all.filter((p) => p.inbound).length,
        outbound: all.filter((p) => !p.inbound).length,
        synced,
        behind: all.length - synced,
        onion,
        max_height: maxHeight,
        tip,
        direction: dir,
        total: filtered.length,
        offset,
        limit,
        peers: filtered.slice(offset, offset + limit),
      };
    } catch (e) {
      return {
        count: 0, inbound: 0, outbound: 0, synced: 0, behind: 0, onion: 0, max_height: 0, tip: tipHeight(), direction: dir, total: 0, offset, limit, peers: [],
        unavailable: true,
        stale: true,
        last_updated: indexer.lastOkAt,
        error: `node unreachable: ${(e as Error).message}`,
      };
    }
  });

  app.get('/api/v1/network-size', async () => indexer.stats());

  app.get('/api/v1/seed', async () => ({
    clearnet: config.seedClearnet,
    onion: config.seedOnion,
    network_mode: config.networkMode,
    light_wallet_source: config.seedOnion ? `http://${config.seedOnion.replace(/:\d+$/, '')}:${NETWORKS.mainnet.publicRpcPort}` : null,
    p2p_onion: config.seedOnion,
    seed_lists: [`${REPO_URL}/blob/main/${MAINNET_SEEDS_FILE}`, `${REPO_URL}/blob/main/${MAINNET_ONION_SEEDS_FILE}`],
    note: config.seedOnion
      ? 'Seeds only introduce peers; every block is validated independently.'
      : 'Seeds only introduce peers; every block is validated independently. The genoryn.io onion address is published at launch.',
  }));

  app.get('/api/v1/mining', async () => ({ miner: miner.stats, network: indexer.stats() }));

  const nameLookup = async (raw: string, reply: FastifyReply) => {
    const c = canonicalUsername(raw);
    if (!c.ok) return reply.code(400).send({ error: c.error });
    try {
      return await rpc.name(c.name);
    } catch (e) {
      if (e instanceof RpcError && e.status < 500) return reply.code(400).send({ error: e.message });
      return reply.code(503).send({ error: 'node unavailable' });
    }
  };
  app.get('/api/v1/names/:name', async (req, reply) => nameLookup((req.params as { name: string }).name, reply));
  app.get('/api/v1/username/:name', async (req, reply) => nameLookup((req.params as { name: string }).name, reply));

  app.get('/api/v1/names/owner/:owner', async (req, reply) => {
    const owner = (req.params as { owner: string }).owner.toLowerCase();
    if (!isHex64(owner)) return reply.code(400).send({ error: 'owner must be 64 hex characters (Ed25519 public key)' });
    try {
      return await rpc.namesByOwner(owner);
    } catch {
      return reply.code(503).send({ error: 'node unavailable' });
    }
  });

  app.get('/api/v1/address/:address', async (req) => {
    const a = (req.params as { address: string }).address;
    return { address: a, ...validateAddress(a), balance: null, note: 'GENORYN has no public balance or history for an address. Every output is a shielded note that only its owner can decrypt.' };
  });

  app.get('/api/v1/releases', async () => getReleases());

  app.get('/api/v1/export/blocks.csv', async (req, reply) => {
    const qs = req.query as Record<string, string>;
    const tip = tipHeight();
    const to = clampInt(qs.to, tip, 1, tip);
    const from = clampInt(qs.from, Math.max(1, to - 999), 1, to);
    if (to - from > 9999) return reply.code(400).send({ error: 'at most 10,000 blocks per export' });
    const rows = (q.latestBlocks.all(to, to - from + 1) as BlockRow[]).reverse();
    const head = 'height,hash,previous,timestamp_utc,difficulty,nonce,size_bytes,transactions,transfers,actions,fees_gori,miner_reward_gori,treasury_reward_gori,solve_time_s';
    const lines = rows.map((r) =>
      [r.height, r.hash, r.previous, new Date(r.timestamp * 1000).toISOString(), r.difficulty, r.nonce, r.size, r.tx_count, r.transfer_count, r.action_count, r.fees, r.miner_reward, r.treasury_reward, r.solve_time ?? ''].join(','),
    );
    reply.header('content-type', 'text/csv; charset=utf-8');
    reply.header('content-disposition', `attachment; filename="genoryn-${config.network}-blocks-${from}-${to}.csv"`);
    return [head, ...lines].join('\n') + '\n';
  });

  app.get('/api/v1/export/txs.csv', async (req, reply) => {
    const qs = req.query as Record<string, string>;
    const tip = tipHeight();
    const to = clampInt(qs.to, tip, 1, tip);
    const from = clampInt(qs.from, Math.max(1, to - 999), 1, to);
    if (to - from > 9999) return reply.code(400).send({ error: 'at most 10,000 blocks per export' });
    const rows: TxRow[] = [];
    for (let h = from; h <= to; h++) rows.push(...(q.txsInBlock.all(h) as TxRow[]));
    const head = 'txid,height,index,kind,kind_label,fee_gori,value_balance_gori,actions,size_bytes,timestamp_utc';
    const lines = rows.map((r) => [r.txid, r.height, r.idx, r.kind, TX_KINDS[r.kind], r.fee, r.value_balance, r.actions, r.size, new Date((r.timestamp ?? 0) * 1000).toISOString()].join(','));
    reply.header('content-type', 'text/csv; charset=utf-8');
    reply.header('content-disposition', `attachment; filename="genoryn-${config.network}-txs-${from}-${to}.csv"`);
    return [head, ...lines].join('\n') + '\n';
  });
}
