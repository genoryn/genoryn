import { createMemo, For, onCleanup, onMount, Show } from 'solid-js';
import { A } from '@solidjs/router';
import { createEffect } from 'solid-js';
import {
  EARLY_BLOCKS,
  EARLY_BUDGET,
  EMISSION_DOC_URL,
  NORMAL_BUDGET,
  REPO_URL,
  TREASURY_BUDGET,
  MAX_SUPPLY,
  emissionCurve,
  formatGnr,
  formatHashrate,
  formatNumber,
  percent,
  reward,
  timeAgo,
} from '@genoryn/shared';
import { GSign } from '~/components/Logo';
import { Icon } from '~/components/Icon';
import { SearchBar } from '~/components/SearchBar';
import { LineChart } from '~/components/Chart';
import { MathTex } from '~/components/MathTex';
import { ExtLink, Gnr, LiveDot, SectionHead, StaleNote, Stat } from '~/components/ui';
import { blocks, connected, now, onNewBlock, stats } from '~/lib/live';
import { theme } from '~/lib/theme';
import type { HeroHandle } from '~/three/hero';

function Hero() {
  let canvasHost!: HTMLDivElement;
  let copy!: HTMLDivElement;
  let hero: HeroHandle | null = null;
  const textEdge = () => {
    const base = canvasHost.getBoundingClientRect().left;
    let right = copy.getBoundingClientRect().right;
    copy.querySelectorAll('h1 .line > span').forEach((el) => (right = Math.max(right, el.getBoundingClientRect().right)));
    return right - base + 24;
  };

  onMount(() => {
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
    let cancelled = false;
    const supportsWebGL = (() => {
      try {
        const c = document.createElement('canvas');
        return !!(c.getContext('webgl2') || c.getContext('webgl'));
      } catch {
        return false;
      }
    })();
    if (supportsWebGL) {
      void import('~/three/hero').then(({ createHero }) => {
        if (cancelled) return;
        hero = createHero(canvasHost, theme(), reduced, textEdge);
        canvasHost.classList.add('ready');
      });
    }
    const off = onNewBlock(() => hero?.pulse());
    onCleanup(() => {
      cancelled = true;
      off();
      hero?.dispose();
      hero = null;
    });
  });
  createEffect(() => {
    // Read the signal unconditionally so the effect stays subscribed even before the hero loads.
    const t = theme();
    hero?.setTheme(t);
  });

  const s = () => stats();
  return (
    <section class="hero" aria-labelledby="hero-title">
      <div class="bg-grid" />
      <div class="hero-canvas" ref={canvasHost} aria-hidden="true" />
      <div class="hero-scrim" aria-hidden="true" />
      <div class="container hero-inner">
        <div class="hero-copy" ref={copy}>
          <div class="eyebrow">
            <LiveDot on={connected() && !!s() && !s()!.stale} /> Mainnet 1.0 live · no premine
          </div>
          <h1 id="hero-title">
            <span class="line"><span>Privacy is</span></span>
            <span class="line"><span>not a crime.</span></span>
            <span class="line"><span>It's a <span class="outline">right.</span></span></span>
          </h1>
          <p class="lead">
            Peer-to-peer money you verify yourself. Every transfer is a shielded note proven with Halo&nbsp;2, the supply is capped at 21,000,000&nbsp;GNR, and anyone can mine it with an ordinary CPU.
          </p>
          <div class="row cta">
            <A href="/download" class="btn primary lg">
              <Icon name="download" size={18} /> Download wallet
            </A>
            <A href="/explorer" class="btn lg">
              Explore the chain <Icon name="arrow" size={16} />
            </A>
            <A href="/learn" class="btn ghost lg hide-sm">
              How it works
            </A>
          </div>
          <div class="hero-search">
            <SearchBar big />
          </div>
          <div class="hero-meta">
            <div>
              <span class="row" style={{ gap: '8px' }}>
                <LiveDot on={connected() && !!s() && !s()!.stale} /> Block height
              </span>
              <b>{s() ? formatNumber(s()!.height) : '—'}</b>
              <Show when={s()?.stale}>
                <small>{s()!.last_updated ? `last updated ${timeAgo(s()!.last_updated!, now())}` : 'last known'} · node unreachable</small>
              </Show>
            </div>
            <div>
              Network hashrate
              <b>{s() ? formatHashrate(s()!.hashrate) : '—'}</b>
            </div>
            <div>
              Issued supply
              <b>{s() ? `${formatGnr(s()!.issued_supply, 0)} GNR` : '—'}</b>
            </div>
            <div>
              Nodes seen · 24h
              <b>{s() ? formatNumber(Math.max(s()!.nodes_24h, s()!.nodes_connected)) : '—'}</b>
              <small>connected to our seeds · estimate</small>
            </div>
            <div>
              Est. miners
              <b>{s() ? `≈ ${formatNumber(s()!.est_miners)}` : '—'}</b>
              <small>network hashrate ÷ ~200 H/s · estimate</small>
            </div>
          </div>
        </div>
      </div>
      <div class="scroll-hint" aria-hidden="true" />
    </section>
  );
}

function Ticker() {
  const items = () => blocks().slice(0, 20);
  return (
    <div class="ticker" aria-label="Latest blocks">
      <Show
        when={items().length}
        fallback={
          <div class="container mono muted" style={{ 'font-size': '13px' }}>
            Connecting to the genoryn.io full node…
          </div>
        }
      >
        <div class="marquee">
          <div class="marquee-track">
            <For each={[...items(), ...items()]}>
              {(b) => (
                <A href={`/explorer/block/${b.height}`} class="ticker-item">
                  <GSign size={12} />
                  <b>#{formatNumber(b.height)}</b>
                  <span>{b.tx_count} tx</span>
                  <span>{timeAgo(b.timestamp, now())}</span>
                </A>
              )}
            </For>
          </div>
        </div>
      </Show>
    </div>
  );
}

const PILLARS = [
  { n: '01', title: 'Private by design', icon: 'eyeOff', body: 'Every transfer is a shielded note. Amounts and receiving identities are hidden from chain observers; a spend reveals only a nullifier. There is no transparent balance database, no address-balance API and no rich list.' },
  { n: '02', title: 'Finite by protocol', icon: 'infinity', body: 'Issuance is an exact integer schedule inside 21,000,000 GNR. No mint transaction class, no founder override, no financial master key. Issuance and fees stay public, so anyone can audit the cap.' },
  { n: '03', title: 'Fair to mine', icon: 'cpu', body: 'RandomX proof of work is designed for ordinary CPUs. Mining never starts by itself and pays to a receiving address only — it runs safely while the wallet is locked, or on a server that holds no keys.' },
  { n: '04', title: "Verify, don't trust", icon: 'shield', body: 'In Full mode the wallet runs its own node; in Quick mode it fetches blocks from remote nodes over built-in Tor. Either way every block, proof, signature and nullifier is validated on your computer. Seeds only introduce peers; they are never trusted for consensus.' },
];

function LiveChain() {
  const s = () => stats();
  const recent = () => blocks().slice(0, 9);
  return (
    <section class="section">
      <div class="container">
        <SectionHead eyebrow="Live mainnet" title={<>The chain, right now.</>} lead="Streamed in real time from the genoryn.io mainnet full node, which validates every block itself before it is shown here." />
        <StaleNote class="mb-2" />
        <div class="stat-strip reveal">
          <Stat label="Height" live={connected() && !!s() && !s()!.stale} value={s() ? formatNumber(s()!.height) : '—'} sub={s()?.last_block_time ? `last block ${timeAgo(s()!.last_block_time!, now())}` : 'waiting for node'} />
          <Stat label="Difficulty" icon="zap" value={s() ? formatNumber(s()!.difficulty) : '—'} sub="per-block LWMA retarget" />
          <Stat label="Hashrate" icon="cpu" value={s() ? formatHashrate(s()!.hashrate) : '—'} sub="difficulty ÷ 60 s target" />
          <Stat label="Avg block time" icon="clock" value={s()?.avg_block_time ? `${s()!.avg_block_time!.toFixed(1)} s` : '—'} sub="last 60 blocks · target 60 s" />
          <Stat label="Miner reward" icon="coins" value={<Gnr gori={s() ? reward(s()!.height + 1).miner.toString() : null} digits={3} />} sub={s() && s()!.height + 1 <= EARLY_BLOCKS ? 'early incentive (blocks 1–500) · fees extra' : 'normal subsidy · fees extra'} />
          <Stat label="Nodes seen · 24h" icon="globe" value={s() ? formatNumber(Math.max(s()!.nodes_24h, s()!.nodes_connected)) : '—'} sub={s() ? `estimate · seen on our seeds · ≈ ${formatNumber(s()!.est_miners)} miners from hashrate` : 'estimate'} />
        </div>
        <div class="card mt-3 reveal" style={{ padding: '22px' }}>
          <div class="panel-title">
            <h3>Latest blocks</h3>
            <A href="/explorer" class="btn sm">
              Open explorer <Icon name="arrow" size={14} />
            </A>
          </div>
          <div class="chain-viz">
            <div class="chain-block pending">
              <div class="h">#{s() ? formatNumber(s()!.height + 1) : '…'}</div>
              <div class="m">
                mining…
                <br />
                {s() ? s()!.mempool : '—'} in mempool
              </div>
            </div>
            <For each={recent()}>
              {(b, i) => (
                <A href={`/explorer/block/${b.height}`} class={`chain-block ${i() === 0 ? 'new' : ''}`}>
                  <div class="h">#{formatNumber(b.height)}</div>
                  <div class="m">
                    {b.tx_count} tx · {b.action_count} act
                    <br />
                    {(b.size / 1024).toFixed(1)} KiB
                    <br />
                    {timeAgo(b.timestamp, now())}
                  </div>
                  <div class="fill" style={{ width: `${Math.min(100, (b.size / 2_000_000) * 100 * 20)}%` }} />
                </A>
              )}
            </For>
          </div>
        </div>
      </div>
    </section>
  );
}

function Monetary() {
  const curve = createMemo(() => emissionCurve(40, 1).map((p) => ({ x: p.year, y: Number(p.issued / 10n ** 12n), label: `Year ${p.year}` })));
  const issued = () => (stats() ? BigInt(stats()!.issued_supply) : null);
  return (
    <section class="section">
      <div class="container">
        <SectionHead eyebrow="Monetary policy" title={<>21,000,000. Not one gori more.</>} lead="Consensus uses exact u128 integer arithmetic. No tail emission, no mint authority. On mainnet the treasury share is paid to a provably unspendable address, so at most about 20,789,999.99995 GNR can ever circulate." />
        <div class="grid cols-2" style={{ 'align-items': 'stretch' }}>
          <div class="card reveal">
            <h3>Allocation</h3>
            <div class="alloc-bar mt-2">
              <div style={{ width: `${percent(NORMAL_BUDGET, MAX_SUPPLY, 2)}%`, background: 'var(--text-strong)' }} />
              <div style={{ width: `${percent(EARLY_BUDGET, MAX_SUPPLY, 2)}%`, background: 'var(--muted)' }} />
              <div style={{ width: `${percent(TREASURY_BUDGET, MAX_SUPPLY, 2)}%`, background: 'var(--border-strong)' }} />
            </div>
            <div class="legend">
              <span><i style={{ background: 'var(--text-strong)' }} />Normal mining 20,290,000 GNR (96.62%)</span>
              <span><i style={{ background: 'var(--muted)' }} />Early mining 500,000 GNR (2.38%)</span>
              <span><i style={{ background: 'var(--border-strong)' }} />Treasury 210,000 GNR (1%) — burned on mainnet</span>
            </div>
            <div class="mt-3">
              <MathTex block tex={'s(h) = \\left\\lfloor \\frac{20{,}290{,}000 \\cdot 10^{12}}{2 \\cdot 2{,}102{,}400} \\right\\rfloor \\gg \\left\\lfloor \\frac{h-501}{2{,}102{,}400} \\right\\rfloor \\quad (h \\ge 501)'} caption="Normal subsidy in gori at height h, starting at block 501. It halves every 2,102,400 blocks (~4 years)." />
            </div>
            <p class="muted mt-2" style={{ 'font-size': '14.5px' }}>
              Blocks 1–500 instead pay an early subsidy of 1,000 GNR each to whoever mines them — the whole subsidy, fees extra. At launch the only known miner is run by the project operator, so most or all of these 500,000 GNR may go to the operator. The 1 GNR treasury share per block (heights 501–210,500) goes to the burn address, whose spend key nobody — developers included — can ever know.{' '}
              <ExtLink href={EMISSION_DOC_URL} class="strong">
                Monetary Policy &amp; Emission Schedule
              </ExtLink>
            </p>
            <div class="mt-2">
              <div class="row between mono" style={{ 'font-size': '12px' }}>
                <span class="muted">Issued so far</span>
                <span class="strong">{issued() === null ? '—' : `${percent(issued()!, MAX_SUPPLY, 4)}%`}</span>
              </div>
              <div class="progress mt-1">
                <div style={{ width: `${issued() === null ? 0 : Math.max(0.4, Number(percent(issued()!, MAX_SUPPLY, 4)))}%` }} />
              </div>
            </div>
          </div>
          <div class="card reveal">
            <div class="panel-title">
              <h3>Issuance curve</h3>
              <span class="badge">GNR · years</span>
            </div>
            <LineChart points={curve()} height={300} zeroBased xFormat={(x) => `year ${x}`} format={(v) => `${(v / 1e6).toFixed(2)}M`} />
            <p class="muted mt-2" style={{ 'font-size': '14px' }}>
              About 50% of the normal-mining budget is issued in the first four years, 75% after eight. The curve approaches but never exceeds the cap.
            </p>
            <A href="/mining" class="btn sm mt-1">
              Mining & rewards <Icon name="arrow" size={14} />
            </A>
          </div>
        </div>
      </div>
    </section>
  );
}

const SAFETY = [
  ['Someone copies your wallet file', 'It is encrypted with Argon2id key derivation and XChaCha20-Poly1305. The recovery phrase comes from operating-system entropy and is never written in plain text.'],
  ['Someone uses your open wallet', 'Nothing is reachable without the password; the wallet locks after five minutes without input and Lock closes every open wallet.'],
  ['A server learns your balance', 'There is none to ask. Your wallet finds its own notes by scanning every ciphertext locally.'],
  ['A peer sends invalid blocks', 'Your node checks proof of work, Halo 2 proofs, signatures, anchors and nullifier uniqueness, with bounded messages and misbehaviour disconnection.'],
  ['Double spends and inflation', 'Nullifiers are unique across chain and mempool, reorganisations replay state, and coinbase value is fixed by an exact integer schedule.'],
  ['Remote control of your node', 'Administrative RPC binds to loopback only and requires a private capability token; public RPC is read-only and opt-in.'],
  ['A tampered update', 'The wallet only points to the official repository releases and never installs anything; every package carries SHA-256 checksums and a manifest with its source commit.'],
];

export default function Home() {
  return (
    <>
      <Hero />
      <Ticker />

      <section class="section">
        <div class="container">
          <SectionHead eyebrow="A new generation of digital cash" title={<>Four ideas, rarely found together.</>} lead="Privacy by default, a hard cap enforced by integer consensus rules, CPU-fair mining, and a wallet that validates every block itself." />
          <div class="grid cols-2">
            <For each={PILLARS}>
              {(p) => (
                <article class="pillar card spot reveal">
                  <Icon name={p.icon} size={220} class="big-glyph" stroke={0.6} />
                  <span class="num">{p.n}</span>
                  <h3>{p.title}</h3>
                  <p>{p.body}</p>
                </article>
              )}
            </For>
          </div>
        </div>
      </section>

      <section class="section tight">
        <div class="container">
          <div class="numbers reveal">
            <div>
              <div class="n">21M</div>
              <div class="t">GNR hard cap, enforced in integer consensus</div>
            </div>
            <div>
              <div class="n">60s</div>
              <div class="t">Block target with per-block difficulty retarget</div>
            </div>
            <div>
              <div class="n">10¹²</div>
              <div class="t">gori per GNR — exact atomic units</div>
            </div>
            <div>
              <div class="n">0</div>
              <div class="t">premine or mint keys — every coin is mined, and the treasury share is burned</div>
            </div>
          </div>
        </div>
      </section>

      <section class="section">
        <div class="container">
          <SectionHead eyebrow="How a private payment works" title={<>Proven, not revealed.</>} lead="The network verifies that money is real and unspent without ever learning who paid whom, or how much." />
          <div class="flow reveal">
            <div>
              <h4>Create a note</h4>
              <p>Your wallet builds a shielded output for the recipient, encrypted with one-time key material. No stable address is published.</p>
            </div>
            <div>
              <h4>Prove it with Halo 2</h4>
              <p>A zero-knowledge proof shows the inputs exist, are yours and balance the outputs plus a public fee — without revealing them.</p>
            </div>
            <div>
              <h4>Nodes validate</h4>
              <p>Every full node checks the proof, signatures, the anchor and that each nullifier has never appeared before. Double spends are impossible.</p>
            </div>
            <div>
              <h4>Scan locally</h4>
              <p>The recipient's wallet trial-decrypts every note on its own machine. No server ever learns a balance or history.</p>
            </div>
          </div>
        </div>
      </section>

      <LiveChain />
      <Monetary />

      <section class="section">
        <div class="container">
          <SectionHead eyebrow="Why it is safe" title={<>Your device holds the keys.</>} lead="Your wallet validates everything, in Quick or Full mode; the network only ever sees shielded notes." />
          <div class="table-wrap reveal">
            <table class="data">
              <thead>
                <tr>
                  <th style={{ width: '32%' }}>Risk</th>
                  <th>How GENORYN responds</th>
                </tr>
              </thead>
              <tbody>
                <For each={SAFETY}>
                  {([risk, answer]) => (
                    <tr>
                      <td class="strong">{risk}</td>
                      <td class="muted">{answer}</td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
          <div class="callout warn mt-3 reveal">
            <Icon name="alert" size={18} style={{ 'flex-shrink': 0, 'margin-top': '2px' }} />
            <div>
              <b>What privacy does not protect against:</b> malware on your computer, copied seeds, weak passwords, screenshots and clipboard history, transaction timing, a small anonymity set, or an adversary watching both ends of a Tor circuit. New installations hide your IP behind the built-in Tor client; Direct and Hybrid modes do not. GENORYN does not claim absolute anonymity. <A href="/security" class="strong" style={{ 'border-bottom': '1px solid' }}>Read the threat model</A>.
            </div>
          </div>
        </div>
      </section>

      <section class="section">
        <div class="container">
          <SectionHead eyebrow="Compared" title={<>Built differently.</>} />
          <div class="table-wrap reveal">
            <table class="data compare">
              <thead>
                <tr>
                  <th />
                  <th>GENORYN</th>
                  <th>Typical transparent PoW chain</th>
                  <th>Typical account-based chain</th>
                </tr>
              </thead>
              <tbody>
                <tr><td>Amounts hidden by default</td><td class="yes">Yes — every transfer</td><td class="no">No</td><td class="no">No</td></tr>
                <tr><td>Receiver hidden from observers</td><td class="yes">Yes</td><td class="no">No</td><td class="no">No</td></tr>
                <tr><td>Public balance lookup / rich list</td><td class="yes">Impossible by design</td><td class="no">Yes</td><td class="no">Yes</td></tr>
                <tr><td>Supply cap</td><td class="yes">21,000,000 GNR, integer-exact</td><td class="no">Varies</td><td class="no">Often uncapped</td></tr>
                <tr><td>Mining hardware</td><td class="yes">Ordinary CPUs (RandomX)</td><td class="no">Often ASIC farms</td><td class="no">Stake, not work</td></tr>
                <tr><td>Wallet validates every block</td><td class="yes">Yes — Full mode node or Quick mode over Tor</td><td class="no">Rarely</td><td class="no">Rarely</td></tr>
                <tr><td>Hides your IP from peers</td><td class="yes">Built-in Tor, on by default</td><td class="no">Manual setup</td><td class="no">Rarely</td></tr>
                <tr><td>Selective disclosure</td><td class="yes">Per-output payment proofs</td><td class="no">Everything is public</td><td class="no">Everything is public</td></tr>
              </tbody>
            </table>
          </div>
        </div>
      </section>

      <section class="section">
        <div class="container">
          <div class="cta-band reveal">
            <GSign size={420} class="cta-glyph" />
            <div class="eyebrow" style={{ color: 'inherit', opacity: 0.6 }}>Open source · MIT or Apache-2.0</div>
            <h2>Run your own node. Hold your own keys.</h2>
            <p>Download the wallet, verify the checksum, and you are a full participant in the network in minutes. Mining is one click away — and never starts without you.</p>
            <div class="row mt-3">
              <A href="/download" class="btn primary lg">
                <Icon name="download" size={18} /> Download
              </A>
              <ExtLink href={REPO_URL} class="btn lg">
                <Icon name="github" size={18} /> Source on GitHub
              </ExtLink>
            </div>
          </div>
        </div>
      </section>
    </>
  );
}
