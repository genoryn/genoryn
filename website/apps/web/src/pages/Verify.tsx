import { createMemo, createResource, createSignal, Match, onMount, Show, Switch, type JSX } from 'solid-js';
import { A } from '@solidjs/router';
import { isServer } from 'solid-js/web';
import {
  MAX_PROOF_FILE_BYTES,
  RESPONSE_PREFIX,
  cleanCode,
  extractProofJson,
  formatNumber,
  parseGnr,
  proofClaims,
  validateAddress,
  type ChallengeIssued,
  type NetworkName,
  type ProofCheck,
  type SenderCheck,
} from '@genoryn/shared';
import { Icon } from '~/components/Icon';
import { Callout, CopyButton, Feature, Gnr, Hash, Kv, PageHero, SectionHead } from '~/components/ui';
import { api } from '~/lib/api';

const post = <T,>(path: string, body: unknown) =>
  api<T>(path, { method: 'POST', body: JSON.stringify(body), headers: { 'content-type': 'application/json' } });

/** A challenge stays usable across a reload while the sender answers. */
const CHALLENGE_KEY = 'genoryn.verify.challenge';

function Verdict(props: { ok: boolean; children: string }) {
  return (
    <p class={props.ok ? 'ok-text' : 'error-text'} style={{ margin: '6px 0', display: 'flex', gap: '8px', 'align-items': 'flex-start' }}>
      <span aria-hidden="true">{props.ok ? '✓' : '✕'}</span>
      <span>{props.children}</span>
    </p>
  );
}

export default function Verify() {
  const [input, setInput] = createSignal('');
  const [expectedTo, setExpectedTo] = createSignal('');
  const [expectedAmount, setExpectedAmount] = createSignal('');
  const [busy, setBusy] = createSignal<null | 'verify' | 'challenge' | 'sender'>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [checked, setChecked] = createSignal<{ proof: string; result: ProofCheck } | null>(null);
  const [challenge, setChallenge] = createSignal<ChallengeIssued | null>(null);
  const [response, setResponse] = createSignal('');
  const [senderError, setSenderError] = createSignal<string | null>(null);
  const [sender, setSender] = createSignal<SenderCheck | null>(null);
  const [status] = createResource(() => (isServer ? null : 'status'), () => api<{ enabled: boolean; network: NetworkName }>('/proof/status'));

  // The fragment never reaches a server, so a proof can be shared as /verify#GNRPROOF1-…
  onMount(() => {
    const fragment = decodeURIComponent(location.hash.slice(1));
    if (fragment.includes('GNRPROOF1-')) setInput(fragment);
  });

  const parsed = createMemo(() => {
    if (!input().trim()) return null;
    try {
      const json = extractProofJson(input());
      return { ok: true as const, json, claims: proofClaims(json) };
    } catch (e) {
      return { ok: false as const, error: (e as Error).message };
    }
  });
  const toCheck = createMemo(() => (expectedTo().trim() ? validateAddress(expectedTo().trim(), status()?.network ?? 'mainnet') : null));
  const amountCheck = createMemo(() => {
    if (!expectedAmount().trim()) return null;
    try {
      parseGnr(expectedAmount());
      return null;
    } catch (e) {
      return (e as Error).message;
    }
  });
  const canVerify = () => parsed()?.ok && !busy() && !(toCheck() && !toCheck()!.valid) && !amountCheck();

  const loadFile = async (file: File | undefined) => {
    if (!file) return;
    if (file.size > MAX_PROOF_FILE_BYTES) {
      setError('This file is too large to be a payment proof.');
      return;
    }
    const text = new TextDecoder('utf-8').decode(await file.arrayBuffer());
    try {
      setInput(extractProofJson(text));
      setError(null);
    } catch (e) {
      setError((e as Error).message);
    }
  };

  const restoreChallenge = (txid: string) => {
    try {
      const saved = JSON.parse(sessionStorage.getItem(CHALLENGE_KEY) ?? 'null') as (ChallengeIssued & { txid: string }) | null;
      if (saved?.txid === txid && saved.expires * 1000 > Date.now()) setChallenge({ challenge: saved.challenge, expires: saved.expires });
    } catch {
      /* nothing saved */
    }
  };

  const verify = async (e: Event) => {
    e.preventDefault();
    const p = parsed();
    if (!p?.ok) return;
    setBusy('verify');
    setError(null);
    setChecked(null);
    setChallenge(null);
    setSender(null);
    setSenderError(null);
    try {
      const result = await post<ProofCheck>('/proof/verify', { proof: p.json, expected_recipient: expectedTo().trim(), expected_amount: expectedAmount().trim() });
      setChecked({ proof: p.json, result });
      restoreChallenge(result.txid);
    } catch (err) {
      setError((err as Error).message);
    } finally {
      setBusy(null);
    }
  };

  const requestChallenge = async () => {
    const c = checked();
    if (!c) return;
    setBusy('challenge');
    setSenderError(null);
    setSender(null);
    try {
      const issued = await post<ChallengeIssued>('/proof/challenge', { proof: c.proof });
      setChallenge(issued);
      sessionStorage.setItem(CHALLENGE_KEY, JSON.stringify({ ...issued, txid: c.result.txid }));
    } catch (err) {
      setSenderError((err as Error).message);
    } finally {
      setBusy(null);
    }
  };

  const responseCode = createMemo(() => cleanCode(response(), RESPONSE_PREFIX));
  const verifySender = async () => {
    const c = checked();
    const ch = challenge();
    if (!c || !ch || !responseCode()) return;
    setBusy('sender');
    setSenderError(null);
    try {
      setSender(await post<SenderCheck>('/proof/sender', { proof: c.proof, challenge: ch.challenge, response: responseCode() }));
    } catch (err) {
      setSenderError((err as Error).message);
    } finally {
      setBusy(null);
    }
  };

  const mismatch = () => {
    const r = checked()?.result;
    if (r?.recipient_matches === false) return 'Recipient mismatch';
    if (r?.amount_matches === false) return 'Amount mismatch';
    return null;
  };

  return (
    <>
      <PageHero
        eyebrow="genoryn.io/verify"
        title={<>Was I really paid?</>}
        lead="Paste the payment proof you received — the code, the JSON or the PDF. genoryn.io checks it against the GENORYN Mainnet 1.0 chain with exactly the code the wallet's Verify payment page runs."
        origin="Payment Proof 2.0 · checked by the genoryn.io full node"
      />
      <section class="section tight">
        <div class="container">
          <Show when={status() && !status()!.enabled}>
            <Callout warn class="mb-3">
              Proof checking is being set up on this server. Until then, use <b>Verify payment</b> in the GENORYN wallet — it runs the same checks on your own computer.
            </Callout>
          </Show>
          <form class="calc" onSubmit={verify}>
            <div>
              <div class="field">
                <label for="proof">Payment proof</label>
                <textarea
                  id="proof"
                  class="input mono"
                  rows={7}
                  placeholder="GNRPROOF1-…   or   { &quot;version&quot;: 1, … }"
                  value={input()}
                  onInput={(e) => setInput(e.currentTarget.value)}
                  spellcheck={false}
                  autocomplete="off"
                />
                <div class="row between">
                  <label class="btn sm" style={{ cursor: 'pointer' }}>
                    <Icon name="file" size={14} /> Load .json or .pdf
                    <input type="file" accept=".json,.pdf,.txt,application/json,application/pdf,text/plain" style={{ display: 'none' }} onChange={(e) => void loadFile(e.currentTarget.files?.[0])} />
                  </label>
                  <Show when={input()}>
                    <button type="button" class="btn sm ghost" onClick={() => { setInput(''); setChecked(null); setError(null); }}>
                      Clear
                    </button>
                  </Show>
                </div>
                <Show when={parsed()}>
                  {(p) => (
                    <Show when={p().ok} fallback={<span class="error-text">{(p() as { error: string }).error}</span>}>
                      <span class="hint">
                        Proof found:{' '}
                        <b class="strong">
                          <Gnr gori={(p() as { claims: { amountAtomic: string } }).claims.amountAtomic} digits={12} />
                        </b>{' '}
                        in transaction {(p() as { claims: { txid: string } }).claims.txid.slice(0, 12)}… — not checked yet.
                      </span>
                    </Show>
                  )}
                </Show>
              </div>
              <div class="field mt-2">
                <label for="to">The address you expected to be paid (optional)</label>
                <input id="to" class="input mono" placeholder="gnr1…" value={expectedTo()} onInput={(e) => setExpectedTo(e.currentTarget.value.replace(/\s/g, ''))} spellcheck={false} autocomplete="off" />
                <Show when={toCheck() && !toCheck()!.valid}>
                  <span class="error-text">{toCheck()!.error}</span>
                </Show>
              </div>
              <div class="field mt-2">
                <label for="amount">The amount you expected (GNR, optional)</label>
                <input id="amount" class="input mono" inputmode="decimal" placeholder="e.g. 10" value={expectedAmount()} onInput={(e) => setExpectedAmount(e.currentTarget.value.replace(/[^\d.]/g, ''))} autocomplete="off" />
                <Show when={amountCheck()} fallback={<span class="hint">Checked exactly, to the last gori.</span>}>
                  <span class="error-text">{amountCheck()}</span>
                </Show>
              </div>
              <button class="btn primary mt-3" type="submit" disabled={!canVerify()} style={{ width: '100%' }}>
                <Icon name="shield" size={16} /> {busy() === 'verify' ? 'Checking against the chain…' : 'Verify payment'}
              </button>
            </div>
            <div>
              <Switch fallback={<div class="empty" style={{ height: '100%', display: 'flex', 'align-items': 'center', 'justify-content': 'center' }}>The result appears here: whether the payment is on the chain, to which address, for exactly how much and with how many confirmations.</div>}>
                <Match when={error()}>
                  <h3 style={{ 'margin-top': 0 }}>Not verified</h3>
                  <Verdict ok={false}>{error()!}</Verdict>
                  <p class="hint">A proof that was edited, belongs to another payment or another network never passes.</p>
                </Match>
                <Match when={checked() && checked()!.result.outcome !== 'payment_verified'}>
                  <h3 style={{ 'margin-top': 0 }}>{checked()!.result.outcome === 'reorganized' ? 'Payment replaced or reorganized' : 'Payment pending or not found'}</h3>
                  <p class="muted">
                    This transaction is not in any block of the chain the genoryn.io node validated (tip {formatNumber(checked()!.result.chain_tip)}). If it was sent moments ago it may still be waiting for a block; check again after the next block. It does not count as paid yet.
                  </p>
                </Match>
                <Match when={checked()}>
                  {(() => {
                    const r = () => checked()!.result;
                    return (
                      <>
                        <div class="row between">
                          <h3 style={{ margin: 0 }}>{mismatch() ?? 'Payment verified'}</h3>
                          <span class={`badge ${mismatch() ? 'err' : 'solid'}`}>{mismatch() ? 'does not match' : `${formatNumber(r().confirmations ?? 0)} confirmations`}</span>
                        </div>
                        <p class="muted" style={{ margin: '6px 0 14px' }}>
                          {mismatch() ? 'A real payment was found on the chain, but it does not match what you entered.' : 'A real transaction on the GENORYN Mainnet 1.0 chain.'}
                        </p>
                        <div style={{ 'font-size': 'clamp(26px, 4vw, 38px)', 'font-family': 'var(--font-display)', 'margin-bottom': '14px' }}>
                          <Gnr gori={r().amount_atomic} digits={12} />
                        </div>
                        <Show when={r().amount_matches !== undefined}>
                          <Verdict ok={r().amount_matches!}>{r().amount_matches ? 'The amount is exactly the amount you entered.' : 'The proven amount differs from the amount you entered.'}</Verdict>
                        </Show>
                        <Show when={r().recipient_matches !== undefined}>
                          <Verdict ok={r().recipient_matches!}>{r().recipient_matches ? 'Yes — this payment went to the address you entered.' : 'No — this payment did NOT go to the address you entered.'}</Verdict>
                        </Show>
                        <Verdict ok={!!r().created_by_sender}>
                          {r().created_by_sender ? "Sender-origin evidence: this proof was made with the paying wallet's key for this payment." : 'Made from the receiving side: it proves the payment, but not who created the proof.'}
                        </Verdict>
                        <div class="mt-2">
                          <Kv
                            rows={[
                              ['Paid to', <span class="break">{r().recipient}</span>],
                              ['Transaction', <Hash value={r().txid} href={`/explorer/tx/${r().txid}`} />],
                              ['Block', <><A class="link" href={`/explorer/block/${r().block_height}`}>#{formatNumber(r().block_height ?? 0)}</A> · output {r().action}</>],
                              ['Reorganization risk', <span style={{ 'font-family': 'var(--font-sans)' }}>{r().reorg_risk}</span>],
                              ...(r().memo ? [['Memo', <span style={{ 'white-space': 'pre-wrap' }}>{r().memo}</span>] as [string, JSX.Element]] : []),
                            ]}
                          />
                        </div>
                        <details class="mt-2">
                          <summary class="muted" style={{ cursor: 'pointer' }}>Advanced details</summary>
                          <p class="hint mt-1">Sender evidence: {r().sender_evidence ?? 'not reported'} · chain tip checked: {formatNumber(r().chain_tip)}</p>
                          <Show when={r().sender_auth_key}>
                            <p class="hint">Payment-bound sender key (committed in the payment's encrypted memo): <code class="mono break">{r().sender_auth_key}</code></p>
                          </Show>
                          <p class="hint">The proof opens one output of a shielded Orchard action: its note commitment, the recipient, the amount and, with the sender's outgoing cipher key, the memo. No spending key, viewing key or other output is disclosed.</p>
                        </details>
                      </>
                    );
                  })()}
                </Match>
              </Switch>
            </div>
          </form>

          <Show when={checked()?.result.outcome === 'payment_verified'}>
            <div class="card mt-3">
              <div class="eyebrow">Who sent it? (optional)</div>
              <Switch>
                <Match when={checked()!.result.sender_evidence === 'origin_disclosure_only'}>
                  <h3>Legacy sender evidence only</h3>
                  <p class="muted mb-0">The proof was made with the paying wallet's key, but this payment was made without a sender-verification key (before GENORYN 1.1, or with it switched off). A fresh sender challenge is not possible for it.</p>
                </Match>
                <Match when={checked()!.result.sender_evidence !== 'fresh_authentication_available'}>
                  <p class="muted mb-0">This proof comes from the receiving side, so the sender cannot be challenged with it. Ask the sender for their own proof, made from the payment on their wallet's Transactions page.</p>
                </Match>
                <Match when={true}>
                  <h3>Ask the sender to prove it was them</h3>
                  <p class="muted">Anyone can forward a proof. A fresh challenge shows that the person you are talking to still holds this payment's key — their wallet signs it, and you learn nothing about their other payments or balance.</p>
                  <Show
                    when={challenge()}
                    fallback={
                      <button class="btn primary" type="button" disabled={!!busy()} onClick={() => void requestChallenge()}>
                        <Icon name="fingerprint" size={16} /> {busy() === 'challenge' ? 'Creating…' : 'Request sender verification'}
                      </button>
                    }
                  >
                    {(ch) => (
                      <div class="grid cols-2" style={{ 'align-items': 'start' }}>
                        <div class="field">
                          <label>1 · Send this challenge to the sender</label>
                          <div class="code">
                            <div class="code-head">
                              <span>challenge · valid until {new Date(ch().expires * 1000).toLocaleTimeString()}</span>
                              <CopyButton text={ch().challenge} />
                            </div>
                            <pre style={{ 'white-space': 'pre-wrap', 'word-break': 'break-all' }}>{ch().challenge}</pre>
                          </div>
                          <span class="hint">It works only for this payment, this address and this amount. In the wallet the sender opens Verify payment → Answer a payment challenge.</span>
                        </div>
                        <div class="field">
                          <label for="response">2 · Paste the sender's response</label>
                          <textarea id="response" class="input mono" rows={4} placeholder={`${RESPONSE_PREFIX}…`} value={response()} onInput={(e) => setResponse(e.currentTarget.value)} spellcheck={false} autocomplete="off" />
                          <div class="row">
                            <button class="btn primary sm" type="button" disabled={!responseCode() || !!busy()} onClick={() => void verifySender()}>
                              {busy() === 'sender' ? 'Checking…' : 'Verify sender response'}
                            </button>
                            <button class="btn sm ghost" type="button" disabled={!!busy()} onClick={() => { setChallenge(null); setSender(null); setResponse(''); sessionStorage.removeItem(CHALLENGE_KEY); }}>
                              New challenge
                            </button>
                          </div>
                          <Show when={response().trim() && !responseCode()}>
                            <span class="hint">A complete response starts with {RESPONSE_PREFIX} and is 144 characters long.</span>
                          </Show>
                        </div>
                      </div>
                    )}
                  </Show>
                  <Show when={senderError()}>
                    <Verdict ok={false}>{senderError()!}</Verdict>
                  </Show>
                  <Show when={sender()}>
                    {(s) => (
                      <div class="mt-2">
                        <Show when={s().outcome === 'sender_verified'} fallback={<Verdict ok={false}>{s().note ?? 'This payment carries no sender-verification key.'}</Verdict>}>
                          <h3 class="mb-0">Sender verified</h3>
                          <Verdict ok>{s().meaning ?? 'Whoever answered holds the key this payment committed to.'}</Verdict>
                        </Show>
                      </div>
                    )}
                  </Show>
                </Match>
              </Switch>
            </div>
          </Show>

          <Callout class="mt-3">
            What you paste is sent to genoryn.io only to be checked: it shows this one payment's recipient, amount and memo to this server. It is not stored or logged and it can never move coins. For a check that depends on nobody, use <b>Verify payment</b> in your own <A class="link" href="/wallet">GENORYN wallet</A>.
          </Callout>
        </div>
      </section>

      <section class="section">
        <div class="container">
          <SectionHead eyebrow="Payment Proof 2.0" title={<>Three separate questions.</>} lead="A proof never mixes them up, and neither does this page." />
          <div class="grid cols-3">
            <Feature icon="cube" title="Was the payment made?">
              The proof opens one shielded output. The node finds its transaction in a validated block and recomputes the note commitment: the address and the exact amount must match, to the gori.
            </Feature>
            <Feature icon="file" title="What did the sender disclose?">
              A proof made by the paying wallet carries a key only that wallet can derive. It shows where the proof came from — but a forwarded proof keeps that mark.
            </Feature>
            <Feature icon="fingerprint" title="Is it them, right now?">
              For payments made with GENORYN 1.1, the sender answers a fresh challenge with a key committed inside the payment. Replays, other payments and impostors fail.
            </Feature>
          </div>
        </div>
      </section>
    </>
  );
}
