import { createMemo, createSignal, onMount, Show } from 'solid-js';
import { useSearchParams } from '@solidjs/router';
import { buildPaymentUri, formatGnr, parsePaymentUri, validateAddress, type NetworkName } from '@genoryn/shared';
import { QR } from '~/components/QR';
import { Callout, CopyButton, PageHero, SectionHead } from '~/components/ui';

export default function Pay() {
  const [params] = useSearchParams();
  const [network, setNetwork] = createSignal<NetworkName>('mainnet');
  const [address, setAddress] = createSignal('');
  const [amount, setAmount] = createSignal('');
  const [verifyText, setVerifyText] = createSignal('');
  onMount(() => {
    if (typeof params.to === 'string') setAddress(params.to);
  });

  const check = createMemo(() => (address().trim() ? validateAddress(address().trim(), network()) : null));
  const built = createMemo(() => {
    if (!check()?.valid) return { uri: null, error: null };
    try {
      return { uri: buildPaymentUri(network(), address().trim(), amount().trim() || undefined), error: null };
    } catch (e) {
      return { uri: null, error: (e as Error).message };
    }
  });
  const verified = createMemo(() => {
    const t = verifyText().trim();
    if (!t) return null;
    try {
      const r = parsePaymentUri(t, network());
      return { ok: true as const, ...r };
    } catch (e) {
      return { ok: false as const, error: (e as Error).message };
    }
  });

  return (
    <>
      <PageHero eyebrow="pay.genoryn.org" title={<>Ask to be paid.</>} lead="Create a payment request and a monochrome QR code entirely in your browser. Nothing you type here is sent anywhere." origin="Experimental payment URI v1 · same rules as the wallet" />
      <section class="section tight">
        <div class="container">
          <div class="calc">
            <div>
              <div class="field">
                <label for="net">Network</label>
                <select id="net" class="input" value={network()} onChange={(e) => setNetwork(e.currentTarget.value as NetworkName)}>
                  <option value="mainnet">mainnet (gnr1…)</option>
                  <option value="testnet">testnet (gnrt1…)</option>
                  <option value="devnet">devnet (gnrd1…)</option>
                </select>
              </div>
              <div class="field mt-2">
                <label for="addr">Your receiving identity</label>
                <textarea id="addr" class="input mono" rows={3} placeholder="gnr1…" value={address()} onInput={(e) => setAddress(e.currentTarget.value.replace(/\s/g, ''))} spellcheck={false} />
                <Show when={check()}>
                  <span class={check()!.valid ? 'ok-text' : 'error-text'}>{check()!.valid ? `✓ Valid ${check()!.network} receiver (Bech32m, ${check()!.byteLength} bytes)` : check()!.error}</span>
                </Show>
              </div>
              <div class="field mt-2">
                <label for="amt">Amount (GNR, optional)</label>
                <input id="amt" class="input mono" inputmode="decimal" placeholder="e.g. 1.5" value={amount()} onInput={(e) => setAmount(e.currentTarget.value.replace(/[^\d.]/g, ''))} />
                <span class="hint">Up to 12 decimal places. No memos or descriptions — they would be visible to anyone holding the request.</span>
              </div>
              <Show when={built().error}>
                <p class="error-text mt-2">{built().error}</p>
              </Show>
            </div>
            <div style={{ display: 'flex', 'flex-direction': 'column', 'align-items': 'center', 'justify-content': 'center', gap: '18px' }}>
              <Show when={built().uri} fallback={<div class="empty" style={{ width: '100%' }}>Enter a valid receiving identity to generate a request.</div>}>
                <QR value={built().uri!} />
                <div class="code" style={{ width: '100%' }}>
                  <div class="code-head">
                    <span>payment request</span>
                    <CopyButton text={built().uri!} />
                  </div>
                  <pre style={{ 'white-space': 'pre-wrap', 'word-break': 'break-all' }}>{built().uri}</pre>
                </div>
              </Show>
            </div>
          </div>
          <Callout class="mt-3">
            Importing a request in the wallet only fills in a draft. It never connects anywhere, unlocks, signs or broadcasts; the separate confirmation step remains mandatory. Share requests only with the intended payer — the address and amount are visible to anyone who has them.
          </Callout>
        </div>
      </section>
      <section class="section">
        <div class="container narrow">
          <SectionHead eyebrow="Check a request" title={<>Received a payment link?</>} lead="Paste it to see exactly what it asks for before you pay." />
          <textarea class="input mono" rows={3} placeholder="genoryn:gnr1…?v=1&network=mainnet&amount=…" value={verifyText()} onInput={(e) => setVerifyText(e.currentTarget.value.trim())} spellcheck={false} aria-label="Payment request to check" />
          <Show when={verified()}>
            <div class="card mt-2">
              <Show when={verified()!.ok} fallback={<p class="error-text mb-0">✕ {(verified() as { error: string }).error}</p>}>
                <p class="ok-text">✓ Well-formed {network()} payment request</p>
                <div class="kv mt-2">
                  <div class="k">Pay to</div>
                  <div class="v">{(verified() as { address: string }).address}</div>
                  <div class="k">Amount</div>
                  <div class="v">{(verified() as { amountGori: bigint | null }).amountGori !== null ? `${formatGnr((verified() as { amountGori: bigint }).amountGori, 12)} GNR` : 'not specified'}</div>
                </div>
              </Show>
            </div>
          </Show>
        </div>
      </section>
    </>
  );
}
