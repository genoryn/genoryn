# Network-privacy testing evidence

Local runs on 10 October 2026: Windows 11 (build 26300) and Ubuntu 24.04 under
WSL2 (kernel 6.6.87.2) on the same machine, Rust 1.92.0, branch
`privacy/tor-network-hardening` (the commit that adds this file). This is
developer evidence from one machine: it is not CI on the exact commit and not
an audit. Machine-readable reports are in [evidence/](evidence/); the earlier
pre-built-in-Tor summary is
[network-privacy-local-windows.json](evidence/network-privacy-local-windows.json).

## Results

| Check | Where | Result |
|---|---|---|
| `cargo fmt --all -- --check` | Windows | Pass |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | Windows | Pass |
| `cargo test --locked --workspace` (working tree) | Windows | 201 passed, 0 failed, 4 ignored |
| `cargo test --locked --workspace` (working tree) | Linux (WSL) | 201 passed, 0 failed, 4 ignored |
| Same, on the exact staged tree in a clean directory and target | Windows | 200 passed, 0 failed, 4 ignored; clippy, fmt and packaging tests pass (another session's uncommitted wallet test is not part of it) |
| Real proofs, mining, reorg: `cargo test -p genoryn-core -- --ignored` | Windows | 4 passed (genuine Halo 2 proof and tamper rejection, treasury witness, committed real block, real-mining reorg) |
| Real-process lab `scripts/test/devnet.py` | Windows | 13 of 13 checks (twice) |
| Deep reorg `scripts/test/reorg_depth.py` (depth 5) | Windows | 2 of 2: payment rolled back, reconsidered, mined once |
| Peer cache, connection limits, consensus vectors | Windows | Pass; monetary SHA-256 `f3e96a38…7ce6` unchanged |
| Packaging unit tests `scripts/test/package_licenses.py` | Windows | 17 passed |
| Fuzz graph, RandomX vendor, launcher scripts | Windows | Pass |
| Native wallet frames `genoryn-wallet --smoke-test-frames 5` | Windows | Exit 0 |
| `cargo audit` (0.22.2) | Windows | Exit 0 with the approved ignore of RUSTSEC-2023-0071 (`rsa` via Arti; see the [dependency review](../security/dependency-review.md)); 4 unmaintained warnings |
| Fuzzing, 180 s per target (`cargo +nightly fuzz`) | Linux | `block_parser` 8,776,923 runs, `transaction_parser` 4,290,326, `username_parser` 37,679,240; no crash |
| Live Tor, built-in client | Windows | 7 of 7 checks |
| Live Tor, built-in client, outage and resolver | Linux | 11 of 11 checks |
| Live Tor, external C Tor 0.4.9.11 with a daemon restart | Linux | 11 of 11 checks |
| Mixed v0.4.0 / v0.5.0 / current devnet `scripts/test/mixed_versions.py` | Windows | 8 of 8 checks |

## Live runs over the Tor network (`scripts/test/tor_live.py`)

Two tor-only devnet nodes with v3 onion services and no clearnet listener;
B knows only A's onion address. Real RandomX mining and Halo 2 proofs. Each
run checks: onion-to-onion peering with zero clearnet connections and zero
direct attempts; block sync over Tor; a shielded transaction A→B and B→A, each
relayed over Tor (identical mempool nullifiers on both nodes) and mined
(recipient or sender balance changes exactly); both onion addresses and the
chain unchanged after a hard kill and restart; every TCP endpoint of both node
processes, sampled from `netstat`/`ss` every 0.5 s, is loopback or a Tor relay
(consensus ORPort or Arti's compiled-in fallback directory), and no UDP.

The Linux runs add, as root in a private mount namespace: `resolv.conf` points
to a logging resolver that must see the harness's positive-control lookup and
no query from the nodes; and an outage (blackhole routes to every relay and
fallback for the built-in client, stopping the Tor daemon for external Tor)
during which B creates a payment, is hard-restarted, and must keep the payment
pending with zero direct attempts, then deliver it after recovery and get it
mined.

| Measurement (seconds) | Windows built-in | Linux built-in | Linux C Tor |
|---|---|---|---|
| Tor ready with onion identity, fresh directory | 10.6 | 16.1 | 0.5 (daemon already running) |
| First onion-to-onion peering | 8.1 | 8.5 | 37.1 |
| 4-block sync after mining | 0.6 | 1.0 | 1.0 |
| Shielded transaction relay over Tor | 10.5, 9.4 | 6.6, 8.6 | 8.5, 9.4 |
| Peering again after a hard restart | 54.1 | 7.5 | 247.0 |
| Delivery after the outage ended | n/a | 19.4 (300 s outage) | 37.9 (242 s daemon stop) |
| Distinct Tor relay endpoints used | 10 | 15 | 0 (only loopback to Tor) |

Single runs over the public Tor network; times depend on the circuits and
relays chosen. Earlier runs of the same scenario took up to 132.8 s for the
first peering (descriptor publication of a new onion service). With an
external Tor, a restarted node re-adds its onion service and Tor publishes a
fresh descriptor, so peers reach it again only after minutes (247 s here);
the built-in client keeps its service state and was reachable within 8–54 s. For comparison,
direct mode in the devnet lab accepts a block on three nodes within 72–81 ms
(empty) or 1.36–1.50 s (with transactions, proof verification included).
Hybrid mode was exercised in the mixed-version run but not timed.

## Light wallet over the Tor network

The same live scenario on Windows with both nodes serving their public RPC
through the built-in onion service ([report](evidence/tor-live-windows-light-wallet.json),
9 of 9 checks). A light wallet without a node, with its own built-in Tor in an
empty directory, synced from both onion RPCs (one `used`, the other `agrees`),
validated the chain fully, saw the exact amount it was paid, then sent a
payment that was relayed through a remote node, reached both nodes'
mempools and was mined; its balance then matched exactly. First light sync
71.6 s (including Tor bootstrap and full validation), light send 64.3 s
(validation, proof and relay). `apps/genoryn/tests/light_wallet.rs` covers
source selection, rejection of a source serving an invalid chain, cross-checks
and relaying through another source behind a SOCKS stand-in (4 tests).

## Mainnet reachability through Tor (read-only)

A tor-only mainnet node with the built-in client and an empty data directory
(no wallet, no mining, no transactions) ran for about eight minutes: Tor was
ready, the published seed list was fetched through Tor, and all 12 attempts to
the two seeds (`187.7.72.137`, `2.24.162.163`, port 29610) failed at the exit
(10 timeouts, 2 stream errors). Both seeds accepted a direct TCP connection
from the same machine. An independent C Tor 0.4.9.11 client in WSL got no
SOCKS grant for any of six connection attempts, while fetching
`check.torproject.org` through it worked. The seeds' networks evidently do not
accept connections from Tor exits; tor-only nodes need onion seeds
(`seeds/mainnet-onion.txt`) before they can bootstrap on mainnet alone.

## Built-in Tor on Windows

Arti stayed at 15 % with one core busy on Windows; the same build bootstrapped
on Linux in 8.6 s. Bisection with an instrumented `tor-netdoc` pinned the loop
to `saturating-time`'s limit search. A program containing only
`SystemTime::now().saturating_sub(300 s)` with `saturating-time` 0.5.0 used
19.8 s of CPU in 20 s and never returned on Windows (where `now - 1 ns == now`),
and returned in 793 ns on Linux. With the vendored fix, Arti bootstrapped from
an empty directory on Windows in 18.2 s and opened an exit stream in 1.6 s.
The patched crate passes its 7 unit and 6 doc tests on Windows and 9 and 6 on
Linux (including upstream's exact Unix limit tests); its new coarse-clock test
hangs with the original algorithm.

## Adversarial relay tests (`apps/genoryn/tests/relay_reliability.rs`)

| Test | Observed |
|---|---|
| A dropping stem plus an echoing accomplice | The honest third peer received the transaction before the shortest embargo; with echo relaying disabled the test fails ("suppressed") |
| A silently dropping stem | Every peer received it; only the stem before the embargo |
| A late peer | Received relayed and already-broadcast own transactions, never one in its stem phase |
| Tor-only outage longer than the old 30 s relay search | Still pending, never a direct connection (canary 0); delivered through the proxy once it returned |
| Saved transactions that no longer validate | Dropped on restart, nothing left on disk |

`p2p::tests::frames_reach_peers_through_buffering_streams` fails when the frame
flush is removed; the live runs confirmed that handshakes over Arti need it.

## Mixed versions (`scripts/test/mixed_versions.py`)

Released v0.4.0 and v0.5.0 binaries from separate build directories, the
current build in direct and hybrid mode: one genesis; blocks mined by v0.4.0
accepted by all; a v0.4.0 wallet's payment mined by the current miner and
accepted by the old nodes; a current wallet's payment (new local delivery)
relayed to every version, mined by the v0.4.0 miner and seen by a v0.5.0
wallet; old nodes stayed connected with no decode errors; the current build
opened both legacy chain databases at the same tip, and the current wallet CLI
opened both legacy wallet files with the same balances.

## Not run here

- CI on the exact commit (both platforms). Packaging now succeeds locally with
  license texts for every dependency; archive verification needs a clean checkout.
- `scripts/test/windows_native_controls.py` (moves the real mouse pointer).
- Peer rotation over its ~15 minute default, 50,000-identity capacity, RPC load.
- Testnet or mainnet traffic of any kind.
- Any independent review.

## Harness notes

The log-capture test installs a second, inert tracing dispatcher: with only one
registered dispatcher `tracing-core` caches a callsite's interest from whichever
thread reaches it first. Socket sampling every 0.5 s can miss connections that
open and close between samples; the resolver check covers DNS independently.
