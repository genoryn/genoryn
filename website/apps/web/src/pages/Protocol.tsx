import { A } from '@solidjs/router';
import { EMISSION_DOC_URL, REPO_URL } from '@genoryn/shared';
import { MathTex } from '~/components/MathTex';
import { Toc } from '~/components/Toc';
import { CodeBlock, ExtLink, PageHero } from '~/components/ui';

const TOC: Array<[string, string]> = [
  ['overview', 'Overview'],
  ['consensus', 'Consensus'],
  ['header', 'Block header'],
  ['body', 'Block body'],
  ['transactions', 'Transactions'],
  ['encoding', 'Encoding & hashing'],
  ['state', 'State & maturity'],
  ['networking', 'Peer protocol'],
  ['names', 'On-chain usernames'],
  ['rpc', 'RPC'],
  ['versions', 'Versions & upgrades'],
];

export default function Protocol() {
  return (
    <div class="editorial">
      <PageHero eyebrow="protocol.genoryn.dev" title={<>The protocol, <em>in full.</em></>} lead="GENORYN Mainnet 1.0, consensus v1: heaviest-work chain selection, RandomX proof of work, Ironwood-v3 shielded bundles with Halo 2 proofs, and canonical integer encodings." origin="Specification: docs/protocol in the repository" />
      <section class="section tight">
        <div class="container">
          <div class="with-toc">
            <Toc items={TOC} />
            <article class="prose">
              <h2 id="overview">Overview</h2>
              <div class="table-wrap">
                <table class="data">
                  <tbody>
                    <tr><td class="strong">Consensus</td><td>Greatest sum of verified per-block difficulty; equal work keeps the current branch</td></tr>
                    <tr><td class="strong">Proof of work</td><td>RandomX-v1 (randomx-rs 1.6.0), light-mode verification ≈ 256 MiB</td></tr>
                    <tr><td class="strong">Shielded protocol</td><td>Ironwood-v3 bundles, Halo 2 proofs, 32-level note commitment tree</td></tr>
                    <tr><td class="strong">Block target</td><td>60 seconds, per-block LWMA-style retarget</td></tr>
                    <tr><td class="strong">Block limits</td><td>2,000,000 bytes; ≤ 64 ordinary transfers plus miner and treasury issuance</td></tr>
                    <tr><td class="strong">Transaction limits</td><td>200,000 bytes; 2–16 actions (coinbase classes exactly 1)</td></tr>
                    <tr><td class="strong">Supply</td><td>21,000,000 GNR in u128 gori (1 GNR = 10¹² gori)</td></tr>
                    <tr><td class="strong">Emission</td><td>Blocks 1–500: 1,000 GNR early subsidy each (500,000 GNR, ~2.38%; fees extra). From block 501: normal subsidy of 4.825437595129 GNR halving every 2,102,400 blocks, plus 1 GNR treasury per block for heights 501–210,500, burned on mainnet. <ExtLink href={EMISSION_DOC_URL}>Monetary Policy &amp; Emission Schedule</ExtLink></td></tr>
                    <tr><td class="strong">Genesis message</td><td>"Privacy is not a feature. Privacy is the default."</td></tr>
                  </tbody>
                </table>
              </div>

              <h2 id="consensus">Consensus</h2>
              <p>
                Nodes select the valid branch with the greatest cumulative work, where work is the sum of each block's verified integer difficulty. Height 1 follows a deterministic, network-specific, zero-issuance genesis record. The Mainnet 1.0 genesis (genoryn-mainnet-1.0, tag 2026-10-10-mainnet-1.0, hash 2b27dda9…6b2e) was finalized in release 1.0.0; every program defaults to Mainnet 1.0, while devnet and testnet remain for development labs. The earlier network genoryn-mainnet-v1 (releases 0.4–0.6) is retired and its coins do not carry over.
              </p>
              <MathTex block tex={'\\mathrm{work}(\\text{branch}) = \\sum_{b \\in \\text{branch}} D_b \\qquad \\text{best} = \\arg\\max_{\\text{valid branches}} \\mathrm{work}'} />
              <p>
                A block's timestamp must exceed the median of up to 11 preceding timestamps and be at most 120 seconds ahead of the node's UTC clock. Reorganisations replay maturity, anchors and nullifiers from the competing branch and then reconsider mempool transactions.
              </p>
              <MathTex block tex={'\\text{genesis} = \\mathrm{H}_{\\text{GENORYN/genesis/v1}}\\big(\\texttt{chain\\_id | message | GNR | 21000000 | 10^{12} | RandomX-v1 | Ironwood-v3 | early:500x1000 | tag}\\big)'} />

              <h2 id="header">Block header</h2>
              <p>The header commits to every consensus-relevant field. Its hash is the block hash.</p>
              <CodeBlock
                lang="rust"
                title="Header (consensus v1)"
                code={`struct Header {
    version: u16,          // 1
    network: Network,      // Devnet | Testnet | Mainnet (u32 tag)
    height: u64,
    previous: [u8; 32],    // parent block hash
    timestamp: u64,        // Unix seconds
    difficulty: u64,       // integer difficulty for this block
    pow_seed: [u8; 32],    // RandomX epoch key
    nonce: u64,
    body_hash: [u8; 32],   // H("GENORYN/body/v1", body)
    notes_root: [u8; 32],  // matured note-commitment root
    nullifiers_root: [u8; 32],
}`}
              />

              <h2 id="body">Block body</h2>
              <CodeBlock
                lang="rust"
                title="Body"
                code={`struct Body {
    miner: ShieldedTx,              // kind 1, exact scheduled issuance
    treasury: Option<ShieldedTx>,   // kind 2, heights 501..=210,500
    transactions: Vec<ShieldedTx>,  // kind 0, at most 64
}`}
              />
              <p>
                Only coinbase classes may have a negative value balance. Their exact amounts, height, parent, network, spend-disabled flags and treasury receiver are validated. All ordinary transfers have positive public fees and no transparent value output.
              </p>

              <h2 id="transactions">Transactions</h2>
              <p>
                Each transaction contains a signed context and one Ironwood-v3 bundle. The context binds protocol version, network, kind, height, expiry and a nonce; cross-network signature reuse is rejected. Transfers use kind 0, height 0, a random nonce and a bounded expiry. Issuance uses kind 1 or 2, its actual height, no expiry and the previous block hash as nonce.
              </p>
              <CodeBlock
                lang="rust"
                title="Shielded transaction"
                code={`struct ShieldedTx { context: TxContext, bundle: BundleWire }
struct TxContext { protocol: u16, network: Network, kind: u8, height: u64, expires: u64, nonce: [u8; 32] }
struct BundleWire {
    actions: Vec<ActionWire>, flags: u8,
    value_balance: i64,      // public fee (>0) or issuance (<0)
    anchor: [u8; 32], proof: Vec<u8>, binding_signature: Vec<u8>,
}
struct ActionWire {
    nullifier: [u8; 32], rk: [u8; 32], cmx: [u8; 32], ephemeral_key: [u8; 32],
    encrypted_note: Vec<u8>, outgoing_ciphertext: Vec<u8>,
    value_commitment: [u8; 32], signature: Vec<u8>,
}`}
              />
              <p>
                There is no username, ordinary receiver, transfer amount or plaintext memo field anywhere in the wire transaction. Validation checks all signatures, the proof, flag and amount constraints, a recognized mature anchor, duplicate nullifiers and chain/mempool conflicts.
              </p>

              <h2 id="encoding">Encoding and hashing</h2>
              <p>
                Consensus objects use canonical bincode 1 with fixed-width little-endian integers. Decoding rejects trailing bytes and must round-trip byte for byte. Every hash is SHA-256 with explicit domain separation and length prefixes:
              </p>
              <MathTex block tex={'\\mathrm{H}_d(m) = \\mathrm{SHA256}\\big(\\mathrm{u64_{le}}(|d|) \\,\\|\\, d \\,\\|\\, \\mathrm{u64_{le}}(|m|) \\,\\|\\, m\\big)'} />
              <div class="table-wrap">
                <table class="data">
                  <thead><tr><th>Object</th><th>Domain</th><th>Preimage</th></tr></thead>
                  <tbody>
                    <tr><td>Block hash</td><td class="mono">GENORYN/block/v1</td><td>encoded header</td></tr>
                    <tr><td>Body commitment</td><td class="mono">GENORYN/body/v1</td><td>encoded body</td></tr>
                    <tr><td>Transaction ID</td><td class="mono">GENORYN/transaction/v1</td><td>entire authorized encoded transaction</td></tr>
                    <tr><td>Genesis</td><td class="mono">GENORYN/genesis/v1</td><td>canonical genesis preimage</td></tr>
                  </tbody>
                </table>
              </div>
              <p class="mt-2">
                The genoryn.io explorer re-implements exactly this encoding in TypeScript to derive block hashes and transaction IDs, and checks its result against the node's best hash on every sync.
              </p>

              <h2 id="state">State and maturity</h2>
              <ul>
                <li>Mature note commitments form the upstream 32-level tree. Only recent admitted roots (a 120-block window) may anchor transfers.</li>
                <li>Mining outputs wait 60 blocks on mainnet and testnet (2 on the devnet lab) before they enter spendable trees.</li>
                <li>On mainnet the treasury receiver is the burn address: a full viewing key whose <code>ak</code> is a hash-to-curve point with no known discrete logarithm, so no spend authorization can ever exist. Test networks keep a public test key with the year-10–19 vesting schedule.</li>
                <li>Nullifiers are unique across all accepted actions.</li>
                <li>SQLite (WAL) atomically stores blocks and the best-tip change; a process lock prevents two writers; schema, network and genesis checks reject incompatible databases.</li>
              </ul>

              <h2 id="networking">Peer protocol</h2>
              <p>
                TCP frames start with four-byte network magic (<code>GNR</code> + 0x10 + network id; Mainnet 1.0 is <code>0x474e5213</code>), a little-endian u32 length and a canonical typed message; frames over 2,100,000 bytes are rejected before allocation. The handshake validates version, network, genesis and peer identity.
              </p>
              <ul>
                <li>Messages: block locators, one-block responses, announcements, shielded-transaction relay, peer exchange, ping/pong.</li>
                <li>Limits: 16 connections, 8 outbound; outbound peers rotate after 15–30 minutes while another validated peer remains.</li>
                <li>Discovery: explicit peers, a 128-entry persistent cache, peer exchange, operator DNS seeds and the published seed list. Seeds are never consensus authorities.</li>
                <li>Misbehaviour earns strikes, disconnection and temporary bans.</li>
                <li>Built-in Tor with Direct, Hybrid and Tor-only modes, v3 onion services, AddrV2/Announce extension messages for onion addresses (negotiated, so v1 peers are unaffected), and a one-hop stem for locally created payments. See <A href="/privacy">Tor &amp; privacy</A>.</li>
              </ul>
              <p>Run your own node: <A href="/nodes">Nodes</A>.</p>

              <h2 id="names">On-chain usernames</h2>
              <p>
                Names are derived state computed while applying each fully validated block. A registration is an ordinary kind-0 transfer with one output paying at least the price to the burn address; its 512-byte memo carries a signed record (magic <code>GNRNAME\x01</code>, operation, name, 43-byte receiver, Ed25519 owner, sequence, signature). Names are not committed in headers, never make a block invalid, and are recomputed on reorganizations — so no activation height or network split is needed.
              </p>
              <p>Prices burned: 1,000 GNR (3–4 characters), 100 GNR (5–6), 10 GNR (7+); updating the receiver burns 1 GNR. Details on the <A href="/name">Names</A> page.</p>

              <h2 id="rpc">RPC</h2>
              <p>
                Versioned HTTP RPC (<code>/v1</code>). The administrative listener binds to loopback only and requires a capability token; the optional public listener is read-only. No endpoint returns a balance for an address, a wallet history, a rich list or a price. See the <A href="/api">API reference</A>.
              </p>

              <h2 id="versions">Versions and upgrades</h2>
              <p>
                Current release 1.0.0 (Mainnet 1.0). Versions: consensus 1, block 1, transaction 1, P2P 1, RPC /v1, wallet file 1, database schema 1, name record 1, payment URI 1 (experimental). Unsupported versions fail closed. Future consensus upgrades must publish a specification and activation height, review new proving keys and monetary invariants, generate cross-platform vectors, and require explicitly compatible software. There is no founder override and no remotely signed exception.
              </p>
              <p>
                Full source of truth: <ExtLink href={`${REPO_URL}/tree/main/docs/protocol`}>docs/protocol</ExtLink> and the <ExtLink href={`${REPO_URL}/tree/main/docs/adr`}>architecture decision records</ExtLink>.
              </p>
            </article>
          </div>
        </div>
      </section>
    </div>
  );
}
