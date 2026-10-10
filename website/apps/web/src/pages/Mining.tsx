import { createMemo, createSignal, For, Show } from 'solid-js';
import { A } from '@solidjs/router';
import {
  BLOCKS_PER_YEAR,
  EARLY_BLOCKS,
  EMISSION_DOC_URL,
  HALVING_INTERVAL,
  INITIAL_NORMAL_REWARD,
  NETWORKS,
  NORMAL_START,
  formatDuration,
  formatGnr,
  formatHashrate,
  formatNumber,
  goriToGnr,
  reward,
  supply,
} from '@genoryn/shared';
import { Icon } from '~/components/Icon';
import { MathTex } from '~/components/MathTex';
import { Callout, CodeBlock, ExtLink, Gnr, LiveDot, PageHero, SectionHead, StaleNote, Stat } from '~/components/ui';
import { EarlyMinerDisclosure } from '~/components/EarlyMining';
import { miner, stats } from '~/lib/live';

export default function Mining() {
  const s = () => stats();
  const m = () => miner();
  const known = () => !!s();
  const height = () => s()?.height ?? 0;
  const nextIsEarly = () => height() + 1 <= EARLY_BLOCKS;
  const difficulty = () => Number(s()?.difficulty ?? 0);
  const perThread = () => (m()?.hashrate && m()?.threads ? m()!.hashrate! / m()!.threads! : 42);

  const [hr, setHr] = createSignal('');
  const yourHr = () => {
    const v = Number(hr());
    return Number.isFinite(v) && v > 0 ? v : perThread() * 4;
  };
  const calc = createMemo(() => {
    const D = difficulty();
    const H = yourHr();
    const r = reward(height() + 1);
    if (!D || !H) return null;
    const T = D / H; // expected seconds per block found by you, at current difficulty
    const netHr = D / 60;
    const share = H / (netHr + H);
    const blocksPerDay = Math.min(1440, 86400 / T);
    const minerGori = r.miner;
    const perDay = BigInt(Math.floor(blocksPerDay * 1e6)) * minerGori / 1_000_000n;
    const pDay = 1 - Math.exp(-86400 / T);
    return { T, netHr, share, blocksPerDay, perDay, pDay, minerGori };
  });

  const sup = createMemo(() => supply(height()));
  const nextHalving = () => sup().next_halving_height ?? HALVING_INTERVAL + NORMAL_START;
  const halvings = createMemo(() =>
    Array.from({ length: 8 }, (_, e) => {
      const start = e * HALVING_INTERVAL + NORMAL_START;
      return { e, start, year: (e * HALVING_INTERVAL) / BLOCKS_PER_YEAR, subsidy: INITIAL_NORMAL_REWARD >> BigInt(e) };
    }),
  );
  const mainnet = NETWORKS.mainnet;

  return (
    <>
      <PageHero
        eyebrow="mining.genoryn.dev"
        title={<>Mine with the CPU you already own.</>}
        lead="GENORYN uses RandomX proof of work, designed for ordinary processors. No pools to trust, no special hardware, no keys on the mining machine. Mining never starts by itself."
        origin="Live data from the genoryn.io node and its CPU miner"
        actions={
          <>
            <A href="/download" class="btn primary">
              <Icon name="download" size={16} /> Get the wallet
            </A>
            <a href="#calculator" class="btn">
              Mining calculator
            </a>
          </>
        }
      />

      <section class="section tight">
        <div class="container">
          <StaleNote class="mb-2" />
          <div class="stat-strip">
            <Stat label="Network difficulty" live={!!s()?.node_online} value={known() ? formatNumber(s()!.difficulty) : '—'} sub="next block" />
            <Stat label="Network hashrate" icon="cpu" value={known() ? formatHashrate(s()!.hashrate) : '—'} sub="≈ difficulty ÷ 60 s" />
            <Stat label="Miner reward" icon="coins" value={<Gnr gori={known() ? reward(height() + 1).miner.toString() : null} digits={4} />} sub={nextIsEarly() ? 'early incentive (blocks 1–500) · fees extra' : 'normal subsidy · fees extra'} />
            <Stat label="Next halving" icon="clock" value={formatNumber(nextHalving())} sub={known() ? `in ${formatNumber(nextHalving() - height())} blocks · ~${formatDuration((nextHalving() - height()) * 60)}` : '—'} />
          </div>
          <div class="card mt-3">
            <div class="panel-title">
              <h3 class="row" style={{ gap: '10px' }}>
                <LiveDot on={!!m()?.online} /> genoryn.io miner
              </h3>
              <span class="badge">{m()?.online ? 'mining' : 'idle / unavailable'}</span>
            </div>
            <p class="muted" style={{ 'font-size': '14.5px' }}>
              This site's server runs a solo RandomX miner next to its full node, exactly as described below. Its numbers are taken from the miner's real JSON status lines — nothing is estimated or simulated.
            </p>
            <div class="grid cols-4 mt-2">
              <Stat label="Hashrate" value={formatHashrate(m()?.hashrate)} sub="incl. RandomX cache setup" />
              <Stat label="Threads" value={m()?.threads ?? '—'} sub="light mode, ~256 MiB each" />
              <Stat label="Hashes" value={m()?.hashes ? formatNumber(m()!.hashes!) : '—'} sub="this session" />
              <Stat label="Blocks" value={m()?.accepted ?? '—'} sub={`${m()?.rejected ?? 0} stale/rejected`} />
            </div>
          </div>
        </div>
      </section>

      <section class="section" id="calculator">
        <div class="container">
          <SectionHead eyebrow="Calculator" title={<>What can you expect?</>} lead="Solo mining is a lottery with fair odds: your chance of finding the next block equals your share of the network's hashrate." />
          <div class="calc">
            <div>
              <div class="field">
                <label for="hr">Your hashrate (H/s)</label>
                <input id="hr" class="input mono" inputmode="decimal" placeholder={`${Math.round(perThread() * 4)} (≈ 4 threads)`} value={hr()} onInput={(e) => setHr(e.currentTarget.value.replace(/[^\d.]/g, ''))} />
                <span class="hint">Light-mode RandomX on this server measures about {Math.round(perThread())} H/s per thread. Desktop CPUs vary.</span>
              </div>
              <div class="row mt-2">
                <For each={[1, 2, 4]}>
                  {(t) => (
                    <button class="btn sm" type="button" onClick={() => setHr(String(Math.round(perThread() * t)))}>
                      {t} thread{t > 1 ? 's' : ''}
                    </button>
                  )}
                </For>
              </div>
              <div class="mt-3">
                <MathTex block tex={'T_{\\text{block}} = \\frac{D}{H_{\\text{you}}} \\qquad P(\\text{found within } t) = 1 - e^{-t / T_{\\text{block}}}'} caption="Expected seconds until you find a block, and the probability of at least one block within time t (Poisson process)." />
              </div>
            </div>
            <div>
              <Show when={calc()} fallback={<p class="muted">Waiting for live network difficulty…</p>}>
                {(c) => (
                  <>
                    <div class="result"><span>Your hashrate</span><b>{formatHashrate(yourHr())}</b></div>
                    <div class="result"><span>Share of network</span><b>{(c().share * 100).toFixed(2)}%</b></div>
                    <div class="result"><span>Expected time per block</span><b>{formatDuration(c().T)}</b></div>
                    <div class="result"><span>Chance of ≥ 1 block in 24 h</span><b>{(c().pDay * 100).toFixed(c().pDay > 0.999 ? 3 : 1)}%</b></div>
                    <div class="result"><span>Expected blocks per day</span><b>{c().blocksPerDay.toFixed(2)}</b></div>
                    <div class="result"><span>Expected GNR per day</span><b>{formatGnr(c().perDay, 4)} GNR</b></div>
                  </>
                )}
              </Show>
              <p class="hint mt-2">Estimates assume today's difficulty stays constant. When more miners join, difficulty rises and every miner's share falls. Mined coins mature after 60 blocks.{nextIsEarly() ? ' Until block 500 the estimate uses the 1,000 GNR early subsidy, which ends at block 500; from block 501 the miner reward is about 4.83 GNR.' : ''}</p>
            </div>
          </div>
        </div>
      </section>

      <section class="section">
        <div class="container">
          <SectionHead eyebrow="Rules" title={<>Simple rules, enforced by every node.</>} />
          <div class="grid cols-3">
            <div class="card"><h3>RandomX, one version</h3><p class="muted">A single explicit algorithm version with GENORYN epoch keys. No alternate easy verifier or mock proof of work exists in production consensus.</p></div>
            <div class="card"><h3>Explicit start only</h3><p class="muted">Mining is never enabled at wallet or daemon startup. You start it from the Mining page or with genoryn-miner, and stop it any time.</p></div>
            <div class="card"><h3>Pays an address, holds no keys</h3><p class="muted">A miner needs a node and a receiving address — never a wallet file or recovery phrase. Nothing is silently redirected to a developer.</p></div>
            <div class="card"><h3>Up to 16 workers</h3><p class="muted">One thread per logical CPU, up to 16, each with an independent light-mode RandomX VM and about 256 MiB cache.</p></div>
            <div class="card"><h3>60-second target</h3><p class="muted">Difficulty retargets every block over up to 60 weighted intervals. Each adjustment is limited to 75–125% of the previous difficulty.</p></div>
            <div class="card"><h3>Maturity</h3><p class="muted">Mining outputs become spendable after 60 blocks — about an hour. On mainnet the treasury share is burned, never paid to anyone.</p></div>
          </div>
        </div>
      </section>

      <section class="section">
        <div class="container">
          <SectionHead eyebrow="The math" title={<>Formulas, exactly as in consensus.</>} lead="All monetary and difficulty arithmetic is integer-only. These are the rules every node evaluates for every block." />
          <div class="grid cols-2">
            <div class="stack">
              <h3>Proof-of-work validity</h3>
              <MathTex block tex={'\\mathrm{valid} \\iff \\mathrm{LE256}\\big(\\mathrm{RandomX}_{k}(\\text{header})\\big) \\cdot D < 2^{256}'} caption="The RandomX output is read as a little-endian 256-bit integer; multiplied by the difficulty, it must still fit in 256 bits." />
              <h3 class="mt-2">RandomX epoch key</h3>
              <MathTex block tex={'h_{\\text{seed}} = \\left\\lfloor \\frac{\\max(h - 65,\\, 0)}{2048} \\right\\rfloor \\cdot 2048'} caption="The key block for height h changes every 2,048 blocks with a 65-block lag (genesis for epoch zero), then passes through the GENORYN RandomX domain hash." />
              <h3 class="mt-2">Hashrate estimate</h3>
              <MathTex block tex={'\\hat{H}_{\\text{net}} = \\frac{D_{\\text{next}}}{60\\ \\text{s}}'} caption="Target-implied network hashrate reported by nodes; not observed miner telemetry." />
            </div>
            <div class="stack">
              <h3>Difficulty retarget (LWMA-style)</h3>
              <MathTex block tex={'D_{\\text{raw}} = \\frac{\\left(\\sum_{i=1}^{n} D_i\\right) \\cdot T \\cdot \\sum_{i=1}^{n} i}{n \\cdot \\max\\!\\left(\\sum_{i=1}^{n} i \\cdot \\tau_i,\\ \\tfrac{T}{10}\\sum_{i=1}^{n} i\\right)}'} caption="Up to n = 60 recent blocks, weights increase toward the tip, T = 60 s." />
              <MathTex block tex={'\\tau_i = \\mathrm{clamp}(t_i - t_{i-1},\\ -6T,\\ 6T) \\qquad D_{\\text{next}} = \\mathrm{clamp}\\!\\left(D_{\\text{raw}},\\ \\tfrac{3}{4}D_{\\text{prev}},\\ \\tfrac{5}{4}D_{\\text{prev}}\\right)'} caption="Solve times are clamped to ±360 s; each block moves difficulty by at most 25%; the minimum is 1. Fewer than 11 samples use the network's initial difficulty." />
              <h3 class="mt-2">Timestamps</h3>
              <MathTex block tex={'\\mathrm{MTP}_{11} < t_h \\le t_{\\text{now}} + 120\\ \\text{s}'} caption="A block's time must exceed the median of the previous 11 blocks and be at most 120 seconds in the future." />
            </div>
          </div>
        </div>
      </section>

      <section class="section">
        <div class="container">
          <SectionHead eyebrow="Rewards" title={<>Reward schedule</>} lead="Blocks 1–500 pay a fixed early subsidy. From block 501 the normal subsidy halves every 2,102,400 blocks (four protocol years of 525,600 blocks). Integer halving eventually reaches zero; there is no tail emission." />
          <MathTex block tex={'R(h) = \\underbrace{[h \\ge 501] \\cdot \\left( \\left\\lfloor \\tfrac{20{,}290{,}000 \\cdot 10^{12}}{4{,}204{,}800} \\right\\rfloor \\gg \\left\\lfloor \\tfrac{h-501}{2{,}102{,}400} \\right\\rfloor \\right)}_{\\text{normal subsidy}} + \\underbrace{1000 \\cdot 10^{12} \\cdot [1 \\le h \\le 500]}_{\\text{early}} + \\underbrace{10^{12} \\cdot [501 \\le h \\le 210{,}500]}_{\\text{treasury}}'} caption="Total issuance at height h in gori; height 0 issues nothing. The miner receives the normal subsidy or the early subsidy; on mainnet the treasury share is burned." />
          <div class="card mt-3">
            <h3>Early mining: blocks 1–500</h3>
            <EarlyMinerDisclosure />
            <p class="mt-2" style={{ 'font-size': '14.5px' }}>
              <ExtLink href={EMISSION_DOC_URL} class="link strong">
                Monetary Policy &amp; Emission Schedule ↗
              </ExtLink>
            </p>
          </div>
          <div class="grid cols-2 mt-3" style={{ 'align-items': 'start' }}>
            <div class="table-wrap">
              <table class="data">
                <thead>
                  <tr>
                    <th>Epoch</th>
                    <th class="num">From height</th>
                    <th class="num">Year</th>
                    <th class="num">Subsidy (GNR)</th>
                  </tr>
                </thead>
                <tbody>
                  <tr style={known() && nextIsEarly() ? { background: 'var(--panel-3)' } : undefined}>
                    <td class="mono">early{known() && nextIsEarly() ? ' ← now' : ''}</td>
                    <td class="num mono">1</td>
                    <td class="num mono">0</td>
                    <td class="num mono">1,000</td>
                  </tr>
                  <For each={halvings()}>
                    {(r) => (
                      <tr style={known() && !nextIsEarly() && sup().halving_epoch === r.e ? { background: 'var(--panel-3)' } : undefined}>
                        <td class="mono">{r.e}{known() && !nextIsEarly() && sup().halving_epoch === r.e ? ' ← now' : ''}</td>
                        <td class="num mono">{formatNumber(r.start)}</td>
                        <td class="num mono">{r.year}</td>
                        <td class="num mono">{goriToGnr(r.subsidy)}</td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </div>
            <div class="card">
              <h3>At the next block (#{known() ? formatNumber(height() + 1) : '—'})</h3>
              <div class="calc" style={{ border: 0, 'grid-template-columns': '1fr' }}>
                <div style={{ padding: 0, background: 'transparent' }}>
                  <div class="result"><span>Normal subsidy</span><b>{known() ? `${goriToGnr(reward(height() + 1).normal)} GNR` : '—'}</b></div>
                  <div class="result"><span>Early subsidy</span><b>{known() ? `${goriToGnr(reward(height() + 1).early)} GNR` : '—'}</b></div>
                  <div class="result"><span>Miner total</span><b>{known() ? `${goriToGnr(reward(height() + 1).miner)} GNR` : '—'}</b></div>
                  <div class="result"><span>Treasury (burned on mainnet)</span><b>{known() ? `${goriToGnr(reward(height() + 1).treasury)} GNR` : '—'}</b></div>
                  <div class="result"><span>Plus</span><b>all transaction fees</b></div>
                </div>
              </div>
              <p class="hint mt-2">Fees transfer existing shielded value to the miner and are never new issuance. Wallet fee tiers are Economy, Standard and Priority at 1×, 2× and 5× the 0.00001 GNR minimum.</p>
            </div>
          </div>
        </div>
      </section>

      <section class="section">
        <div class="container">
          <SectionHead eyebrow="Start mining" title={<>Two ways to mine.</>} />
          <div class="grid cols-2">
            <div class="card">
              <h3>In the wallet</h3>
              <ol class="muted" style={{ 'padding-left': '20px', 'line-height': 1.9 }}>
                <li>Download and open the wallet in Full mode; its node starts automatically.</li>
                <li>Create or unlock a wallet — the destination is its receiving address.</li>
                <li>Open <b class="strong">Mining</b>, choose CPU threads (one per logical CPU, up to 16), press <b class="strong">Start</b>.</li>
                <li>Hashrate, accepted blocks and rewards appear live. If the miner stops, the page explains why and retries in portable RandomX mode or with fewer threads.</li>
              </ol>
            </div>
            <div>
              <CodeBlock
                title="On a server — no keys required"
                code={`# 1. Run a full node (Mainnet 1.0 is the default)
$ genorynd --data-dir ~/.genoryn/mainnet-1.0
# 2. Mine to YOUR receiving address (gnr1…)
$ genoryn-miner --node http://127.0.0.1:${mainnet.adminRpcPort} \\
    --token-file ~/.genoryn/mainnet-1.0/admin.token \\
    --wallet gnr1YOUR_RECEIVER --threads 4`}
              />
              <p class="hint mt-2">The admin token is a private loopback capability, never a spend key, and is never sent to a remote endpoint. Use --max-blocks or --max-seconds to bound test runs.</p>
            </div>
          </div>
          <Callout class="mt-3">
            Pooling, P2Pool-style protocols and fast-memory (dataset) mining are roadmap work. CPU-friendly does not prove ASIC or GPU resistance; GENORYN makes no such claim.
          </Callout>
        </div>
      </section>
    </>
  );
}
