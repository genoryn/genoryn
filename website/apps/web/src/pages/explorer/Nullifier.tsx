import { createResource, Match, Switch } from 'solid-js';
import { A, useParams } from '@solidjs/router';
import { formatNumber } from '@genoryn/shared';
import { Callout, Hash, Kv } from '~/components/ui';
import { api } from '~/lib/api';
import { ExplorerHeader } from './common';

interface NfResult {
  nullifier: string;
  spent: boolean;
  txid: string | null;
  height: number | null;
  action: number | null;
  note: string;
}

export default function NullifierPage() {
  const params = useParams();
  const [r] = createResource(() => params.id, (id) => api<NfResult>(`/nullifier/${encodeURIComponent(id)}`));
  return (
    <>
      <ExplorerHeader eyebrow="Nullifier" title="Nullifier" sub="A nullifier is a unique tag revealed when a note is spent. Seeing it twice would mean a double spend — consensus rejects that." />
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
                      ['Nullifier', <Hash value={x().nullifier} short={false} />],
                      ['Status', x().spent ? <span class="badge solid">revealed on chain</span> : <span class="badge">not seen on chain</span>],
                      ['Transaction', x().txid ? <Hash value={x().txid!} href={`/explorer/tx/${x().txid}`} short={false} /> : '—'],
                      ['Block', x().height ? <A class="link" href={`/explorer/block/${x().height}`}>#{formatNumber(x().height!)}</A> : '—'],
                      ['Action index', x().action === null ? '—' : String(x().action)],
                    ]}
                  />
                  <Callout class="mt-3">{x().note} Issuance outputs carry a dummy spend whose nullifier is also unique.</Callout>
                </>
              )}
            </Match>
          </Switch>
        </div>
      </section>
    </>
  );
}
