# Development status

## 1.2.0 (merged to `main`, not yet released as binaries)

Wallet security; no consensus change. Evidence is local (Windows 11) unless a CI run is named.

| Feature | State | Evidence |
|---|---|---|
| Recovery phrase shown only after the password is typed again; masked, non-copyable, wiped after 60 s / focus loss / lock; waiting after wrong passwords; backup quiz | Implemented, verified locally | `recovery` GUI tests; devnet check `recovery_phrase_only_with_password_and_checked_password_change`; captures in `docs/images/wallet-1.2/` |
| Window kept out of screen capture while words are shown | Windows: implemented (`WDA_EXCLUDEFROMCAPTURE`, black on older Windows), the system call succeeds in local runs; Linux: not available, stated in the UI | `genoryn-screen-guard`; capture shows the reported state |
| Wallet file version 2 (Argon2id 256 MiB, authenticated parameters), checked password change | Implemented, verified locally | `keys` unit tests; devnet check |
| Stronger new passwords (12+ characters, strength estimate) | Implemented | `recovery` and `views_wallet` tests |

## 1.1.0 (merged to `main`, not yet released as binaries)

No consensus change; Mainnet 1.0 genesis, chain, supply and rules unchanged. Evidence is
local (Windows 11, Intel Core Ultra 9 285K) unless a CI run is named.

| Feature | State | Evidence |
|---|---|---|
| Payment Proof 2.0 (memo-committed key, challenge/response, outcomes) | Implemented, verified locally | `payment_proof_v2` real-chain test; devnet CLI check; GUI tests |
| Shared mining: PPLNS, VarDiff, share verification, ledger, payouts | Implemented, verified locally on devnet; **experimental**, custodial | `scripts/test/pool.py` (13 pool blocks, exact payouts, restart); pool unit/property tests on SQLite and real PostgreSQL 17 |
| Pool capacity | Measured on one machine | `docs/testing/evidence/pool-*-1.1.json` |
| Mainnet pool activation at 1150 | **Blocked by design**: the height passed before deployment; a future height needs explicit approval | `docs/testing/evidence/pool-activation-mainnet.txt` |
| My Full Node page | Implemented; Windows lifecycle test updated (CI runs it) | `windows_native_controls.py`, GUI tests, captures |
| Faster sync (paging, parallel pre-verification, snapshot) | Implemented, measured on the real mainnet chain | `wallet-sync-*-1.1.json`, `replay_bench` |
| Privacy & Security Center | Implemented, verified locally | `views_security` tests |
| Node IBD parallel validation | Not done (P2P path unchanged) | — |
| Native P2Pool, TLS inside `genoryn-pool`, signed releases | Not done | — |

## Mainnet 1.0 (release 1.0.0)

- **New network.** `genoryn-mainnet-1.0`, genesis `2b27dda9…6b2e`, magic `0x474e5213`,
  port 29710. The pre-1.0 mainnet (`genoryn-mainnet-v1`, `c5fc87d9…d708`) is retired;
  its data was archived and verified before its nodes were stopped.
- **Emission.** Blocks 1–500 pay 1,000 GNR each (500,000 GNR, the whole subsidy); from
  block 501 the normal reward 4.825437595129 GNR halves every 2,102,400 blocks, plus a
  burned 1 GNR treasury share for 210,000 blocks. Lifetime issuance 20,999,999.999950064 GNR
  ([emission](docs/protocol/emission.md)). Initial difficulty 150,000.
- **Wallet.** Quick mode (locally validated chain from remote onion sources, kept between
  syncs, fork choice by cumulative work), durable encrypted outbox, persistent mining
  history with Replaced rewards, honest balance states, wallet birthday, serialized Tor
  mode transitions, Explorer link and About section ([release notes](docs/releases/v1.0.0.md)).
- **Evidence.** Consensus suite with exact emission vectors and retired-network replay
  rejection; real mined fork-choice tests; real three-node devnet including Quick mode,
  lost acknowledgements and a reorganization; Quick mode over the public Tor network
  against both mainnet onion services.

## Earlier status (2026-10-10, pre-1.0)

- **The pre-1.0 mainnet** launched on 10 October 2026 with release 0.4.0
  (genesis `c5fc87d9…d708`, burned treasury) and is retired by 1.0. Statements below
  describe the state at the time they were written and are superseded.
- **No independent audit.** Consensus, the Orchard/Halo 2 integration, wallet key
  handling and the new Tor integration have only internal engineering reviews
  ([October review](docs/security/audit-2026-10.md),
  [network-privacy review](docs/security/network-privacy-review-2026-10.md)).
- **No exact-commit CI evidence for 0.4.0–0.5.0** is recorded in this repository;
  the last fully verified CI baseline is `cc280b5`, below.
- **Network privacy and P2P hardening (branch `privacy/tor-network-hardening`).**
  Direct, hybrid and tor-only modes (new wallet installations start in tor-only;
  existing ones keep their mode) with a built-in Tor client (Arti;
  nothing to install) or an operator-run Tor daemon, v3 onion services,
  fail-closed transport policy, backward-compatible address exchange,
  transaction-origin relay hardening with delivery that survives outages,
  restarts and a dropping relay peer, per-run handshake identities, reserved
  outbound slots and outbound diversity, wallet privacy controls. Peer protocol
  v1 and every consensus and monetary rule are unchanged. A Windows-only Arti
  stall was traced to a dependency and fixed with a vendored patch
  ([ADR 0015](docs/adr/0015-built-in-tor-client.md)).
  Local evidence, not CI ([details](docs/testing/network-privacy-evidence.md)):
  Windows formatting, strict clippy and the full workspace tests; Linux (WSL)
  workspace tests; live runs over the real Tor network with real mining and
  proofs on Windows (built-in Tor) and Linux (built-in Tor with a 300 s outage
  and a resolver that saw no node DNS query; external C Tor with a daemon
  restart); a mixed v0.4.0/v0.5.0/current devnet; deep-reorg, payment and
  recovery labs; time-boxed fuzzing.
  RUSTSEC-2023-0071 (`rsa` via Arti, assessed as not reachable) is ignored
  with a recorded justification. Release blockers: **onion addresses for the
  mainnet seeds** (both seeds refused connections from Tor exits on 2026-10-10,
  so a new tor-only installation finds no peer by itself), CI evidence on the
  exact commit, and an independent review. License texts for all dependencies are
  now collected (pinned upstream supplements for 55 crates). Not
  run: the Windows native-controls GUI test (moves the real mouse), peer
  rotation, testnet traffic.

## Latest fully verified baseline: cc280b5

All four jobs in [run 37739804704](https://github.com/deepdrogo/GENORYN/actions/runs/37739804704) passed. Both native platforms passed the complete practical suite, real thirteen-check private-payment/recovery/reorg lab, genuine testnet block search, 50,000 identities, read RPC, archive verification, actual packaged captures and launchers. Windows passed all five native node lifecycle checks; Linux observed real default peer rotation in 918.103 seconds. Consensus vectors are identical.

The Windows ZIP was independently downloaded and all 886 file hashes verified. Linux's 900-file ZIP was verified inside CI; its small report and screenshot were downloaded and checked. [Evidence](docs/testing/evidence/native-artifacts-cc280.json) records this distinction. Further source changes require the same exact-commit gates. One held additional RPC regression, a mixed live Windows/Linux LAN, external audit and production signing/genesis remain unresolved. Mainnet stayed disabled at that baseline (superseded: it launched in 0.4.0).

The entries below retain the earlier engineering history and its then-pending gates; they do not override this latest baseline.

## Testnet budget and execution-mode follow-through

Candidate `a688680` passed the complete Linux platform job, audit and all fuzz
campaigns. Windows passed all source/UI tests, actual rendering, configured peer
limits and the thirteen-check real payment/account/fee/reorg/restart lab. Its
additional real testnet block search then exhausted the 300-second budget; this
was a failed observation, not a passing testnet run. Failure reports now retain
miner metrics, and Windows gets a bounded 900-second search budget.

[ADR 0011](docs/adr/0011-randomx-execution-modes.md) records supported light-mode
CPU/JIT execution optimization requesting W^X and fallback for returned
construction errors. [ADR 0012](docs/adr/0012-randomx-binding-safety.md) records
the minimal reviewed Rust binding patch needed to handle nullable native VM
creation and writable output buffers correctly. An explicit portable-only
environment option is available. The algorithm,
consensus difficulty and bytes are unchanged. Differential mode tests and golden
vectors pass locally. A real optimized testnet block required 120 hashes and
about five seconds of hashing on the measured Linux host; cross-platform final
verification is still required. A Windows-native owned-node control harness is
also included for actual start/restart/stop/normal-close validation.

The final Linux native controls binary passed ordinary input clearing followed
by Ctrl+Z, and normal window close exited zero and reaped its owned node. An
external forced X window destruction hit an upstream winit panic; that separate
edge case is recorded and not represented as a normal-close success.

## Packaging baseline and required feature completion

Clean commit `95d1a5a` passed [all four workflow jobs](https://github.com/deepdrogo/GENORYN/actions/runs/37716486018).
Both actual native archives were downloaded and checked: Linux 876 file hashes,
436 packages/805 substantive notices; Windows 862 hashes, 425 packages/791 notices.
Both identify the exact clean source and their consensus vectors match. The
[artifact report](docs/testing/evidence/native-artifacts-95d1a5a.json) records
checksums and limits; it does not certify legal obligations or security.

The subsequent source closes required local feature gaps: GUI payment proof
review/import/export/verification; owned-node restart/log/TOML controls; optional
display reference value; actual diagnostics; fixed integer fee quotes and real
fee-rate miner ordering; bounded operator policy; encrypted derived accounts and
receiver indices; shared practical verification and measured DB/wallet/relay costs.
The [scope map](docs/specification-status.md) distinguishes required, verified and
explicitly later work. No consensus maxima or cryptographic protocol changed.

Local evidence includes a thirteen-check real private-payment lab with actual
two-payment fee ordering, a two-node genuine testnet block, account recovery,
configured connection cap/permit reuse and native proof workflows. Independent
review also found three issues (per-route RPC admission, late log reopening,
editor undo retaining cleared text); fixes and focused regressions are present.
The feature batch still needs its own final exact-commit CI and archive inspection.
The separately held additional RPC regression and external audit remain open.

Updated 2026-10-08. Experimental devnet implementation, not local-v1 sign-off.

## Latest cross-platform functional evidence

[Run 37703358653](https://github.com/deepdrogo/GENORYN/actions/runs/37703358653)
for exact commit `1911bd630d8af4af6ea881e5a42b3c1fc3b0f66b` completed successfully.
Both Ubuntu 24.04 and Windows Server 2022 passed formatting, strict lint,
unit/property/UI tests, genuine shielded proofs, native wallet rendering, the full
three-node mining/private-payment/recovery/reorg/restart lab, matching consensus
vectors, 50,000 registered identities, dataset-backed read RPC, package creation
and the actual packaged launchers. The three bounded parser-fuzz campaigns passed.

An independent packaging review subsequently found omitted dependency notices,
missing packaged documentation targets, undocumented Linux ABI requirements, and
an order-dependent native wallet path option. Source corrections and focused
regression tests are present; their final exact-commit CI remains separate from
the successful prior candidate. The [native notice inventory](docs/security/dependency-licenses.md)
now has complete mechanical coverage for both supported target graphs, with
checksummed upstream supplements and an explicitly identified standard CC0 text.
An actual internal Linux archive passed all 876 file checks and notice coverage.
Final clean-commit native archives still require CI and independent inspection.

## Transport-policy completion

Full-spec review identified deterministic sorted-prefix peer selection and absent
periodic rotation. The bounded policy now shuffles eligible outbound candidates,
retains the eight-outbound/sixteen-total resource limits, and rotates outbound
connections after 15–30 minutes only while another peer remains. A shared
one-minute cooldown prevents simultaneous voluntary rotations. Pure policy tests
and strict lint pass; the new ordinary real-process lab passed all eleven checks
again through height 12 ([report](docs/testing/evidence/peer-selection-lab.json)).
A separate four-node test observed the unchanged real rotation timer and required
reconnection: it passed after 918.659 seconds, with all four nodes returning to
three peers and the same genesis ([report](docs/testing/evidence/peer-rotation-default-policy.json)).
The Linux CI timer test also passed for `69669a5`. Both operating systems passed
all functional/50,000-identity/RPC stages on that candidate; their package stages
stopped at the documented missing-notice gate. Audit and all fuzz jobs passed.
This does not add Tor/I2P or claim source anonymity; see ADR 0008.

An independent peer-policy review reproduced a full stale-cache bug: a sorted
128-entry truncation excluded a higher-sorting explicit live endpoint. Admission
and dialing now preserve explicit peers inside the same memory bound, and the
real owned-loopback regression passes in 0.218 seconds ([report](docs/testing/evidence/peer-cache-fixed.json)).
An independent rerun against the exact changed P2P blob connected in 0.103 seconds,
kept its peer across twelve samples and preserved it in the bounded disk cache
([independent result](docs/testing/evidence/peer-cache-independent.json)). That
specific reproduced starvation finding is closed for the tested source snapshot.
Mapped IPv4 discovery policy and bounded dial backoff have dedicated pure tests.
The complete real mining/payment/recovery/reorg/restart lab passed all eleven
checks again after these corrections ([report](docs/testing/evidence/bounded-cache-lab.json)).
Candidate `b4c1397` subsequently passed all source/native runtime, real cache,
ordinary lab, vector, 50,000-identity and RPC checks on Windows/Linux, plus actual
rotation on Linux, audit and fuzz. Its package stages stopped at the prior notice
gate. The packaging correction is validated locally and still requires its own
exact-commit CI and final archive inspection.

## Verified functionality (final release gate remains open)
- Rust 1.92.0 and RandomX native C++ dependencies build on Linux and Windows.
- 15 core tests pass: exact supply, all halving epochs, treasury tranches,
  retarget simulations, canonical decoding, network separation, real RandomX.
- Full corrected Halo 2 mint proof generated and verified; amount tampering,
  cross-network signature replay and corrupted proof were rejected.
- Encrypted wallet recovery and signed-username ownership/replay tests pass.
- SDK unit tests and actual Linux/Windows native GUI runtime pass.

## Additional completed validation
- Two real local-process labs passed, latest through height 12, including private
  Alice/Bob/Carol transfers, recipient re-spend, signed username payment, payment
  proof, seed recovery, node catch-up, a competing-chain reorganization and abrupt
  three-node restart. Sanitized reports are in `docs/testing/evidence/`.
- Actual native GUI ran on Linux, unlocked and scanned a real wallet/node, and
  all eight screens were captured. No fabricated balance or simulated peers.
- SDK and GUI focused tests pass, including URI validation, process reaping,
  late-result invalidation and monochrome rendering. Strict lint passes.
- 512-case parser property tests, hostile length checks, exclusive database lock,
  schema/network mismatch and corrupt database tests pass.
- A genuine mined block fixture validates real PoW/proofs and canonical bytes;
  a shared RandomX/header vector is committed for Windows/Linux comparison.
- Genuine 5-block and 10-block heavier-chain rollbacks restore Bob's prior
  balance, return the orphaned payment to the mempool, and remine it exactly once.
- Actual treasury commitments remain absent from the spend tree until their
  unlock height, then produce a matching Merkle witness in the boundary test.
- A bounded real-cryptography benchmark and a complete Linux package/checksum
  validation are recorded. An initial ZIP timestamp edge case was fixed.
- Dependency review upgraded vulnerable Hickory 0.24 to patched 0.26.3. The
  subsequent cargo-audit scan found zero known vulnerabilities. Three
  informational maintenance warnings remain documented.

## Trust-boundary corrections awaiting one additional regression run
Code review identified a missing exact-name check on signed resolver responses
and a proof-verification path that needed full selected-chain validation. Both
are corrected. The additional local service-substitution regression was blocked
before a verified post-fix result; it is not counted as passing. Existing ordinary
unit/integration and compilation checks continue independently.

## Implemented and exercised by the ordinary integration labs
Chain persistence, branch replay and reorg selection; shielded maturity anchors;
mempool double-spend handling; P2P sync and relay; local authenticated RPC; CLI
wallet scan/send/restore; voluntary per-output payment proofs; native controls.

## Remaining gates
Final native package generation and redistribution review after the completed
mechanical notice inventory; the aggregate check on the latest changed commit; inspection of
that commit's Windows/Linux package artifacts; one blocked additional
trust-boundary regression.
The CI matrix runs each OS separately; a mixed Windows/Linux live LAN remains
unverified. These limits are not replaced by a passing compile.
External audit, mainnet genesis/keys and production release authorization are
separate external gates. Mainnet remained disabled at that time (superseded: it launched in 0.4.0).

## Registered-identity capacity and platform progress

The clarified capacity target is tens of thousands of registered identities,
not concurrent payments. The daemon now uses indexed atomic SQLite username
storage with a 100,000-record bound. A real 50,000-distinct-wallet/receiver/owner
benchmark passed persistence, lookup and renewal: 35.6 MB database, 272.3 seconds
for generation plus durable registration, 79 microseconds median verified lookup.
The 50,000-record daemon also passed 5,000 mixed information/name read requests
at up to 128 client workers, all successful and consistent. These are bounded
local measurements, not a universal production capacity guarantee or chain TPS.

Candidate 4ac8f6b's Linux CI passed every stage, including native GUI, real lab,
golden vectors and package generation. Its downloaded package was independently
verified against all 891 manifest hashes, source commit and clean-worktree flag.
Windows passed all 11 genuine lab checks but failed test-directory cleanup due
to an unclosed Python SQLite backup handle. The explicit-close correction is
included in the later successful `1911bd6` run described above.
All three bounded parser fuzz campaigns passed. One extra trust-boundary regression
and a mixed live Windows/Linux LAN remain unverified, as recorded above.
