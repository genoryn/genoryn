import { A } from '@solidjs/router';
import { For } from 'solid-js';
import { MathTex } from '~/components/MathTex';
import { Toc } from '~/components/Toc';
import { Callout, PageHero } from '~/components/ui';

const TOC: Array<[string, string]> = [
  ['why', 'Why private money'],
  ['shielded-notes', 'Shielded notes'],
  ['nullifiers', 'Commitments & nullifiers'],
  ['zk', 'Zero-knowledge proofs'],
  ['keys', 'Keys & addresses'],
  ['pow', 'Proof of work'],
  ['supply', 'A finite supply'],
  ['fees', 'Fees'],
  ['finality', 'Confirmations'],
  ['proofs', 'Payment proofs'],
  ['names', 'Usernames'],
  ['limits', 'Limits of privacy'],
  ['glossary', 'Glossary'],
];

const GLOSSARY: Array<[string, string]> = [
  ['GNR', 'The GENORYN currency unit. Maximum supply 21,000,000 GNR.'],
  ['gori', 'The smallest unit: 1 GNR = 10¹² gori. All consensus values are integers in gori.'],
  ['Note', 'A shielded unit of value, encrypted to its owner. The private equivalent of a coin.'],
  ['Note commitment (cmx)', 'The public fingerprint of a note, added to the note tree. Reveals nothing about value or owner.'],
  ['Nullifier', 'A unique tag revealed when a note is spent. Each can appear only once, which prevents double spending.'],
  ['Anchor', 'A recent root of the note-commitment tree that a transaction proves its inputs against (120-block window).'],
  ['Action', 'One spend plus one output inside a shielded bundle. Ordinary transfers have 2–16 actions.'],
  ['Halo 2', 'The zero-knowledge proving system used by GENORYN. It needs no trusted setup.'],
  ['RandomX', 'A proof-of-work algorithm optimized for general-purpose CPUs.'],
  ['Difficulty', 'How much work a block must prove. Adjusted every block to target 60 seconds.'],
  ['Maturity', 'Blocks a mining output must wait before it can be spent: 60 on mainnet (about an hour).'],
  ['Mainnet', 'The real GENORYN network, Mainnet 1.0 (genoryn-mainnet-1.0), launched with release 1.0.0 on 10 October 2026. The earlier genoryn-mainnet-v1 is retired.'],
  ['Burn address', 'An address nobody can ever spend from. Mainnet treasury issuance and username prices go there.'],
  ['Full node', 'Software that downloads and validates every block itself. The GENORYN wallet runs one in Full mode; in Quick mode it still validates every block, fetched over Tor.'],
  ['Receiving identity', 'Your address (gnr1… on mainnet). It is never written on chain.'],
];

export default function Learn() {
  return (
    <div class="editorial">
      <PageHero eyebrow="learn.genoryn.org" title={<>How private money <em>works.</em></>} lead="A plain-language guide to GENORYN: what is hidden, what is public, how mining secures it, and where privacy ends. No prior knowledge needed." origin="Reading time ≈ 15 minutes" />
      <section class="section tight">
        <div class="container">
          <div class="with-toc">
            <Toc items={TOC} />
            <article class="prose">
              <h2 id="why">Why private money</h2>
              <p>
                Cash in your pocket does not announce to the world how much you hold, who paid you or what you bought. Most blockchains do the opposite: every balance and every payment is published forever, readable by anyone, and easy to link to a person once a single transaction is tied to their identity.
              </p>
              <p>
                GENORYN restores the properties of cash to digital money. Payments are verified by everyone, but their contents are visible only to the people involved. That is not a loophole — it is the default for every transfer, because financial privacy protects ordinary people from theft, discrimination, profiling and coercion.
              </p>
              <blockquote>Privacy is not a crime. It's a right.</blockquote>

              <h2 id="shielded-notes">Shielded notes</h2>
              <p>
                GENORYN has no accounts and no public balances. Value lives in <b>notes</b>: small encrypted records that say "this amount belongs to this receiver." When you are paid, the sender creates a new note encrypted to your receiving identity using one-time key material. Repeated payments to you do not publish a stable destination.
              </p>
              <p>
                Your wallet finds your money by <b>trial-decrypting every note</b> in every block on your own computer. Notes it can open are yours; the rest are noise. Because this happens locally, there is no server to ask "what is my balance?" — and therefore no server that could leak it.
              </p>

              <h2 id="nullifiers">Commitments and nullifiers</h2>
              <p>
                If everything is encrypted, how does the network stop someone from spending the same money twice? Two public values make it work:
              </p>
              <ul>
                <li>
                  A <b>note commitment</b> is published when a note is created. It goes into a large Merkle tree (32 levels deep) of all notes ever made. It reveals nothing about the amount or owner.
                </li>
                <li>
                  A <b>nullifier</b> is published when a note is spent. Only the owner can compute it, it is unique to that note, and it cannot be linked back to the commitment by anyone else.
                </li>
              </ul>
              <p>
                Every node keeps the set of all nullifiers. A transaction that reveals a nullifier already in the set — in the chain or in the mempool — is rejected. So double spending is impossible, yet nobody learns which note was spent.
              </p>
              <MathTex block tex={'\\text{spend valid} \\iff \\mathrm{nf} \\notin \\mathcal{N}_{\\text{chain}} \\cup \\mathcal{N}_{\\text{mempool}} \\ \\wedge\\ \\pi \\text{ verifies against a recent anchor}'} />

              <h2 id="zk">Zero-knowledge proofs</h2>
              <p>
                Each transaction carries a <b>Halo 2</b> proof — a few kilobytes of mathematics that convince every node of statements like these, without revealing the underlying data:
              </p>
              <ul>
                <li>the notes being spent exist in the commitment tree at a recent anchor;</li>
                <li>the spender knows the keys that authorize them;</li>
                <li>the nullifiers are computed correctly;</li>
                <li>inputs equal outputs plus the public fee — no money is created.</li>
              </ul>
              <p>Halo 2 needs no trusted setup ceremony: there is no secret "toxic waste" that someone could use to forge coins.</p>
              <MathTex block tex={'\\sum v_{\\text{in}} = \\sum v_{\\text{out}} + \\text{fee} \\quad\\text{(proven via value commitments, never revealed)}'} />

              <h2 id="keys">Keys and addresses</h2>
              <p>Your 24-word recovery phrase derives three distinct kinds of capability:</p>
              <ul>
                <li><b>Spend authority</b> — signs spends. Never leaves your wallet.</li>
                <li><b>Full viewing capability</b> — decrypts your notes to show balance and history. Stays local.</li>
                <li><b>Receiving identity</b> — the address you share (it starts with <code>gnr1</code> on mainnet). It never appears on chain.</li>
              </ul>
              <p>
                The wallet file is encrypted with Argon2id and XChaCha20-Poly1305, and it locks after five minutes without input. Lose the password but keep the phrase, and you can restore. Lose both, and no one — by design — can recover the funds.
              </p>

              <h2 id="pow">Proof of work</h2>
              <p>
                Blocks are produced by miners competing to find a RandomX hash below a target. RandomX runs a random program in a virtual machine that suits the large caches and branch predictors of general-purpose CPUs, so a laptop can take part.
              </p>
              <p>
                The chain everyone follows is the valid one with the most accumulated work. Difficulty adjusts every block so that blocks arrive about every 60 seconds no matter how many miners join. Read the exact formulas on the <A href="/mining">Mining</A> page.
              </p>

              <h2 id="supply">A finite supply</h2>
              <p>
                There will never be more than 21,000,000 GNR — and because the mainnet treasury share is burned, at most about 20,789,999.99995 can ever circulate. New coins enter only through block rewards on an exact integer schedule: blocks 1–500 pay a fixed early subsidy of 1,000 GNR each, and from block 501 the normal reward halves every four years. There was no premine, there is no mint transaction, no admin key and no way for anyone — including developers — to create more.
              </p>
              <p>
                The early subsidy goes to whoever mines those first blocks. At launch the only known miner is run by the project operator, so the operator may receive most or all of those 500,000 GNR (about 2.38% of the cap) and early holdings may be concentrated. See the <A href="/mining">Mining</A> page for the full disclosure.
              </p>
              <p>
                Wait — if amounts are hidden, how can anyone audit the cap? Because issuance is the one thing that is public: miner and treasury rewards and all fees are visible numbers in every block, and the proofs guarantee that shielded transfers can neither create nor destroy value.
              </p>

              <h2 id="fees">Fees</h2>
              <p>
                Every ordinary transfer pays a positive, public fee to the miner who includes it — at least 0.00001 GNR. The wallet suggests Economy, Standard and Priority at 1×, 2× and 5× the minimum. Miners order transactions by fee per byte. Fees move existing value; they are not new issuance.
              </p>

              <h2 id="finality">Confirmations and finality</h2>
              <p>
                A sent payment waits in the mempool until a miner includes it in a block. Each further block buries it deeper; reversing it would require redoing all that work faster than the rest of the network. That is <b>probabilistic</b> finality — strong after several confirmations, but never instant. The wallet shows the exact confirmation count.
              </p>

              <h2 id="proofs">Payment proofs</h2>
              <p>
                Sometimes you need to prove a payment — to a merchant, an auditor or a court. The wallet can export a proof for a single output: whoever receives it learns that output's recipient and amount, and can check it against the chain. Nothing else is revealed: no spending key, no other outputs, no wallet-wide view key.
              </p>

              <h2 id="names">Usernames</h2>
              <p>
                Instead of a long address you can share <code>@name</code>. Names live on the blockchain: the first valid registration wins, names never expire, and every node derives the same table. The price is burned (1,000 GNR for 3–4 letters, 100 for 5–6, 10 for 7+), which makes squatting expensive. A name points to a separate receiving address of your wallet, and payments to it stay shielded. Check any name on the <A href="/name">Names</A> page.
              </p>

              <h2 id="limits">The limits of privacy</h2>
              <Callout warn>
                Cryptography protects transaction contents. It does not protect against malware on your computer, copied seeds, weak passwords, screenshots or clipboard history, transaction timing, a small anonymity set, or an adversary who can watch both ends of a Tor circuit. Tor-only mode (the default for new installations) hides your IP address from peers; Direct and Hybrid modes do not. Use a trusted computer and keep your recovery phrase offline. More on <A href="/privacy">Tor &amp; network privacy</A>.
              </Callout>
              <p class="mt-3">Honest limits are part of the design. Read the full <A href="/security">threat model</A>.</p>

              <h2 id="glossary">Glossary</h2>
              <div class="kv" style={{ 'grid-template-columns': '200px minmax(0,1fr)' }}>
                <For each={GLOSSARY}>
                  {([k, v]) => (
                    <>
                      <div class="k strong">{k}</div>
                      <div class="v" style={{ 'font-family': 'var(--font-sans)', color: 'var(--text)' }}>{v}</div>
                    </>
                  )}
                </For>
              </div>
            </article>
          </div>
        </div>
      </section>
    </div>
  );
}
