import { For, Show } from 'solid-js';
import { A } from '@solidjs/router';
import { formatNumber, type BlockSummary, type TxSummary } from '@genoryn/shared';
import { Gnr, Hash, TimeAgo } from '~/components/ui';
import { SearchBar } from '~/components/SearchBar';

export function ExplorerHeader(props: { title: string; eyebrow?: string; sub?: string }) {
  return (
    <header class="page-hero" style={{ padding: '56px 0 40px' }}>
      <div class="bg-grid" />
      <div class="container" style={{ position: 'relative' }}>
        <div class="eyebrow">{props.eyebrow ?? 'GENORYN Explorer · mainnet'}</div>
        <h1 style={{ 'font-size': 'clamp(32px, 4.4vw, 60px)' }}>{props.title}</h1>
        <Show when={props.sub}>
          <p class="lead">{props.sub}</p>
        </Show>
        <div class="mt-3" style={{ 'max-width': '820px' }}>
          <SearchBar big />
        </div>
      </div>
    </header>
  );
}

export function BlocksTable(props: { blocks: BlockSummary[]; animate?: boolean; tip?: number }) {
  return (
    <div class="table-wrap">
      <table class="data">
        <thead>
          <tr>
            <th>Height</th>
            <th>Hash</th>
            <th>Age</th>
            <th class="num">Tx</th>
            <th class="num">Actions</th>
            <th class="num">Size</th>
            <th class="num">Difficulty</th>
            <th class="num">Solve</th>
            <th class="num">Reward</th>
          </tr>
        </thead>
        <tbody>
          <For each={props.blocks} fallback={<tr><td colspan="9" class="center muted" style={{ padding: '40px' }}>Loading blocks…</td></tr>}>
            {(b, i) => (
              <tr class={props.animate && i() === 0 ? 'enter' : ''}>
                <td>
                  <A href={`/explorer/block/${b.height}`} class="link mono strong">
                    {formatNumber(b.height)}
                  </A>
                </td>
                <td>
                  <Hash value={b.hash} href={`/explorer/block/${b.height}`} head={8} tail={6} />
                </td>
                <td class="muted">
                  <TimeAgo ts={b.timestamp} />
                </td>
                <td class="num mono">{b.tx_count}</td>
                <td class="num mono">{b.action_count}</td>
                <td class="num mono">{(b.size / 1024).toFixed(1)} KiB</td>
                <td class="num mono">{formatNumber(b.difficulty)}</td>
                <td class="num mono muted">{b.solve_time === null ? '—' : `${b.solve_time}s`}</td>
                <td class="num">
                  <Gnr gori={b.miner_reward} digits={3} unit={false} />
                </td>
              </tr>
            )}
          </For>
        </tbody>
      </table>
    </div>
  );
}

export function TxTable(props: { txs: TxSummary[]; showBlock?: boolean }) {
  return (
    <div class="table-wrap">
      <table class="data">
        <thead>
          <tr>
            <th>Transaction ID</th>
            <th>Type</th>
            <Show when={props.showBlock !== false}>
              <th>Block</th>
            </Show>
            <th class="num">Actions</th>
            <th class="num">Size</th>
            <th class="num">Fee / Issuance</th>
            <th>Amount</th>
          </tr>
        </thead>
        <tbody>
          <For each={props.txs} fallback={<tr><td colspan="7" class="center muted" style={{ padding: '40px' }}>No transactions.</td></tr>}>
            {(t) => (
              <tr>
                <td>
                  <Hash value={t.txid} href={`/explorer/tx/${t.txid}`} head={10} tail={6} />
                </td>
                <td>
                  <span class={`badge ${t.kind === 0 ? 'solid' : ''}`}>{t.kind_label}</span>
                </td>
                <Show when={props.showBlock !== false}>
                  <td>
                    <A href={`/explorer/block/${t.height}`} class="link mono">
                      {formatNumber(t.height)}
                    </A>
                  </td>
                </Show>
                <td class="num mono">{t.actions}</td>
                <td class="num mono">{(t.size / 1024).toFixed(1)} KiB</td>
                <td class="num">
                  <Show when={t.kind === 0} fallback={<Gnr gori={(-BigInt(t.value_balance)).toString()} digits={4} />}>
                    <Gnr gori={t.fee} digits={6} />
                  </Show>
                </td>
                <td>
                  <Show when={t.kind === 0} fallback={<span class="muted mono" style={{ 'font-size': '12px' }}>public issuance</span>}>
                    <span class="privacy-mask" title="Shielded: amounts are encrypted">●●●●●● shielded</span>
                  </Show>
                </td>
              </tr>
            )}
          </For>
        </tbody>
      </table>
    </div>
  );
}

export function DetailNav(props: { prev?: string | null; next?: string | null }) {
  return (
    <div class="detail-nav">
      <Show when={props.prev} fallback={<span class="btn sm" style={{ opacity: 0.35 }}>← Prev</span>}>
        <A href={props.prev!} class="btn sm">
          ← Prev
        </A>
      </Show>
      <Show when={props.next} fallback={<span class="btn sm" style={{ opacity: 0.35 }}>Next →</span>}>
        <A href={props.next!} class="btn sm">
          Next →
        </A>
      </Show>
    </div>
  );
}

export function EncryptedField(props: { bytes: number; label?: string }) {
  return (
    <span class="encrypted">
      <span class="bars" aria-hidden="true">
        <For each={Array.from({ length: 12 })}>{() => <i />}</For>
      </span>
      {props.bytes} bytes {props.label ?? 'encrypted'}
    </span>
  );
}
