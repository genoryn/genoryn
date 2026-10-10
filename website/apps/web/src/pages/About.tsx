import { A } from '@solidjs/router';
import { GENESIS_MESSAGE, REPO_URL } from '@genoryn/shared';
import { GSign } from '~/components/Logo';
import { ExtLink, PageHero } from '~/components/ui';

export default function About() {
  return (
    <div class="editorial">
      <PageHero eyebrow="About" title={<>Nobody's money. <em>Everybody's right.</em></>} lead="GENORYN is an independent, open-source proof-of-work monetary network. It has no company, no foundation token sale and no known founder." />
      <section class="section tight">
        <div class="container narrow">
          <article class="prose">
            <h2>Origin</h2>
            <div class="kv" style={{ 'grid-template-columns': '180px minmax(0,1fr)', 'margin-bottom': '28px' }}>
              <div class="k">Creator</div>
              <div class="v" style={{ 'font-family': 'var(--font-serif)', 'font-size': '22px' }}>Unknown</div>
              <div class="k">Language</div>
              <div class="v">Rust 1.92</div>
              <div class="k">License</div>
              <div class="v">MIT OR Apache-2.0</div>
              <div class="k">Genesis</div>
              <div class="v">Mainnet 1.0 · 2026-10-10 · release 1.0.0 · no premine</div>
              <div class="k">Repository</div>
              <div class="v"><ExtLink href={REPO_URL}>github.com/genoryn/genoryn</ExtLink></div>
            </div>
            <p>
              The person or people who wrote GENORYN have chosen not to be known. That is deliberate and fitting: money that asks you to verify rather than trust should not depend on trusting its author either. The code, the specification and every decision record are public. Anyone can read them, build them, run them and challenge them.
            </p>
            <p>Every network starts with a message embedded in its genesis record. GENORYN's reads:</p>
            <blockquote>“{GENESIS_MESSAGE}”</blockquote>

            <h2>Principles</h2>
            <ul>
              <li><b>Privacy is the default,</b> not an option you must remember to turn on. There is no transparent send mode.</li>
              <li><b>Supply is finite and auditable.</b> 21,000,000 GNR, exact integer issuance, no mint key.</li>
              <li><b>Anyone can mine.</b> CPU-friendly RandomX; mining never starts without consent and never needs keys.</li>
              <li><b>Verify, don't trust.</b> The wallet is a full node. Seeds, explorers and websites are conveniences, not authorities.</li>
              <li><b>Say what is not done.</b> Unaudited code is labelled as such. Limits of privacy are written next to its strengths.</li>
              <li><b>No silent changes.</b> No auto-updates, no remote kill switch, no founder override.</li>
            </ul>

            <h2>This website</h2>
            <p>
              genoryn.io brings together everything about the network in one place — the consumer side (wallet, download, names, pay, support, learn, status) and the developer side (docs, explorer, API, nodes, protocol, mining, testnet, devnet, releases, security). It runs a validating GENORYN mainnet full node, a public seed, a solo CPU miner and an indexer, all on one server. It has no analytics, cookies or third-party requests.
            </p>
            <p>
              The site is open about its own limits too: it can only show public chain data, and your wallet never depends on it. <A href="/status">See its live status</A>.
            </p>
            <div class="row mt-4" style={{ 'justify-content': 'center', color: 'var(--text-strong)' }}>
              <GSign size={96} />
            </div>
          </article>
        </div>
      </section>
    </div>
  );
}
