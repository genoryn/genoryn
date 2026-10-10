import { createResource, Match, Switch } from 'solid-js';
import { A, useParams } from '@solidjs/router';
import { formatNumber } from '@genoryn/shared';
import { Callout, Hash, Kv } from '~/components/ui';
import { api } from '~/lib/api';
import { stats } from '~/lib/live';
import { ExplorerHeader } from './common';

interface CmResult {
  commitment: string;
  found: boolean;
  txid: string | null;
  height: number | null;
  action: number | null;
  confirmations: number | null;
}

export default function CommitmentPage() {
  const params = useParams();
  const [r] = createResource(() => params.id, (id) => api<CmResult>(`/commitment/${encodeURIComponent(id)}`));
  return (
    <>
      <ExplorerHeader eyebrow="Note commitment" title="Note commitment" sub="A commitment (cmx) is the public fingerprint of a newly created shielded note. It joins the 32-level note tree once mature, and says nothing about value or owner." />
      <section class="section tight">
        <div class="container wide">
          <Switch>
            <Match when={r.error}>
              <div class="empty">{(r.error as Error).message}</div>
            </Match>
            <Match when={r()}>
              {(x) => (
                <>
                  <Kv
                    rows={[
                      ['Commitment', <Hash value={x().commitment} short={false} />],
                      ['Status', x().found ? <span class="badge solid">created on chain</span> : <span class="badge">not found</span>],
                      ['Transaction', x().txid ? <Hash value={x().txid!} href={`/explorer/tx/${x().txid}`} short={false} /> : '—'],
                      ['Block', x().height ? <A class="link" href={`/explorer/block/${x().height}`}>#{formatNumber(x().height!)}</A> : '—'],
                      ['Confirmations', x().height ? String(Math.max(x().confirmations ?? 0, (stats()?.height ?? 0) - x().height! + 1)) : '—'],
                      ['Action index', x().action === null ? '—' : String(x().action)],
                    ]}
                  />
                  <Callout class="mt-3">Mining outputs mature after 60 blocks (about an hour) on mainnet before they can be spent. Mainnet treasury outputs pay the provably unspendable burn address. Only recent tree roots (a 120-block window) may anchor new transfers.</Callout>
                </>
              )}
            </Match>
          </Switch>
        </div>
      </section>
    </>
  );
}
