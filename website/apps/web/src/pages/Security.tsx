import { For } from 'solid-js';
import { A } from '@solidjs/router';
import { REPO_URL } from '@genoryn/shared';
import { Toc } from '~/components/Toc';
import { Callout, CodeBlock, ExtLink, PageHero } from '~/components/ui';

const TOC: Array<[string, string]> = [
  ['status', 'Status'],
  ['audit', 'Internal review'],
  ['wallet', 'Wallet protection'],
  ['node', 'Node validation'],
  ['network', 'Network privacy'],
  ['not', 'What is not protected'],
  ['releases', 'Release verification'],
  ['report', 'Reporting'],
];

const ROWS: Array<[string, string]> = [
  ['Wallet file theft', 'Argon2id key derivation and XChaCha20-Poly1305 authenticated encryption. The recovery phrase comes from OS entropy and is never stored in plain text.'],
  ['Unattended open wallet', 'Automatic lock after five minutes without input; Lock erases held password and phrase buffers and invalidates late responses.'],
  ['Secret residue in memory', 'Rust zeroization of secret buffers; sensitive text fields discard undo history. Cannot guarantee removal from swap, crash dumps or clipboard managers.'],
  ['Remote node control', 'Admin RPC binds to loopback only and requires a constant-time-compared capability token. Public RPC is read-only, opt-in, bounded and rate-limited by concurrency.'],
  ['Invalid or malicious blocks', 'Full validation of proof of work, Halo 2 proofs, signatures, anchors and nullifier uniqueness. Frames bounded before allocation; misbehaving peers are disconnected and banned.'],
  ['Inflation', 'Integer-only issuance with no mint class; exact coinbase amounts validated; proofs enforce value balance for shielded transfers.'],
  ['Username impersonation', 'On-chain names are first-come in chain order, signed by a seed-derived owner key, permanent and resolved by your own node. Lookalike names remain a social risk.'],
  ['Burned value', 'The burn address has a hash-to-curve spend key with no known discrete logarithm; breaking it would break Orchard spend authorization generally.'],
  ['Tampered downloads', 'SHA-256 checksums for every package, a manifest with the source commit, and a wallet that never installs updates by itself.'],
];

export default function Security() {
  return (
    <div class="editorial">
      <PageHero eyebrow="security.genoryn.dev" title={<>Honest about <em>threats.</em></>} lead="What GENORYN defends against, how, and — just as important — what it does not claim to protect." origin="Threat model: docs/security/threat-model.md" />
      <section class="section tight">
        <div class="container">
          <div class="with-toc">
            <Toc items={TOC} />
            <article class="prose">
              <h2 id="status">Status</h2>
              <Callout warn>
                <b>Mainnet 1.0 is live — independent audit still pending.</b> The GENORYN consensus integration, Orchard/Halo 2 composition and network code have had an internal review, not an independent external audit. Mine or hold only what you can afford to lose.
              </Callout>

              <h2 id="audit">Internal review, October 2026</h2>
              <p>Before the first mainnet launch the codebase received an internal security review covering codec, proof of work, difficulty, emission, shielded validation, chain state, storage, fork choice, mempool, peer protocol, wallet and release handling. That review changed no consensus rule; the 1.0.0 release later introduced the early-emission rule with a new genesis. Thirteen findings were fixed, including:</p>
              <ul>
                <li><b>High:</b> side-branch blocks are checked for header context and RandomX work before any branch replay; unknown parents are refused.</li>
                <li><b>High:</b> policy rejections (full mempool, duplicates, timing races) no longer strike or ban honest relays — only invalid data is punished.</li>
                <li><b>Medium:</b> indexed block locators and per-connection rate limits for GetBlocks, Ping and GetPeers.</li>
                <li><b>Medium:</b> per-netgroup discovery caps, solicited peer lists only, four inbound connections per IP.</li>
                <li><b>Medium:</b> blocks applied in place with tail-only header context — no quadratic cost growth.</li>
                <li><b>Medium:</b> mining templates build their Halo 2 proofs outside the node lock; public RPC has its own concurrency budget.</li>
                <li><b>Low:</b> fee-rate displacement in a full mempool, bounded ban tables, FIFO proof cache, HTTPS-only seed lists.</li>
              </ul>
              <p>Admin endpoints answer HTTP 401 for a missing or wrong token, remote connections require HTTPS (plain HTTP only for literal loopback), and the repository has GitHub secret scanning and push protection enabled.</p>
              <p>Residual risks are documented openly: block-by-block sync without headers-first download or minimum chain work, a nullifier root that hashes the whole set per block, and the need for independent cryptography, consensus and network audits.</p>

              <h2 id="wallet">Wallet protection</h2>
              <div class="table-wrap">
                <table class="data">
                  <thead><tr><th style={{ width: '30%' }}>Threat</th><th>Mitigation</th></tr></thead>
                  <tbody>
                    <For each={ROWS}>{([t, m]) => (<tr><td class="strong">{t}</td><td class="muted">{m}</td></tr>)}</For>
                  </tbody>
                </table>
              </div>

              <h2 id="node">Node validation</h2>
              <p>
                In Full mode the wallet starts its own node; in Quick mode it fetches blocks from remote nodes over Tor and follows the chain with the most cumulative work as computed locally. Either way it does not trust seeds, explorers (including this one) or any server for consensus — it checks every block, proof, signature and nullifier itself. Seeds only help nodes meet; cached and exchanged peers, explicit peers and full validation remain independent of any seed operator.
              </p>
              <p>
                The genoryn.io explorer is a convenience. It runs the same node software and shows what that node validated, but your wallet never relies on it.
              </p>

              <h2 id="network">Network privacy</h2>
              <p>
                In Direct and Hybrid mode, clearnet peers see your IP address. A Tor client is built in, and new installations start in Tor-only mode: every connection, peer lookup and payment broadcast goes through Tor and nothing ever falls back to a direct connection. Payments you create are first sent to one relay peer before wider broadcast. Tor does not protect against an adversary who watches both ends of a circuit, and application checks do not replace OS-level isolation such as Whonix or Tails. Details: <A href="/privacy">Tor &amp; privacy</A>.
              </p>

              <h2 id="not">What is not protected</h2>
              <ul>
                <li>Malware, keyloggers or remote-access tools on your computer.</li>
                <li>Copied or photographed recovery phrases; weak passwords.</li>
                <li>Screenshots, screen recording, clipboard history and clipboard managers.</li>
                <li>Transaction timing correlation and a small anonymity set while the network is young.</li>
                <li>In Direct or Hybrid mode, a network observer or peer that sees your IP address; in Tor mode, an adversary correlating both ends of a Tor circuit.</li>
                <li>Voluntary disclosures: payment proofs you share, off-chain metadata, exchange records.</li>
              </ul>
              <p>GENORYN does not claim absolute anonymity, instant finality or an inherited audit.</p>

              <h2 id="releases">Release verification</h2>
              <p>
                Every package on GitHub carries a <code>.sha256</code> file, an internal <code>SHA256SUMS</code> list and a release manifest naming its exact source commit. The wallet only points to the official repository's releases and never downloads or installs anything automatically.
              </p>
              <CodeBlock code={`$ sha256sum -c genoryn-1.0.0-mainnet-linux-x86_64.zip.sha256\n$ unzip genoryn-1.0.0-mainnet-linux-x86_64.zip && cd genoryn-1.0.0-mainnet-linux-x86_64\n$ sha256sum -c SHA256SUMS --quiet`} />
              <p class="mt-2">Independent builder reproducibility, production signing and a multi-maintainer policy remain release gates before any mainnet.</p>

              <h2 id="report">Reporting a vulnerability</h2>
              <p>
                Please report security issues privately following the repository's <ExtLink href={`${REPO_URL}/blob/main/SECURITY.md`}>security policy</ExtLink>, or through GitHub's private vulnerability reporting on <ExtLink href={`${REPO_URL}/security`}>github.com/genoryn/genoryn/security</ExtLink>. Do not open public issues for undisclosed vulnerabilities.
              </p>
              <p>
                This website itself sends no trackers or cookies, loads no third-party scripts or fonts, serves a strict Content-Security-Policy, and exposes only read-only, rate-limited chain data.
              </p>
            </article>
          </div>
        </div>
      </section>
    </div>
  );
}
