import { createResource, Match, Show, Switch } from 'solid-js';
import { A, useNavigate, useSearchParams } from '@solidjs/router';
import type { SearchResult } from '@genoryn/shared';
import { Callout } from '~/components/ui';
import { routeForResult } from '~/components/SearchBar';
import { api } from '~/lib/api';
import { ExplorerHeader } from './common';

export default function SearchPage() {
  const [params] = useSearchParams();
  const navigate = useNavigate();
  const q = () => (typeof params.q === 'string' ? params.q : '');
  const [r] = createResource(q, async (query) => {
    if (!query) return null;
    const res = await api<SearchResult>(`/search?q=${encodeURIComponent(query)}`);
    if (['block', 'tx', 'nullifier', 'commitment', 'username'].includes(res.type)) navigate(routeForResult(res), { replace: true });
    return res;
  });

  return (
    <>
      <ExplorerHeader eyebrow="Search" title="Search" />
      <section class="section tight">
        <div class="container narrow">
          <Show when={q()} fallback={<p class="muted">Enter a block height or hash, transaction ID, nullifier, note commitment, receiving address or @username.</p>}>
            <p class="muted mono">Query: {q()}</p>
            <Switch>
              <Match when={r.loading}>
                <div class="empty">Searching…</div>
              </Match>
              <Match when={r()?.type === 'address' && r()}>
                {(res) => {
                  const a = res() as Extract<SearchResult, { type: 'address' }>;
                  return (
                    <div class="stack">
                      <div class="card">
                        <div class="row">
                          <span class={`badge ${a.valid ? 'solid' : 'err'}`}>{a.valid ? `valid ${a.network} address` : 'invalid address'}</span>
                        </div>
                        <p class="mono break mt-2 strong">{a.address}</p>
                        <Show when={!a.valid}>
                          <p class="error-text">{a.error}</p>
                        </Show>
                      </div>
                      <Callout>
                        <b>There is no balance or history to show — by design.</b> A GENORYN receiving identity never appears on chain. Payments to it are encrypted shielded notes that only the owner's wallet can find, by scanning locally. No explorer, node or server can list its transactions or balance.
                      </Callout>
                      <div class="row">
                        <A href={`/pay?to=${encodeURIComponent(a.address)}`} class="btn sm">
                          Create a payment request
                        </A>
                        <A href="/learn#shielded-notes" class="btn sm ghost">
                          How shielded notes work
                        </A>
                      </div>
                    </div>
                  );
                }}
              </Match>
              <Match when={r()?.type === 'none' && r()}>
                {(res) => (
                  <div class="empty">
                    <h3>Nothing found</h3>
                    <p>{(res() as Extract<SearchResult, { type: 'none' }>).reason}.</p>
                  </div>
                )}
              </Match>
              <Match when={r.error}>
                <div class="empty">Search failed: {(r.error as Error).message}</div>
              </Match>
            </Switch>
          </Show>
        </div>
      </section>
    </>
  );
}
