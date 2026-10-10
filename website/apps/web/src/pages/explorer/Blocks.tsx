import { createEffect, createResource, createSignal, on, onCleanup, onMount, Show } from 'solid-js';
import { A, useSearchParams } from '@solidjs/router';
import { formatNumber, type BlockSummary } from '@genoryn/shared';
import { Icon } from '~/components/Icon';
import { LiveDot, StaleNote } from '~/components/ui';
import { api } from '~/lib/api';
import { setTitle } from '~/lib/dom';
import { connected, onNewBlock } from '~/lib/live';
import { BlocksTable, ExplorerHeader } from './common';

const PAGE = 50;

interface Page {
  tip: number;
  blocks: BlockSummary[];
}

export default function Blocks() {
  const [params, setParams] = useSearchParams();
  // `before` is absent on the live first page; the resource key must never be undefined or it won't fetch.
  const key = () => (typeof params.before === 'string' && /^\d+$/.test(params.before) ? params.before : 'latest');
  const [data, { refetch, mutate }] = createResource(key, (k) => api<Page>(`/blocks?limit=${PAGE}${k === 'latest' ? '' : `&before=${k}`}`));
  const [jump, setJump] = createSignal('');
  const [fresh, setFresh] = createSignal<number | null>(null);
  const isLive = () => key() === 'latest';
  const first = () => data()?.blocks[0]?.height;
  const last = () => data()?.blocks.at(-1)?.height;

  onMount(() => {
    setTitle('All blocks — GENORYN Explorer');
    const off = onNewBlock((b) => {
      if (!isLive()) return;
      const cur = data();
      if (!cur || cur.blocks.some((x) => x.height === b.height && x.hash === b.hash)) return;
      mutate({ tip: Math.max(cur.tip, b.height), blocks: [b, ...cur.blocks.filter((x) => x.height !== b.height)].sort((a, z) => z.height - a.height).slice(0, PAGE) });
      setFresh(b.height);
    });
    // Safety net if the live stream is interrupted.
    const timer = setInterval(() => {
      if (isLive() && !connected()) void refetch();
    }, 30_000);
    onCleanup(() => {
      off();
      clearInterval(timer);
    });
  });

  createEffect(on(key, () => setFresh(null), { defer: true }));

  return (
    <>
      <ExplorerHeader title="All blocks" sub="Every block in the active GENORYN mainnet chain, newest first." />
      <section class="section tight">
        <div class="container wide">
          <StaleNote class="mb-2" />
          <div class="panel-title">
            <h2 class="row" style={{ gap: '10px' }}>
              <Show when={isLive()}>
                <LiveDot on={connected()} />
              </Show>
              <Show when={first() !== undefined} fallback="Blocks">
                Blocks {formatNumber(last()!)} – {formatNumber(first()!)}
              </Show>
              <Show when={isLive()}>
                <span class="badge">live</span>
              </Show>
            </h2>
            <form
              class="row"
              onSubmit={(e) => {
                e.preventDefault();
                const n = Number(jump());
                if (n > 0) setParams({ before: String(n) });
              }}
            >
              <input class="input mono" style={{ width: '160px', height: '36px' }} placeholder="Jump to height" inputmode="numeric" value={jump()} onInput={(e) => setJump(e.currentTarget.value.replace(/\D/g, ''))} aria-label="Jump to height" />
              <button class="btn sm" type="submit">
                Go
              </button>
            </form>
          </div>

          <Show when={!data.error} fallback={
            <div class="empty">
              <p>Could not load blocks: {(data.error as Error)?.message}</p>
              <button class="btn sm" type="button" onClick={() => void refetch()}>
                <Icon name="refresh" size={14} /> Retry
              </button>
            </div>
          }>
            <Show when={data() || !data.loading} fallback={<div class="empty">Loading blocks…</div>}>
              <Show when={(data()?.blocks.length ?? 0) > 0} fallback={<div class="empty">No blocks in this range.</div>}>
                <BlocksTable blocks={data()!.blocks} animate={fresh() !== null} />
              </Show>
            </Show>
          </Show>

          <div class="row between mt-3">
            <Show when={!isLive()} fallback={<span class="muted" style={{ 'font-size': '13px' }}>New blocks appear here automatically.</span>}>
              <A href="/explorer/blocks" class="btn sm">
                ↑ Newest (live)
              </A>
            </Show>
            <div class="row">
              <Show when={!isLive() && first() !== undefined && data() && first()! < data()!.tip}>
                <button class="btn sm" type="button" onClick={() => (first()! + PAGE >= data()!.tip ? setParams({ before: undefined }) : setParams({ before: String(first()! + PAGE) }))}>
                  ← Newer
                </button>
              </Show>
              <Show when={last() !== undefined && last()! > 1}>
                <button class="btn sm" type="button" onClick={() => setParams({ before: String(last()! - 1) })}>
                  Older →
                </button>
              </Show>
            </div>
          </div>
        </div>
      </section>
    </>
  );
}
