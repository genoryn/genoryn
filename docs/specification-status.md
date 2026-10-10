# Specification traceability

This map covers the master GENORYN implementation requirements. Implementation,
local evidence, clean cross-platform verification and production release approval
are separate statements. The project remains an unaudited development network.

The clean `cc280b5` baseline passed all four Windows/Linux, audit and fuzz jobs.
Its actual native lifecycle, thirteen-check labs, testnet work, 50K identities,
package checks and matching vectors are recorded in
[the artifact evidence](testing/evidence/native-artifacts-cc280.json).
The Windows ZIP was independently downloaded; the Linux ZIP was verified inside
CI, with its small report and actual runtime capture inspected separately.
Subsequent changes require their own exact-commit validation.

| Required area | Implementation | Evidence and limits |
| --- | --- | --- |
| Independent L1 and genesis | Own genesis, network identifiers, canonical blocks, cumulative validated work | Cross-platform golden vectors; real independently validating peers |
| Monetary invariants | Integer 21M cap, 100K bootstrap, 210K treasury, halving and height vesting | Exhaustive interval/boundary tests and real supply comparison; ADR 0003 |
| CPU PoW/difficulty | Genuine RandomX and bounded integer retarget | Mined devnet and testnet blocks, spike/drop/time simulations |
| Mandatory shielded payments | Corrected Orchard 0.16/Ironwood-v3 notes, commitments, nullifiers, proofs and encrypted memos | Real private payment, unrelated scan, spend, duplicate/proof rejection; integration has no inherited audit |
| Durable chain and reorg | SQLite WAL/FULL, integrity/schema/network checks, branch replay and nullifier reapplication | Abrupt restart, competing work, five/ten-block rollback and remine; full replay cost remains |
| P2P and discovery | TCP, explicit/DNS/cache/exchange discovery, bounded randomized selection/retries, actual rotation, reserved outbound slots, outbound netgroup diversity | Independently closed stale-cache bug, real default timer; no Sybil/anonymity guarantee |
| Network privacy (Tor) | Direct/hybrid/tor-only modes with a built-in Tor client (Arti) or an operator-run Tor daemon, remote name resolution, v3 onion services, fail-closed transport policy, address exchange v2, one-hop stem relay with outage-proof local delivery, wallet controls | 15 local-stand-in and 5 adversarial relay integration tests; opt-in live runs over the real Tor network on Windows (built-in) and Linux (built-in with outage and DNS checks; external C Tor); mixed v0.4.0/v0.5.0/current devnet; not CI, no anonymity guarantee, no independent review; [network privacy](network-privacy.md) |
| RPC and resource policy | Versioned public reads, authenticated loopback writes, node-wide request admission, bounded connection/mempool policy | Native policy lab and per-route shared-budget regression; consensus maxima are not operator knobs |
| Discovery SDK | Caller-pinned chain/genesis, signed manifests, DNS hints, parallel health/latency checks and failover | Real loopback HTTP tests; no invented production bootstrap service or browser TCP claim |
| Local encrypted wallet | BIP39/ZIP32, Argon2id/XChaCha20, local validated scan/history, recovery and zeroization | Actual seed recovery; account and receiver-index recovery; OS compromise remains outside protection |
| Subaccounts and receivers | CLI derives encrypted account files and diversified receivers | Same seed plus account index recovers keys; keep used account numbers with recovery instructions |
| Selective payment proof | Existing cryptographic backend plus CLI and native export/import/verification | Real-note disclosure review, explicit save/copy, locked verification, cancellation and malformed-input checks |
| Username payments | Signed canonical records, ownership/expiry/replay checks, local pins, separate SQLite registry | Real private @name payment; 50K distinct identities measured; local resolver is not decentralized global registration |
| Native wallet | Eight actual monochrome views, create/restore/lock/send/receive/history/proofs/mining/node/network/settings | Real Linux interaction and Windows D3D12 rendered frames; no fabricated balance or peers |
| Node/miner controls | Explicit start/stop/restart of owned processes, bounded log view, optional TOML, measured metrics | Ownership and late-result tests; fresh local UI lifecycle checks; no automatic mining |
| Fee experience | Shared fixed policy quotes, editable preview fee and actual fee-per-byte miner ordering | Exact integer comparator and real two-payment ordering; no confirmation-time prediction |
| Reference display | Optional user-entered USD/GNR value, checked integer arithmetic | Display only, rounded down to cents; no price feed, financial valuation or consensus role |
| Capacity and performance | Indexed bounded registry, 50K identities, bounded RPC clients, real crypto/DB/miner/scan/relay timings | Registered identities are not TPS; 64 tx/60s is a provisional ~1.07 TPS ceiling; long-history scale is unproven |
| Test command and CI | Shared portable practical runner for Bash/PowerShell; release extras cover timer/capacity/audit/fuzz/package | Failures and incomplete runs cannot be marked passed; native runtime prerequisites fail visibly |
| Packaging and documentation | Pinned compiler/lockfile, exact source manifest, checksums, native notices, real screenshots and protocol/ADR docs | Clean baseline archives verified; checksums are not publisher signatures or legal certification |

## Review findings and unresolved verification

Independent review identified and closed the stale-peer-cache starvation and
substantive notice gaps. The feature review additionally identified per-route
RPC accounting, late log reopening and editor undo retaining dismissed text.
The source now uses a shared node RPC permit budget, generation-bound log
results, and sensitive editors without retained undo history. Focused regressions
must pass again on the final source before a release statement.

One separately held additional RPC trust-boundary regression remains unexecuted;
existing ordinary valid-node/payment tests do not substitute for it. No external
cryptographic/consensus audit, mixed Windows/Linux live LAN test, production
signing ceremony or bit-for-bit independent-builder reproduction is claimed.

## Explicitly later and gated work

The specification explicitly defers its future ecosystem list: macOS/mobile and
hardware wallets, full watch-only wallets, multisig, private merchant invoices,
public websites/explorer/dashboard, atomic swaps, decentralized pools, I2P,
exchange integrations, market price feeds and merchant APIs (Tor support now
exists; see the table above). Required building
blocks such as encrypted memos, payment requests and the native discovery SDK
are present now; the later products are not labeled implemented.

Pruning and privacy-preserving light clients are explicitly allowed after the
full-node desktop version. No address-indexed balance service is used as a
shortcut. Future network transports must preserve the same consensus validation.

Phase 10 VPS tooling is explicitly gated on the local release gate passing.
Its systemd/deployment deliverables remain planned behind that gate. Actual VPS
access, public infrastructure, domain ownership and server deployment are not
part of the currently executed local network.

The mainnet has been running since 10 October 2026 (release 0.4.0) with a
finalized genesis and a burned treasury. External audit, signed binaries and
reviewed bootstrap configuration are still missing; they are not placeholders
to fill with test secrets.
