# Security review of Genoryn Krypton 1.0.0 and 1.0.1 (fixed in 1.0.2)

*November 2026. Internal, AI-assisted end-to-end review of the whole repository and of the
live mainnet. It is not a substitute for an independent audit by a specialised firm.*

## Scope

Every crate and application: consensus (`krypton-core`), chain storage and fork choice
(`krypton-store`), the spend circuit (`krypton-zk`), proof of work (`krypton-pow`),
cryptography (`krypton-crypto`), peer-to-peer transport (`krypton-p2p`), light client
(`krypton-light`), wallet engine (`krypton-wallet-core`), node, miner, desktop and CLI
wallets, release tooling, deployment files and the public node at `genoryn.io`.

## Result in one paragraph

The cryptographic core is sound: no flaw was found in the STARK spend circuit, the
emission schedule, the reward accounting or the post-quantum primitives. The serious
problems were in **how nodes store and recover from bad data**: two of them let anyone
split nodes off the network at no cost. Both are fixed in 1.0.2, reproduced against 1.0.0
by regression tests, and nodes repair any damage automatically on upgrade.
**1.0.2 changes no consensus rule**: every block valid under 1.0.0 or 1.0.1 is valid under
1.0.2 and vice versa, the database format is unchanged, and all versions interoperate.
1.0.1 (dashboard and automatic updates) carries the same node issues as 1.0.0.

## Findings

| ID | Severity | Finding | Status |
|---|---|---|---|
| C-1 | Critical | Relaying a real header with an altered body made nodes reject the real block forever | Fixed |
| C-2 | Critical | Undoing a block without new notes deleted a still-valid anchor root (node-local consensus split) | Fixed + auto-repair |
| H-1 | High | A storage error marked a valid block invalid permanently | Fixed |
| H-2 | High | A reorganization could drop the current chain and then fail on a missing body | Fixed |
| H-3 | High | Wrong "invalid" marks survived restarts | Fixed (one-time reset) |
| H-4 | High | Transactions with failing proofs were never penalised (CPU exhaustion) | Fixed |
| H-5 | High | Unbounded per-peer send queue (memory exhaustion) | Fixed |
| H-6 | High | Light wallets trusted one server's claimed chain work | Mitigated, see recommendations |
| M-1 | Medium | Payment proofs did not bind the named recipient to the paid owner | Fixed |
| M-2 | Medium | Mining jobs could hold gigabytes on a public node | Fixed |
| M-3 | Medium | Unlimited writes to the public address directory | Fixed |
| M-4 | Medium | Address book could be flooded to isolate a node (eclipse) | Fixed |
| M-5 | Medium | Unbounded inbound handshakes; an `accept` error stopped P2P listening | Fixed |
| M-6 | Medium | A send that never confirmed locked its notes forever | Fixed |
| M-7 | Medium | Repository web config pointed the public API at the testnet port | Fixed |
| M-8 | Medium | Transactions in reorganized-away blocks were lost | Fixed |
| M-9 | Medium | The 1.0.1 updater could be served an old, validly signed installer (rollback) | Fixed |
| L-1 | Low | `meets_difficulty` could overflow for difficulties ≥ 2^64 | Fixed |
| L-2 | Low | Confirmation counts could underflow during reorganizations | Fixed |
| L-3 | Low | One compact-block response could reach hundreds of megabytes | Fixed |
| L-4 | Low | Download links hard-code the version | Updated for 1.0.2 |

### C-1: block-body substitution poisoned valid headers

`process_block` stored any block whose header had valid proof of work, then marked the
header **invalid** if the body failed validation. The block id is the header hash, so a
peer could copy a freshly mined header, drop one share or transaction, and relay it first.
Receiving nodes rejected the genuine block from then on ("block previously found invalid"),
rejected every descendant ("parent is invalid") and banned the honest peers relaying them.
No proof of work was needed.

*Fix:* the body is checked against the header's commitments (`tx_root`, `share_root`,
nullifier and output digests) **before anything is stored**
(`consensus::check_body_commitments`). These commitments fix the body uniquely, so any later
failure is a genuine property of the block. Test:
`relayed_block_with_altered_body_does_not_poison_its_header`.

### C-2: anchor roots lost after reorganizations

A transfer may anchor to the note-tree root of any of the last 2,880 main-chain blocks. The
`roots` index maps each root to its latest height. Blocks that add no notes (most blocks)
repeat their parent's root; when such a block was disconnected, its root was **deleted**
although the parent still had it. A node that went through an ordinary one-block
reorganization then rejected valid transfers that other nodes accepted, and marked the next
block containing one invalid: it left the network. *Fix:* the root is moved to the parent's
height instead of deleted; on every start the node rewrites the roots of the anchor window
from the main chain. Test: `undoing_a_block_without_new_leaves_keeps_the_anchor`.

### H-1 to H-3: recovery

* `mark_invalid` ran for storage errors too; now only for consensus errors.
* `reorganize` disconnected the current chain before checking that every branch body was
  present; it now checks first and leaves the chain untouched (`Orphan`) otherwise.
* Because 1.0.0 and 1.0.1 nodes may hold wrong invalid marks from C-1 and C-2, 1.0.2 clears
  them once on upgrade (`repair/invalid-reset-v1`); bodies are fetched and fully validated
  again.
  Test: `startup_repair_restores_anchor_roots_and_clears_wrong_invalid_marks`.

### H-4: invalid-proof spam

Peers sending transactions whose proof fails were only logged. Verifying a proof costs about
2 ms, so a few peers could keep a node's CPU busy. Structural checks and state checks now run
before the proof; malformed transactions and failing proofs get the peer banned (honest
nodes only relay transactions whose proofs they verified).

### H-5: per-peer memory

Each peer had a 1,024-frame send queue with frames up to 8 MiB. A peer requesting blocks
without reading could make the node buffer gigabytes. Queued bytes are now counted: bulk
replies pause above 32 MiB and a peer above 128 MiB is disconnected; the reader notices
immediately instead of after 10 minutes.

### H-6: light-client trust (design)

FlyClient samples are chosen by **height** and each is checked against the difficulty the
server itself claims; total work is never compared with anything. A malicious server can
therefore present a fabricated low-difficulty chain from the genuine genesis. The desktop
wallet also read from a single node (only the CLI cross-checked several).
*Mitigation in 1.0.2:* the desktop wallet cross-checks all configured nodes like the CLI.
*Remaining:* see recommendations 1 and 2.

### M-1: payment-proof recipient

A proof names a recipient short address, but only the owner tag is bound to the note. A
payer could prove a payment to themselves while naming someone else, and the wallets showed
"paid to the recipient above". Now `recipient_verified` is true only when the named address
is the verifier's own or the published address it hashes to and pays exactly that owner; a
proof naming the verifier's wallet but paying someone else is rejected as forged.

### Other fixes

* **M-2** mining jobs are bounded by the transactions they hold; an expired job no longer
  makes the template handler panic.
* **M-3** the address directory accepts at most 120 new entries a minute, never rewrites
  entries and stores the canonical encoding.
* **M-4** seeds are dialed first while a node has fewer than two outbound peers,
  unreachable addresses are forgotten, and targets being dialed are not dialed twice.
* **M-5** at most 64 inbound handshakes in progress (and `max_per_ip` per address); the
  listener survives `accept` errors such as file-descriptor exhaustion.
* **M-6** a send that passes its expiry height without confirming is removed from history
  and its notes become spendable again (`expire_pending`).
* **M-7** `deploy/caddy/genoryn.io.caddy` proxied to `127.0.0.1:31334` (testnet); the live
  server was configured by hand, so redeploying from the repository would have pointed
  every mainnet wallet at testnet. Now `30334`; `setup-server.sh` defaults to mainnet.
* **M-8** transactions of disconnected blocks return to the mempool if still valid.
* **M-9** the 1.0.1 updater accepted any installer listed in a validly signed
  `SHA256SUMS`, under any tag, and parsed tags loosely (`v9.0.0-/../x` passed and was used
  in a path). Someone controlling the GitHub account could publish an old signed release
  under a new tag and roll wallets back. Tags must now be exactly `vX.Y.Z` and the installer
  must be `GenorynKrypton-X.Y.Z-Windows-x64-Setup.exe`, a name the signature covers.
* **L-1** `meets_difficulty` now multiplies exactly (384-bit); results are identical for
  every difficulty below 2^64, so consensus is unchanged. Differential test against a
  reference big-integer implementation over 20,000 random cases.

## What was checked and found correct

* **Spend circuit:** position bits equal the Merkle path directions and the nullifier
  position (no double spend through position aliasing); 60-bit range checks on all four
  values and a balance equation that cannot wrap the 64-bit field; dummy inputs are forced to
  zero value and use a separate nullifier domain; the verifier accepts only the exact proof
  options and trace shape, and catches panics.
* **Consensus:** 21,000,000 GNR cap and halving schedule, LWMA difficulty with monotonic
  timestamps, median-time-past and future-drift limits, canonical encodings, Merkle roots that
  commit to the count (no duplicate-transaction ambiguity), nullifier uniqueness inside blocks
  and against the chain, `mint = 0`, share window, share weights capped at 4× difficulty
  (no arbitrage across heights), credits independent of other shares, exact supply
  conservation, deterministic payouts.
* **Cryptography:** hybrid KEM secret binds both secrets, the ciphertext and the public key;
  one-time note keys make nonce reuse impossible; wallet files use Argon2id (256 MiB) with
  parameters authenticated as associated data; release signatures require both ML-DSA-87 and
  SLH-DSA-SHAKE-256s.
* **Desktop wallet:** served on 127.0.0.1 only, every call needs a session token in a custom
  header (blocks CSRF and DNS rebinding), navigation is restricted to project links.
* **Live mainnet at review time:** height 30, two miners, 58.6 s average block time, reward
  pool 39.27 GNR, which matches the 1/8-per-block drain model.

## Recommendations

1. **Light clients:** sample by cumulative work instead of height (true FlyClient), ship a
   minimum-chain-work value with every release, and configure at least two independently
   operated nodes by default. This needs a protocol extension and should be done before
   light wallets hold significant value.
2. **Checkpoints:** publish a recent block hash in each release so new nodes reject
   low-difficulty side branches from the distant past.
3. **Payout throughput:** at most 64 payouts per block pays about 23,000 accounts per
   360-block interval; plan a change before the number of active miners approaches it.
4. **P2P authentication:** the transport is encrypted but not authenticated (like Bitcoin's
   BIP 324). Integrity comes from validation; consider optional peer identities for
   operator-to-operator links.
5. **Independent audit:** commission an external review of the STARK zero-knowledge masking
   and the spend AIR before large value depends on them.
6. **Operations:** keep the release signing key and recovery phrases offline and encrypted,
   never paste access tokens into chats or scripts, and never force-push or delete releases
   on the public repository.
