import { createSignal } from 'solid-js';
import { isServer } from 'solid-js/web';
import type { BlockSummary, ChainStats, MinerStats } from '@genoryn/shared';
import { api } from './api';

const [rawStats, setRawStats] = createSignal<ChainStats | null>(null);
const [statsReceivedAt, setStatsReceivedAt] = createSignal(0);
const [miner, setMiner] = createSignal<MinerStats | null>(null);
const [blocks, setBlocks] = createSignal<BlockSummary[]>([]);
const [connected, setConnected] = createSignal(false);
const [lastBlockAt, setLastBlockAt] = createSignal<number>(0);
const [now, setNow] = createSignal(Math.floor(Date.now() / 1000));

/**
 * Chain stats that describe a real chain view. A server whose node never answered reports
 * height 0 with `stale: true`; that is unknown, not an empty chain, so it reads as null here.
 */
const stats = () => {
  const s = rawStats();
  return s && !(s.stale && s.height === 0) ? s : null;
};

export { rawStats, stats, statsReceivedAt, miner, blocks, connected, lastBlockAt, now };

function setStats(s: ChainStats) {
  setRawStats(s);
  setStatsReceivedAt(Math.floor(Date.now() / 1000));
}

type BlockListener = (b: BlockSummary) => void;
const listeners = new Set<BlockListener>();
export function onNewBlock(fn: BlockListener): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

let started = false;
let source: EventSource | null = null;

function pushBlock(b: BlockSummary) {
  setBlocks((prev) => {
    if (prev.some((x) => x.height === b.height && x.hash === b.hash)) return prev;
    const merged = [b, ...prev.filter((x) => x.height !== b.height)].sort((a, z) => z.height - a.height);
    return merged.slice(0, 40);
  });
  setLastBlockAt(Date.now());
  listeners.forEach((fn) => fn(b));
}

async function refreshBlocks() {
  try {
    const r = await api<{ blocks: BlockSummary[] }>('/blocks?limit=40');
    setBlocks(r.blocks);
  } catch {
    /* keep previous */
  }
}

function connect() {
  source?.close();
  source = new EventSource('/api/v1/live');
  source.addEventListener('open', () => setConnected(true));
  source.addEventListener('error', () => setConnected(false));
  source.addEventListener('stats', (e) => setStats(JSON.parse((e as MessageEvent).data) as ChainStats));
  source.addEventListener('miner', (e) => setMiner(JSON.parse((e as MessageEvent).data) as MinerStats));
  source.addEventListener('block', (e) => pushBlock(JSON.parse((e as MessageEvent).data) as BlockSummary));
  source.addEventListener('reorg', () => void refreshBlocks());
}

export function startLive() {
  if (isServer || started) return;
  started = true;
  void api<ChainStats>('/stats').then(setStats).catch(() => undefined);
  void api<{ miner: MinerStats }>('/mining').then((r) => setMiner(r.miner)).catch(() => undefined);
  void refreshBlocks();
  connect();
  setInterval(() => setNow(Math.floor(Date.now() / 1000)), 1000);
  document.addEventListener('visibilitychange', () => {
    if (document.visibilityState === 'visible' && (!source || source.readyState === EventSource.CLOSED)) {
      void refreshBlocks();
      connect();
    }
  });
}
