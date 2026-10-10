# Light Wallet: architecture review and implementation plan

Status: the full plan was approved on 2026-10-10. **Phase A is implemented**
(see "Progress" below); the rest of the plan follows. Nothing here changes
consensus, the peer protocol or wallet files.

## Progress

- **L0 (shared Tor):** no separate crate was needed. The wallet CLI already links
  the node library, so the light wallet uses the same built-in client and SOCKS
  bridge (`genoryn::light::LightTor`), with its own state beside the wallet.
- **L1 (verification):** stronger than planned. The wallet already validated a
  downloaded chain fully with the consensus code (`Wallet::scan` →
  `validate_chain`, RandomX work and Halo 2 proofs included); the light wallet
  uses exactly that, so no header-only rules were written.
- **L2 (Phase A sync):** `genoryn::light::sync` over Tor from several sources,
  highest tip first, falling back when a chain fails validation, with per-source
  reports (`used`, `agrees`, `behind`, `different branch`, `rejected`).
- **L3 (submission):** `POST /v1/relay` (new, public RPC) through a source other
  than the sync source. Not yet: keeping a payment that no source accepted and
  resubmitting it later.
- **L4 (cross-check):** tips of all sources compared with the validated chain.
  Not yet: automatic retries against a source that withholds.
- **L5 (UI):** wallet Network page toggle and source list; no owned node is
  started in light mode; sync and send only.
- **L6 (compact blocks), L7 (review):** not started. Each operation currently
  downloads and validates the whole chain; a persisted validated chain is the
  next step for longer chains.
- Nodes serve their public RPC through the built-in onion service
  (`--public-rpc --onion-service`).
- Evidence: `apps/genoryn/tests/light_wallet.rs` (Tor-routed source selection,
  invalid chain rejection, relay through another source) and a
  live run over the Tor network in which a light wallet with its own built-in Tor
  synced from two onion RPCs, received a payment and sent one that was mined.

## Terms

- **Full Wallet**: today's desktop wallet with the node it starts (also the
  existing *GENORYN Lite* edition, which is a full node without the miner).
  It validates everything itself; its Tor is the node's built-in client.
- **Light Wallet**: a wallet without a local chain. It reads chain data from
  remote nodes over Tor, keeps keys and scanning on the device, and verifies
  what it can without the full chain. The name must not be confused with
  *GENORYN Lite*.

## What the remote node can and cannot learn or do

| Remote node can | Light Wallet's defence |
|---|---|
| See that someone syncs and when | Tor (onion nodes preferred); identical requests for every user |
| See a submitted transaction first | Separate circuit and node for submission, random delay; shielded content |
| Serve a fake chain | Header linkage, difficulty rules and RandomX PoW checked on device; release checkpoints |
| Hide or alter outputs | Wallet rebuilds the note commitment tree and compares with each header's `notes_root` (to be confirmed: root after the block) |
| Hide spends | Wallet tracks its own spends; a node refusing a valid spend is visible; cross-check with a second node |
| Withhold new blocks, censor submissions | At least two independent nodes; compare tips; submit elsewhere |
| Learn addresses or balances | Never: no viewing keys, addresses or note-specific queries leave the device |

The Light Wallet trusts remote nodes for availability, not for validity of what
it can check. It cannot fully validate proofs of other users' transactions;
that remains the Full Wallet's advantage and is shown in the UI.

## Architecture

1. **Shared Tor module.** Move `net::embedded`, `net::bridge` and the Tor
   status types into a `genoryn-tor` crate used by `genorynd` and the Light
   Wallet. One Arti runtime per process. The Full Wallet keeps using its node's
   client (no second Tor). Isolation groups in the Light Wallet: one per remote
   node for sync, a fresh one per submission, one for update checks; sync and
   submission never share a circuit.
2. **Sync engine** in `genoryn-wallet-core`: header verification (linkage,
   difficulty, RandomX in light mode), incremental note commitment tree with
   local witnesses, trial decryption with the existing scanner, reorg handling
   by header work.
3. **Data source**, in two phases:
   - Phase A uses the existing public `GET /v1/blocks` over Tor. Works with every
     deployed node including v0.4/v0.5; costs full block bandwidth.
   - Phase B adds a versioned RPC extension serving compact blocks (header plus
     per action: nullifier, `cmx`, ephemeral key, leading ciphertext bytes
     needed for compact trial decryption). An RPC addition, not a consensus or
     peer-protocol change; the wallet falls back to Phase A when absent.
4. **Submission**: proofs built on the device; `POST /v1/transactions` (public
   RPC) through a different node and circuit than sync; the exact bytes are kept
   and resubmitted with backoff until mined or expired, like the node's
   delivery rules (no new spend, no double debit).
5. **Keys and storage**: same encrypted wallet file format (Argon2id,
   XChaCha20-Poly1305) and recovery phrase as the Full Wallet; headers, tree
   state and scan cache encrypted the same way.
6. **Node selection**: user-configured nodes plus a published list of onion
   nodes; pinned onion addresses; tor-only by default with no clearnet
   fallback; at least two nodes for cross-checks.

## Decisions needed before implementation

1. Approve the Light Wallet at all, and whether Phase A alone is acceptable
   as a first release.
2. RandomX verification on clients: light mode needs about 256 MB per seed
   epoch. Acceptable on desktop; mobile may need checkpoints plus sampled
   verification, which weakens the fake-chain defence.
3. Who publishes and signs the remote onion node list.
4. Whether nodes should serve compact blocks by default (Phase B).

## Implementation plan (after approval)

| Stage | Work | Exit criteria |
|---|---|---|
| L0 | Extract `genoryn-tor`; no behaviour change | Full test suite and live Tor evidence unchanged |
| L1 | Header chain verifier and commitment-tree check | Unit tests; fake-chain and altered-output mocks rejected |
| L2 | Phase A sync over Tor with scanning and witnesses | Balance equals the Full Wallet's on the same devnet chain |
| L3 | Submission path with persistence and resubmission | Live Tor devnet payment both ways; outage and restart tests |
| L4 | Multi-node cross-check and node selection | Withholding/censoring node mocks detected |
| L5 | Light Wallet UI mode with honest trust labels | GUI smoke and native control tests |
| L6 | Phase B compact blocks (node RPC + client) | Mixed-version tests; fuzzed parser; bandwidth measured |
| L7 | Evidence, documentation, independent review request | Measured sync time, bandwidth, memory and proving time |

Performance numbers (first sync time, bandwidth per block, proving time,
memory) will be measured during L2-L6; none are claimed here.
