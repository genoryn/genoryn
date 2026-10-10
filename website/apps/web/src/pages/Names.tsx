import { createMemo, createResource, createSignal, For, Match, Show, Switch } from 'solid-js';
import { A, useSearchParams } from '@solidjs/router';
import { NAME_PRICES, NAME_UPDATE_PRICE_GNR, RESERVED_NAMES, canonicalUsername, formatNumber, nameRegistrationPriceGnr, type NameStatus } from '@genoryn/shared';
import { Icon } from '~/components/Icon';
import { Callout, CopyButton, Gnr, Hash, Kv, PageHero, SectionHead } from '~/components/ui';
import { api } from '~/lib/api';
import { stats } from '~/lib/live';

export default function Names() {
  const [params, setParams] = useSearchParams();
  const [input, setInput] = createSignal(typeof params.u === 'string' ? params.u : '');
  const [error, setError] = createSignal<string | null>(null);
  const query = () => (typeof params.u === 'string' && params.u ? params.u : null);
  const [res] = createResource(query, (u) => api<NameStatus>(`/names/${encodeURIComponent(u)}`));

  const preview = createMemo(() => {
    const c = canonicalUsername(input());
    return input().trim() ? c : null;
  });

  const submit = (e: Event) => {
    e.preventDefault();
    const c = canonicalUsername(input());
    if (!c.ok) {
      setError(c.error);
      return;
    }
    setError(null);
    setParams({ u: c.name });
  };

  return (
    <>
      <PageHero
        eyebrow="name.genoryn.org"
        title={<>@names, on the chain.</>}
        lead="Check any GENORYN username straight from the blockchain. The first valid registration wins, names never expire, and the price is burned — nobody receives it."
        origin="Resolved by the genoryn.io mainnet node · /v1/names"
      />
      <section class="section tight">
        <div class="container narrow">
          <form class="search" onSubmit={submit}>
            <Icon name="user" size={20} class="lead-icon" />
            <input class="input big mono" placeholder="@username" value={input()} onInput={(e) => setInput(e.currentTarget.value)} maxLength={33} autocomplete="off" spellcheck={false} aria-label="Username" />
            <button class="btn primary submit" type="submit" style={{ '--h': '50px' }}>
              Check
            </button>
          </form>
          <div class="row mt-1" style={{ 'min-height': '24px', 'font-size': '13.5px' }}>
            <Show when={error()} fallback={
              <Show when={preview()}>
                {(p) => (
                  <Show when={p().ok} fallback={<span class="error-text">{(p() as { error: string }).error}</span>}>
                    <span class="muted">
                      @{(p() as { name: string }).name} · registration price <b class="strong">{formatNumber(nameRegistrationPriceGnr((p() as { name: string }).name))} GNR</b> (burned)
                    </span>
                  </Show>
                )}
              </Show>
            }>
              <span class="error-text">{error()}</span>
            </Show>
          </div>

          <div class="mt-3">
            <Switch>
              <Match when={res.loading}>
                <div class="empty">Checking @{query()} on the chain…</div>
              </Match>
              <Match when={res.error}>
                <div class="empty">{(res.error as Error).message}</div>
              </Match>
              <Match when={res()}>
                {(r) => (
                  <div class="card">
                    <div class="row between">
                      <h2 style={{ margin: 0, 'font-family': 'var(--font-display)', 'font-size': 'clamp(24px, 4vw, 34px)', 'letter-spacing': '-0.03em' }}>@{r().name}</h2>
                      <span class={`badge ${r().registered ? 'solid' : ''}`}>{r().registered ? 'registered' : r().pending ? 'registration pending' : 'available'}</span>
                    </div>
                    <Show
                      when={r().registered}
                      fallback={
                        <>
                          <p class="muted mt-2">
                            {r().pending
                              ? 'A registration for this name is waiting in the mempool. The first one confirmed in a block wins.'
                              : 'Nobody owns this name yet. Register it from the GENORYN wallet: Receive → Username.'}
                          </p>
                          <Kv
                            rows={[
                              ['Registration price', <Gnr gori={r().price_atomic} digits={0} />],
                              ['Update price', <Gnr gori={r().update_price_atomic} digits={0} />],
                              ['Checked at height', formatNumber(r().height)],
                            ]}
                          />
                          <div class="row mt-3">
                            <A href="/download" class="btn primary sm">
                              <Icon name="download" size={14} /> Get the wallet
                            </A>
                          </div>
                        </>
                      }
                    >
                      {(reg) => (
                        <div class="mt-3">
                          <Kv
                            rows={[
                              ['Pays to', reg().receiver ? <span class="row" style={{ gap: '8px', 'flex-wrap': 'nowrap' }}><span class="break">{reg().receiver}</span><CopyButton text={reg().receiver!} /></span> : '—'],
                              ['Owner key', <Hash value={reg().owner} short={false} />],
                              ['Registered at', <A class="link" href={`/explorer/block/${reg().registered_height}`}>block #{formatNumber(reg().registered_height)}</A>],
                              ['Last updated', reg().updated_height === reg().registered_height ? 'never' : <A class="link" href={`/explorer/block/${reg().updated_height}`}>block #{formatNumber(reg().updated_height)}</A>],
                              ['Sequence', String(reg().sequence)],
                              ['Registration tx', <Hash value={reg().txid} href={`/explorer/tx/${reg().txid}`} short={false} />],
                              ['Burned', <Gnr gori={reg().burned_atomic} digits={4} />],
                            ]}
                          />
                          <p class="hint mt-2">Pay @{r().name} from your wallet — it resolves the name through your own node and shows this exact address before you confirm.</p>
                        </div>
                      )}
                    </Show>
                  </div>
                )}
              </Match>
            </Switch>
          </div>
        </div>
      </section>

      <section class="section">
        <div class="container">
          <SectionHead eyebrow="Prices" title={<>Burned, not paid.</>} lead="Every price goes to the network's provably unspendable burn address. There is no registrar and no one to pay — squatting simply has a real, permanent cost." />
          <div class="grid cols-4">
            <For each={NAME_PRICES}>
              {(p) => (
                <div class="stat">
                  <div class="label">{p.lengths}</div>
                  <div class="value">{formatNumber(p.gnr)} GNR</div>
                  <div class="sub">one-time, burned</div>
                </div>
              )}
            </For>
            <div class="stat">
              <div class="label">Change receiver</div>
              <div class="value">{formatNumber(NAME_UPDATE_PRICE_GNR)} GNR</div>
              <div class="sub">per update, burned</div>
            </div>
          </div>
        </div>
      </section>

      <section class="section">
        <div class="container">
          <SectionHead eyebrow="Rules" title={<>How on-chain names work</>} />
          <div class="grid cols-3">
            <div class="card"><h3>Format</h3><p class="muted">3–32 characters: lowercase ASCII letters, digits and underscores, starting with a letter. A leading @ and upper case are accepted and normalized. Reserved: {RESERVED_NAMES.join(', ')}.</p></div>
            <div class="card"><h3>First come, forever</h3><p class="muted">Operations apply in chain order. A registration for an existing name, or one that burns less than the price, has no effect and is not refunded. A registered name never expires and cannot be taken over.</p></div>
            <div class="card"><h3>Owned by your seed</h3><p class="muted">The owner is an Ed25519 key derived from your 24-word recovery phrase (GENORYN/username/v1). Restoring the phrase restores control of the name.</p></div>
            <div class="card"><h3>One global answer</h3><p class="muted">Every node derives the same name table from the same chain, so there is no outside resolver that could swap an owner. Lookups ask only your own node.</p></div>
            <div class="card"><h3>Payments stay private</h3><p class="muted">A name publishes only the address it points to — a separate address of your wallet (index 1), never your main one. Who pays you and how much stays shielded.</p></div>
            <div class="card"><h3>No consensus split</h3><p class="muted">Names are derived state, not committed in headers. A registration is an ordinary private transfer, so older nodes accept exactly the same blocks.</p></div>
          </div>
        </div>
      </section>

      <section class="section">
        <div class="container">
          <div class="grid cols-2" style={{ 'align-items': 'start' }}>
            <div>
              <SectionHead eyebrow="Encoding" title={<>Inside a registration</>} lead="A registration is a kind-0 private transfer with one extra output that pays at least the price to the burn address. That output's 512-byte memo carries the signed record:" />
              <div class="table-wrap">
                <table class="data">
                  <thead><tr><th>Field</th><th class="num">Bytes</th></tr></thead>
                  <tbody>
                    <tr><td>Magic <code>GNRNAME\x01</code></td><td class="num mono">8</td></tr>
                    <tr><td>Operation (1 register, 2 update)</td><td class="num mono">1</td></tr>
                    <tr><td>Name length, then name</td><td class="num mono">1 + 3…32</td></tr>
                    <tr><td>Receiver (raw Orchard address)</td><td class="num mono">43</td></tr>
                    <tr><td>Owner Ed25519 public key</td><td class="num mono">32</td></tr>
                    <tr><td>Sequence (0, then previous + 1)</td><td class="num mono">4</td></tr>
                    <tr><td>Ed25519 signature</td><td class="num mono">64</td></tr>
                    <tr><td>Zero padding</td><td class="num mono">rest</td></tr>
                  </tbody>
                </table>
              </div>
              <p class="hint mt-2">The owner signs GENORYN/name-record/v1 ‖ genesis ‖ op ‖ len ‖ name ‖ receiver ‖ owner ‖ sequence, binding the record to one network.</p>
            </div>
            <div>
              <SectionHead eyebrow="The burn address" title={<>Spendable by no one</>} />
              <p class="muted">
                The burn address belongs to a full viewing key built from public values: <code>ak</code> is a hash-to-curve point (GENORYN-name-burn-v1) whose discrete logarithm nobody knows, and <code>nk</code>/<code>rivk</code> are public hashes. Without that discrete logarithm no spend authorization signature can exist.
              </p>
              <p class="muted">
                Because the viewing key is public, every node decrypts burn outputs and checks them against the on-chain commitment — so the burned value it reads is the value actually committed. The same address receives the mainnet treasury share.
              </p>
              <Callout warn class="mt-2">
                Lookalike names (<code>maxl</code> vs <code>max1</code>) remain a social risk — always check the full name and the address your wallet shows. A reorganization deeper than a registration's confirmations can change which of two competing registrations won.
              </Callout>
            </div>
          </div>
          <p class="muted mt-4" style={{ 'font-size': '14px' }}>
            Legacy note: before on-chain names, each node kept signed bundles in a local registry. It still answers for compatibility ({stats()?.usernames ?? 0} local records on this node), but it is not globally unique and new wallets do not register there.
          </p>
        </div>
      </section>
    </>
  );
}
