import { createResource, createSignal, For, onCleanup, onMount, Show } from 'solid-js';
import { NETWORKS, formatBytes, formatHashrate, formatNumber, timeAgo, utc } from '@genoryn/shared';
import { LiveDot, PageHero, StaleNote, Stat } from '~/components/ui';
import { BarChart } from '~/components/Chart';
import { api } from '~/lib/api';
import { blocks, connected, miner, now, rawStats, stats } from '~/lib/live';

const SEED_ROW = `Public mainnet seed (P2P ${NETWORKS.mainnet.p2pPort})`;

interface Health {
  ok: boolean;
  node_online: boolean;
  stale?: boolean;
  last_updated?: number | null;
  error?: string | null;
  last_error: string | null;
  hash_self_check: boolean | null;
  sse_clients: number;
  indexed: number;
}
interface PeersPage {
  count: number;
  inbound: number;
  outbound: number;
  synced: number;
  behind: number;
  onion: number;
  max_height: number;
  tip: number;
  direction: 'all' | 'inbound' | 'outbound';
  total: number;
  offset: number;
  limit: number;
  peers: Array<{ address: string; inbound: boolean; height: number; onion: boolean }>;
  unavailable?: boolean;
  stale?: boolean;
  error?: string;
}

const PAGE = 50;
type Row = { name: string; detail: string; state: 'ok' | 'bad' | 'pending'; label: string };

export default function Status() {
  const [health, { refetch }] = createResource(() => api<Health>('/health'));
  const [direction, setDirection] = createSignal<'all' | 'inbound' | 'outbound'>('all');
  const [offset, setOffset] = createSignal(0);
  const [peers, { refetch: refetchPeers }] = createResource(
    () => [direction(), offset()] as const,
    ([d, o]) => api<PeersPage>(`/peers?direction=${d}&offset=${o}&limit=${PAGE}`),
  );
  onMount(() => {
    const t = setInterval(() => {
      void refetch();
      void refetchPeers();
    }, 15_000);
    onCleanup(() => clearInterval(t));
  });

  // Charts and figures use the filtered view; health rows use the raw one so an unreachable node is reported, not hidden.
  const s = () => stats();
  const raw = () => rawStats();
  const p = () => (peers.latest && !peers.latest.unavailable ? peers.latest : undefined);
  const nodeDown = () => !!raw()?.stale;
  const lag = () => (s() ? s()!.height - s()!.indexed_height : 0);
  const staleBlock = () => (s()?.last_block_time ? now() - s()!.last_block_time! > 900 : false);
  const ready = () => !!raw() && !!health();
  const heightText = () => (s() ? formatNumber(s()!.height) : '—');

  const rows = (): Row[] => {
    if (!ready())
      return ['Full node (genorynd)', 'Indexer', SEED_ROW, 'CPU miner', 'Live stream (SSE)', 'Block production'].map((name) => ({ name, detail: 'checking…', state: 'pending', label: 'checking' }));
    const st = (ok: boolean): Row['state'] => (ok ? 'ok' : 'bad');
    const r = raw()!;
    return [
      {
        name: 'Full node (genorynd)',
        detail: nodeDown() ? `${r.node_version} · last known height ${heightText()} · ${r.last_updated ? `last answered ${timeAgo(r.last_updated, now())}` : 'has not answered yet'}` : `${r.node_version} · height ${heightText()}`,
        state: st(!nodeDown()),
        label: nodeDown() ? 'unreachable' : 'online',
      },
      { name: 'Indexer', detail: `indexed ${formatNumber(health()!.indexed)} · hash self-check ${health()!.hash_self_check === null ? 'pending' : health()!.hash_self_check ? 'matches node' : 'MISMATCH'}`, state: st(!nodeDown() && lag() <= 2 && health()!.hash_self_check !== false), label: nodeDown() ? 'paused (node unreachable)' : lag() > 2 ? `syncing (${formatNumber(lag())} behind)` : 'in sync' },
      nodeDown() && !p()
        ? { name: SEED_ROW, detail: 'unknown while the node is unreachable', state: 'bad', label: 'unknown' }
        : { name: SEED_ROW, detail: p() ? `${formatNumber(p()!.count)} peers · ${formatNumber(p()!.inbound)} inbound · ${formatNumber(p()!.outbound)} outbound` : `${formatNumber(r.peers)} peers`, state: st((p()?.count ?? r.peers) > 0), label: (p()?.count ?? r.peers) > 0 ? 'connected' : 'no peers' },
      { name: 'CPU miner', detail: miner()?.online ? `${formatHashrate(miner()!.hashrate)} · ${miner()!.threads} threads · ${miner()!.accepted ?? 0} blocks this session` : 'no recent status line', state: miner() ? st(!!miner()!.online) : 'pending', label: miner()?.online ? 'mining' : miner() ? 'idle' : 'checking' },
      { name: 'Live stream (SSE)', detail: `${formatNumber(health()!.sse_clients)} connected viewers`, state: st(connected()), label: connected() ? 'connected' : 'reconnecting' },
      { name: 'Block production', detail: s()?.last_block_time ? `last block ${timeAgo(s()!.last_block_time!, now())}` : '—', state: nodeDown() ? 'bad' : st(!staleBlock()), label: nodeDown() ? 'unknown' : staleBlock() ? 'slow' : 'normal' },
    ];
  };
  const overall = () => (!ready() ? 'pending' : rows().every((r) => r.state === 'ok') ? 'ok' : 'bad');
  const solve = () => blocks().slice(0, 40).reverse().map((b) => ({ x: b.height, y: Math.max(0, b.solve_time ?? 0), label: `#${b.height}` }));
  const pct = (a: number, b: number) => (b ? `${((a / b) * 100).toFixed(1)}%` : '—');
  const pageEnd = () => Math.min((p()?.offset ?? 0) + PAGE, p()?.total ?? 0);

  return (
    <>
      <PageHero eyebrow="status.genoryn.org" title={<>Status</>} lead="Live health of the genoryn.io mainnet node, indexer, miner and peers as seen from this server." />
      <section class="section tight">
        <div class="container">
          <div class="card" style={{ padding: 0, overflow: 'hidden' }}>
            <div class="status-row" style={{ background: 'var(--panel-2)' }}>
              <LiveDot on={overall() === 'ok'} err={overall() === 'bad'} />
              <div>
                <div class="name" style={{ 'font-size': '18px' }}>
                  {overall() === 'pending' ? 'Checking systems…' : overall() === 'ok' ? 'All systems operational' : 'Some systems degraded'}
                </div>
                <div class="detail">
                  {!raw() ? 'Connecting to the genoryn.io node…' : nodeDown() ? `${raw()!.last_updated ? `Last updated ${utc(raw()!.last_updated!)}` : 'No node data yet'} · node unreachable` : `Updated ${utc(raw()!.updated_at)}`}
                </div>
              </div>
            </div>
            <For each={rows()}>
              {(r) => (
                <div class="status-row">
                  <LiveDot on={r.state === 'ok'} err={r.state === 'bad'} />
                  <div>
                    <div class="name">{r.name}</div>
                    <div class="detail">{r.detail}</div>
                  </div>
                  <span class="state">{r.label}</span>
                </div>
              )}
            </For>
          </div>
          <Show when={health()?.last_error}>
            <p class="error-text mt-2">Last indexer error: {health()!.last_error}</p>
          </Show>
          <StaleNote class="mt-3" />

          <div class="stat-strip mt-4">
            <Stat label="Chain ID" value={<span style={{ 'font-size': '16px' }}>{raw()?.chain_id ?? '—'}</span>} />
            <Stat label="Height" value={heightText()} sub={nodeDown() && s() ? 'last known' : undefined} />
            <Stat label="Network hashrate" value={s() ? formatHashrate(s()!.hashrate) : '—'} />
            <Stat label="Connected now · seeds" value={s() ? formatNumber(s()!.nodes_connected) : '—'} sub={s() ? Object.entries(s()!.per_seed ?? {}).map(([k, v]) => `${k}: ${v < 0 ? 'offline' : formatNumber(v)}`).join(' · ') : undefined} />
            <Stat label="Est. miners" value={s() ? `≈ ${formatNumber(s()!.est_miners)}` : '—'} sub="estimate · network hashrate ÷ ~200 H/s per CPU" />
            <Stat label="Reachable nodes" value={s() ? formatNumber(s()!.reachable_24h) : '—'} sub={s() ? `crawler · ${formatNumber(s()!.crawl_addresses)} addresses known` : 'network crawler'} />
            <Stat label="Mempool" value={s() ? formatNumber(s()!.mempool) : '—'} />
            <Stat label="Node database" value={formatBytes(s()?.database_bytes)} />
          </div>

          <div class="card mt-3">
            <div class="panel-title">
              <h3>Block solve times · last 40 blocks</h3>
              <span class="badge">target 60 s</span>
            </div>
            <BarChart points={solve()} height={180} format={(v) => `${v}s`} />
          </div>

          <div class="mt-4">
            <div class="panel-title">
              <h3>Peers of the genoryn.io node</h3>
              <span class="muted" style={{ 'font-size': '13px' }}>Addresses are masked; onion peers are never shown</span>
            </div>
            <div class="stat-strip">
              <Stat label="Connected" value={p() ? formatNumber(p()!.count) : '—'} sub={p() ? `${formatNumber(p()!.inbound)} in · ${formatNumber(p()!.outbound)} out` : undefined} />
              <Stat label="Synced" value={p() ? formatNumber(p()!.synced) : '—'} sub={p() ? `${pct(p()!.synced, p()!.count)} of peers` : undefined} />
              <Stat label="Still syncing" value={p() ? formatNumber(p()!.behind) : '—'} sub="new nodes, 100+ blocks behind" />
              <Stat label="Over Tor" value={p() ? formatNumber(p()!.onion) : '—'} sub="onion peers" />
              <Stat label="Highest reported" value={p() ? formatNumber(p()!.max_height) : '—'} sub={p() ? `our tip ${formatNumber(p()!.tip)}` : undefined} />
            </div>

            <div class="row between mt-3">
              <div class="tabs" role="tablist">
                <For each={['all', 'outbound', 'inbound'] as const}>
                  {(d) => (
                    <button
                      type="button"
                      class={direction() === d ? 'active' : ''}
                      onClick={() => {
                        setDirection(d);
                        setOffset(0);
                      }}
                    >
                      {d === 'all' ? 'All' : d === 'inbound' ? 'Inbound' : 'Outbound'}
                    </button>
                  )}
                </For>
              </div>
              <span class="muted mono" style={{ 'font-size': '12px' }}>
                <Show when={p() && p()!.total > 0} fallback="—">
                  {formatNumber((p()!.offset ?? 0) + 1)}–{formatNumber(pageEnd())} of {formatNumber(p()!.total)}
                </Show>
              </span>
            </div>
            <div class="table-wrap mt-2">
              <table class="data">
                <thead>
                  <tr>
                    <th>Peer</th>
                    <th>Direction</th>
                    <th class="num">Last reported height</th>
                    <th class="num">State</th>
                  </tr>
                </thead>
                <tbody>
                  <For each={p()?.peers ?? []} fallback={<tr><td colspan="4" class="center muted" style={{ padding: '30px' }}>{peers.loading ? 'Loading…' : peers.latest?.unavailable ? 'Peer list unavailable · node unreachable' : 'No peers'}</td></tr>}>
                    {(x) => (
                      <tr>
                        <td class="mono">{x.address}</td>
                        <td>
                          <span class="badge">{x.inbound ? 'inbound' : 'outbound'}</span>
                        </td>
                        <td class="num mono">{formatNumber(x.height)}</td>
                        <td class="num mono muted">{p() && x.height >= p()!.tip - 100 ? 'synced' : p() ? `syncing (${formatNumber(x.height)} / ${formatNumber(p()!.tip)})` : ''}</td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </div>
            <div class="row between mt-2">
              <button class="btn sm" type="button" disabled={(p()?.offset ?? 0) === 0} onClick={() => setOffset(Math.max(0, offset() - PAGE))}>
                ← Previous
              </button>
              <button class="btn sm" type="button" disabled={!p() || pageEnd() >= p()!.total} onClick={() => setOffset(offset() + PAGE)}>
                Next →
              </button>
            </div>
            <p class="hint mt-2">
              <b>How big is the whole network?</b> GENORYN has no central registry, so nobody can list every node. Two numbers bound it: <b>reachable nodes</b> (found by a crawler that walks the network through the peer protocol — only nodes with free inbound slots answer), and <b>estimated miners</b> (network hashrate divided by a typical light-mode CPU). The real network is at least as large as the largest of them.
            </p>
            <p class="hint mt-2">
              This list shows the connections of this one node (at most 32 at a time — the protocol limit per node). "Last reported height" is what a peer told us when it connected or last announced a block; connected peers keep receiving every new block from us, so a slightly older number does not mean they are behind. The whole network is far larger: GENORYN has no central registry of nodes, and peers reached through Tor are counted but never listed.
            </p>
          </div>
        </div>
      </section>
    </>
  );
}
