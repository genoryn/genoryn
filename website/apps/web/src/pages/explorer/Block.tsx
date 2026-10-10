import { createEffect, createResource, Match, Switch } from 'solid-js';
import { A, useParams } from '@solidjs/router';
import { formatNumber, goriToGnr, reward, utc, type BlockDetail } from '@genoryn/shared';
import { Icon } from '~/components/Icon';
import { Callout, Gnr, Hash, Kv, TimeAgo } from '~/components/ui';
import { api, apiUrl, ApiError } from '~/lib/api';
import { setTitle } from '~/lib/dom';
import { onNewBlock, stats } from '~/lib/live';
import { DetailNav, ExplorerHeader, TxTable } from './common';
import { onCleanup, onMount } from 'solid-js';

export default function BlockPage() {
  const params = useParams();
  const [block, { refetch }] = createResource(() => params.id, (id) => api<BlockDetail>(`/block/${encodeURIComponent(id)}`));

  onMount(() => {
    const off = onNewBlock(() => {
      if (block() && !block()!.next) void refetch();
    });
    onCleanup(off);
  });
  createEffect(() => {
    const b = block();
    if (b) setTitle(`Block #${formatNumber(b.height)} — GENORYN Explorer`);
  });

  const confirmations = () => {
    const b = block();
    if (!b) return 0;
    return Math.max(b.confirmations, (stats()?.height ?? 0) - b.height + 1);
  };

  return (
    <>
      <ExplorerHeader eyebrow="Block" title={block() ? `Block #${formatNumber(block()!.height)}` : `Block ${(params.id ?? '').length > 16 ? (params.id ?? '').slice(0, 12) + '…' : params.id ?? ''}`} />
      <section class="section tight">
        <div class="container wide">
          <Switch>
            <Match when={block.error}>
              <div class="empty">
                <h3>{(block.error as ApiError).status === 404 ? 'Block not found' : 'Could not load block'}</h3>
                <p>{(block.error as Error).message}. It may not be mined or indexed yet.</p>
                <A href="/explorer" class="btn sm">
                  Back to explorer
                </A>
              </div>
            </Match>
            <Match when={block()}>
              {(b) => (
                <>
                  <div class="detail-head">
                    <div class="row">
                      <span class="badge solid">{confirmations()} confirmation{confirmations() === 1 ? "" : "s"}</span>
                      <span class="badge">{b().network}</span>
                      <span class="badge">v{b().version}</span>
                    </div>
                    <div class="row">
                      <a class="btn sm" href={apiUrl(`/block/${b().height}/raw`)} download="">
                        <Icon name="download" size={14} /> Raw JSON
                      </a>
                      <DetailNav prev={b().height > 1 ? `/explorer/block/${b().height - 1}` : null} next={b().next ? `/explorer/block/${b().height + 1}` : null} />
                    </div>
                  </div>
                  <Kv
                    rows={[
                      ['Block hash', <Hash value={b().hash} short={false} />],
                      ['Height', formatNumber(b().height)],
                      ['Timestamp', <>{utc(b().timestamp)} · <span class="muted"><TimeAgo ts={b().timestamp} /></span></>],
                      ['Solve time', b().solve_time === null ? '—' : `${b().solve_time} s (target 60 s)`],
                      ['Previous block', b().height > 1 ? <Hash value={b().previous} href={`/explorer/block/${b().height - 1}`} short={false} /> : <Hash value={b().previous} short={false} />],
                      ['Next block', b().next ? <Hash value={b().next!} href={`/explorer/block/${b().height + 1}`} short={false} /> : <span class="muted">not yet mined</span>],
                      ['Difficulty', formatNumber(b().difficulty)],
                      ['Nonce', b().nonce],
                      ['Transactions', `${b().tx_count} (${b().transfer_count} shielded transfers, ${b().tx_count - b().transfer_count} issuance)`],
                      ['Actions', String(b().action_count)],
                      ['Size', `${formatNumber(b().size)} bytes (max 2,000,000)`],
                      ['Miner reward', <Gnr gori={b().miner_reward} digits={12} />],
                      ['Treasury issuance', <Gnr gori={b().treasury_reward} digits={12} />],
                      ['Fees', <Gnr gori={b().fees} digits={12} />],
                      ['Scheduled reward', <span class="muted">{goriToGnr(reward(b().height).total)} GNR total by consensus schedule</span>],
                      ['RandomX seed', <Hash value={b().pow_seed} short={false} />],
                      ['Body hash', <Hash value={b().body_hash} short={false} />],
                      ['Matured-notes root', <Hash value={b().notes_root} short={false} />],
                      ['Nullifier root', <Hash value={b().nullifiers_root} short={false} />],
                    ]}
                  />
                  <div class="panel-title mt-4">
                    <h2>Transactions in this block</h2>
                    <span class="badge">{b().transactions.length}</span>
                  </div>
                  <TxTable txs={b().transactions} showBlock={false} />
                  <Callout class="mt-3">
                    The first transaction is the miner's issuance (kind 1) — 1,000 GNR early subsidy for heights 1–500, the normal subsidy afterwards — and, for heights 501–210,500, the second is the scheduled treasury issuance (kind 2); on mainnet it pays the provably unspendable burn address. Their amounts are public by design so anyone can audit the 21M cap. Ordinary transfers, including on-chain username registrations, are fully shielded.
                  </Callout>
                </>
              )}
            </Match>
            <Match when={block.loading}>
              <div class="kv">
                {Array.from({ length: 8 }).map(() => (
                  <>
                    <div class="k"><span class="skeleton">Loading field</span></div>
                    <div class="v"><span class="skeleton">0000000000000000000000000000000000</span></div>
                  </>
                ))}
              </div>
            </Match>
          </Switch>
        </div>
      </section>
    </>
  );
}
