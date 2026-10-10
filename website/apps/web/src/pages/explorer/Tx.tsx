import { createEffect, createResource, For, Match, Show, Switch } from 'solid-js';
import { A, useParams } from '@solidjs/router';
import { formatNumber, shortHash, utc, type TxDetail } from '@genoryn/shared';
import { Icon } from '~/components/Icon';
import { Callout, Gnr, Hash, Kv, TimeAgo } from '~/components/ui';
import { api, apiUrl, ApiError } from '~/lib/api';
import { setTitle } from '~/lib/dom';
import { stats } from '~/lib/live';
import { EncryptedField, ExplorerHeader } from './common';

export default function TxPage() {
  const params = useParams();
  const [tx] = createResource(() => params.id, (id) => api<TxDetail>(`/tx/${encodeURIComponent(id)}`));
  createEffect(() => {
    const t = tx();
    if (t) setTitle(`Transaction ${shortHash(t.txid, 8, 6)} — GENORYN Explorer`);
  });
  const conf = () => (tx() ? Math.max(tx()!.confirmations, (stats()?.height ?? 0) - tx()!.height + 1) : 0);

  return (
    <>
      <ExplorerHeader eyebrow="Transaction" title={tx() ? tx()!.kind_label : 'Transaction'} />
      <section class="section tight">
        <div class="container wide">
          <Switch>
            <Match when={tx.error}>
              <div class="empty">
                <h3>{(tx.error as ApiError).status === 404 ? 'Transaction not found' : 'Could not load transaction'}</h3>
                <p>Unconfirmed transactions live only in node mempools and are not public. Once a block includes it, it appears here.</p>
                <A href="/explorer" class="btn sm">
                  Back to explorer
                </A>
              </div>
            </Match>
            <Match when={tx()}>
              {(t) => (
                <>
                  <div class="detail-head">
                    <div class="row">
                      <span class="badge solid">{conf()} confirmation{conf() === 1 ? "" : "s"}</span>
                      <span class="badge">kind {t().kind}</span>
                      <span class="badge">{t().actions} actions</span>
                    </div>
                    <a class="btn sm" href={apiUrl(`/tx/${t().txid}/raw`)} download="">
                      <Icon name="download" size={14} /> Raw JSON
                    </a>
                  </div>
                  <Kv
                    rows={[
                      ['Transaction ID', <Hash value={t().txid} short={false} />],
                      ['Type', t().kind_label],
                      ['Block', <><A href={`/explorer/block/${t().height}`} class="link">#{formatNumber(t().height)}</A> · index {t().index}</>],
                      ['Block hash', <Hash value={t().block_hash} href={`/explorer/block/${t().height}`} short={false} />],
                      ['Time', <>{utc(t().timestamp)} · <span class="muted"><TimeAgo ts={t().timestamp} /></span></>],
                      [t().kind === 0 ? 'Public fee' : 'Public issuance', t().kind === 0 ? <Gnr gori={t().fee} digits={12} /> : <Gnr gori={(-BigInt(t().value_balance)).toString()} digits={12} />],
                      ['Value balance', <span>{t().value_balance} gori <span class="muted">({t().kind === 0 ? 'positive = fee to miner' : 'negative = new issuance'})</span></span>],
                      ['Transferred amount', t().kind === 0 ? <span class="privacy-mask">●●●●●●●● shielded — known only to sender and recipient</span> : <span class="muted">issuance outputs pay a receiving address inside an encrypted note</span>],
                      ['Expiry height', t().expires === 0 ? 'none' : formatNumber(t().expires)],
                      ['Protocol', `v${t().protocol} · ${t().network}`],
                      ['Context nonce', <Hash value={t().nonce} short={false} />],
                      ['Anchor', <Hash value={t().anchor} short={false} />],
                      ['Bundle flags', `0x${t().flags.toString(16).padStart(2, '0')} (${t().flags & 1 ? 'spends enabled' : 'spends disabled'}, ${t().flags & 2 ? 'outputs enabled' : 'outputs disabled'})`],
                      ['Halo 2 proof', `${formatNumber(t().proof_bytes)} bytes`],
                      ['Binding signature', <Hash value={t().binding_signature} head={16} tail={16} />],
                      ['Encoded size', `${formatNumber(t().size)} bytes`],
                    ]}
                  />
                  <div class="panel-title mt-4">
                    <h2>Actions</h2>
                    <span class="muted" style={{ 'font-size': '14px' }}>Each action spends one note (nullifier) and creates one note (commitment).</span>
                  </div>
                  <For each={t().actions_detail}>
                    {(a) => (
                      <div class="action-card">
                        <header>
                          <span>ACTION #{a.index}</span>
                          <span>{t().kind === 0 ? 'spend + output' : 'issuance output'}</span>
                        </header>
                        <Kv
                          rows={[
                            ['Nullifier', <Hash value={a.nullifier} href={`/explorer/nullifier/${a.nullifier}`} short={false} />],
                            ['Note commitment (cmx)', <Hash value={a.cmx} href={`/explorer/commitment/${a.cmx}`} short={false} />],
                            ['Randomized key (rk)', <Hash value={a.rk} short={false} />],
                            ['Value commitment', <Hash value={a.value_commitment} short={false} />],
                            ['Ephemeral key', <Hash value={a.ephemeral_key} short={false} />],
                            ['Encrypted note', <EncryptedField bytes={a.encrypted_note_bytes} />],
                            ['Outgoing ciphertext', <EncryptedField bytes={a.outgoing_ciphertext_bytes} />],
                            ['Spend auth signature', <Hash value={a.signature} head={16} tail={16} />],
                          ]}
                        />
                      </div>
                    )}
                  </For>
                  <Show when={t().kind === 0}>
                    <Callout class="mt-3">
                      <b>Need to prove this payment?</b> The sender or recipient can export a payment proof from the wallet's Transactions page. It discloses one output's recipient and amount to whoever receives the proof — nothing else.
                    </Callout>
                  </Show>
                </>
              )}
            </Match>
            <Match when={tx.loading}>
              <div class="empty">Loading transaction…</div>
            </Match>
          </Switch>
        </div>
      </section>
    </>
  );
}
