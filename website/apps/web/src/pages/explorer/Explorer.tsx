import { createResource, createSignal, For, onCleanup, onMount, Show } from 'solid-js';
import { A } from '@solidjs/router';
import { formatBytes, formatHashrate, formatNumber, timeAgo, type ChartPoint, type TxSummary } from '@genoryn/shared';
import { Icon } from '~/components/Icon';
import { LineChart, BarChart } from '~/components/Chart';
import { Callout, Gnr, StaleNote, Stat } from '~/components/ui';
import { api, apiUrl } from '~/lib/api';
import { blocks, connected, now, onNewBlock, stats } from '~/lib/live';
import { BlocksTable, ExplorerHeader, TxTable } from './common';

const METRICS = [
  { id: 'difficulty', label: 'Difficulty', kind: 'line' },
  { id: 'blocktime', label: 'Block time', kind: 'bar' },
  { id: 'actions', label: 'Actions', kind: 'bar' },
  { id: 'size', label: 'Block size', kind: 'line' },
] as const;

export default function Explorer() {
  const [metric, setMetric] = createSignal<(typeof METRICS)[number]['id']>('difficulty');
  const [tick, setTick] = createSignal(0);
  const [chart] = createResource(
    () => [metric(), tick()] as const,
    ([m]) => api<{ points: ChartPoint[] }>(`/charts/${m}?limit=180`).then((r) => r.points),
  );
  const [txs, { refetch: refetchTxs }] = createResource(() => api<{ txs: TxSummary[] }>('/txs?limit=12').then((r) => r.txs));
  const [transfers, { refetch: refetchTransfers }] = createResource(() => api<{ txs: TxSummary[] }>('/txs?kind=transfer&limit=8').then((r) => r.txs));

  onMount(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const off = onNewBlock(() => {
      clearTimeout(timer);
      timer = setTimeout(() => {
        setTick((t) => t + 1);
        void refetchTxs();
        void refetchTransfers();
      }, 400);
    });
    onCleanup(() => {
      off();
      clearTimeout(timer);
    });
  });

  const s = () => stats();
  const [from, setFrom] = createSignal('');
  const [to, setTo] = createSignal('');
  const current = () => METRICS.find((m) => m.id === metric())!;
  const points = () => (chart() ?? []).map((p) => ({ x: p.height, y: p.value, label: `#${p.height}` }));
  const fmt = (v: number) => (metric() === 'size' ? formatBytes(v) : metric() === 'blocktime' ? `${v}s` : formatNumber(Math.round(v)));

  return (
    <>
      <ExplorerHeader title="Explorer" sub="Every public fact about the GENORYN chain — blocks, shielded transactions, nullifiers and note commitments — streamed live from a validating full node." />
      <section class="section tight">
        <div class="container wide">
          <StaleNote class="mb-2" />
          <div class="explorer-top">
            <Stat label="Height" live={connected() && !!s() && !s()!.stale} value={s() ? formatNumber(s()!.height) : '—'} sub={s()?.last_block_time ? timeAgo(s()!.last_block_time!, now()) : '—'} />
            <Stat label="Difficulty" value={s() ? formatNumber(s()!.difficulty) : '—'} sub={s() ? `≈ ${formatHashrate(s()!.hashrate)}` : '—'} />
            <Stat label="Avg block" value={s()?.avg_block_time ? `${s()!.avg_block_time!.toFixed(1)}s` : '—'} sub="target 60 s" />
            <Stat label="Issued" value={<Gnr gori={s()?.issued_supply} digits={0} />} sub="of 21,000,000" />
            <Stat label="Transfers" value={s() ? formatNumber(s()!.total_transfers) : '—'} sub={s() ? `${formatNumber(s()!.total_actions)} actions` : '—'} />
            <Stat label="Fees paid" value={<Gnr gori={s()?.total_fees} digits={8} />} sub={`mempool ${s() ? s()!.mempool : '—'}`} />
          </div>

          <div class="card mt-3" style={{ padding: '22px' }}>
            <div class="panel-title">
              <h2>Chain</h2>
              <span class="muted mono" style={{ 'font-size': '12px' }}>
                tip {s()?.best_hash ? `${s()!.best_hash.slice(0, 16)}…` : '—'}
              </span>
            </div>
            <div class="chain-viz">
              <div class="chain-block pending">
                <div class="h">#{s() ? formatNumber(s()!.height + 1) : '…'}</div>
                <div class="m">being mined<br />{s() ? s()!.mempool : '—'} pending tx</div>
              </div>
              <For each={blocks().slice(0, 12)}>
                {(b, i) => (
                  <A href={`/explorer/block/${b.height}`} class={`chain-block ${i() === 0 ? 'new' : ''}`}>
                    <div class="h">#{formatNumber(b.height)}</div>
                    <div class="m">
                      {b.tx_count} tx · {(b.size / 1024).toFixed(1)} KiB
                      <br />
                      {b.solve_time === null ? '—' : `${b.solve_time}s`} · {timeAgo(b.timestamp, now())}
                    </div>
                  </A>
                )}
              </For>
            </div>
          </div>

          <div class="card mt-3" style={{ padding: '22px' }}>
            <div class="panel-title">
              <h2>{current().label} · last 180 blocks</h2>
              <div class="tabs" role="tablist">
                <For each={METRICS}>
                  {(m) => (
                    <button type="button" role="tab" aria-selected={metric() === m.id} class={metric() === m.id ? 'active' : ''} onClick={() => setMetric(m.id)}>
                      {m.label}
                    </button>
                  )}
                </For>
              </div>
            </div>
            <Show when={current().kind === 'line'} fallback={<BarChart points={points()} height={240} format={fmt} />}>
              <LineChart points={points()} height={240} format={fmt} />
            </Show>
          </div>

          <div class="mt-4">
            <div class="panel-title">
              <h2>Latest blocks</h2>
              <A href="/explorer/blocks" class="btn sm">
                All blocks <Icon name="arrow" size={14} />
              </A>
            </div>
            <BlocksTable blocks={blocks().slice(0, 15)} animate />
          </div>

          <div class="grid cols-2 mt-4" style={{ 'align-items': 'start' }}>
            <div>
              <div class="panel-title">
                <h2>Shielded transfers</h2>
                <span class="badge">kind 0</span>
              </div>
              <TxTable txs={transfers() ?? []} />
            </div>
            <div>
              <div class="panel-title">
                <h2>All transactions</h2>
                <span class="badge">incl. issuance</span>
              </div>
              <TxTable txs={txs() ?? []} />
            </div>
          </div>

          <div class="grid cols-2 mt-4">
            <div class="card">
              <h3>What the explorer can see</h3>
              <p class="muted">Block headers, timestamps, difficulty, nonces, miner and treasury issuance, public fees, transaction IDs, the number of actions, nullifiers (proof that some note was spent), note commitments, anchors and proof sizes.</p>
              <h3 class="mt-3">What it can never see</h3>
              <p class="muted">Amounts, senders, recipients, memos, balances or the link between a spend and the note it consumed. Those are encrypted or hidden inside zero-knowledge proofs, and only the owner's wallet can decrypt its own notes.</p>
            </div>
            <div class="card">
              <h3>Export data</h3>
              <p class="muted">Download public chain data as CSV (up to 10,000 blocks per file) or use the free JSON API.</p>
              <div class="grid cols-2 mt-2">
                <div class="field">
                  <label for="ex-from">From height</label>
                  <input id="ex-from" class="input mono" inputmode="numeric" placeholder="1" value={from()} onInput={(e) => setFrom(e.currentTarget.value.replace(/\D/g, ''))} />
                </div>
                <div class="field">
                  <label for="ex-to">To height</label>
                  <input id="ex-to" class="input mono" inputmode="numeric" placeholder={s() ? String(s()!.height) : 'tip'} value={to()} onInput={(e) => setTo(e.currentTarget.value.replace(/\D/g, ''))} />
                </div>
              </div>
              <div class="row mt-2">
                <a class="btn sm" href={apiUrl(`/export/blocks.csv?${from() ? `from=${from()}&` : ''}${to() ? `to=${to()}` : ''}`)} download="">
                  <Icon name="download" size={14} /> Blocks CSV
                </a>
                <a class="btn sm" href={apiUrl(`/export/txs.csv?${from() ? `from=${from()}&` : ''}${to() ? `to=${to()}` : ''}`)} download="">
                  <Icon name="download" size={14} /> Transactions CSV
                </a>
                <A href="/api" class="btn sm ghost">
                  API docs <Icon name="arrow" size={14} />
                </A>
              </div>
              <Callout class="mt-3">
                <span style={{ 'font-size': '13.5px' }}>Looking for your own history? It lives only in your wallet. Open <b>Transactions</b> in the GENORYN wallet to search, filter and export payment proofs.</span>
              </Callout>
            </div>
          </div>
        </div>
      </section>
    </>
  );
}
