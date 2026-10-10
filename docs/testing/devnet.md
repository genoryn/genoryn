# Reproducible local devnet

Run from the repository root after `cargo build --locked --workspace`.

```sh
python3 scripts/test/devnet.py --bin-dir target/debug \
  --work-dir .devnet/my-fresh-lab --report .devnet/my-report.json
```

The lab chooses free loopback ports, creates separate data directories and
encrypted test wallets, then launches three real daemon processes. It mines
genuine RandomX blocks with real Halo 2 coinbase proofs, sends private payments,
checks unrelated-wallet scanning, spends received value, tests double-spend and
invalid-proof rejection, registers/resolves a signed username, verifies a
per-output payment proof, restores a seed, tests offline catch-up, mines competing
chains and verifies heavier-chain reorganization, then kills/restarts nodes.

`--keep` retains valueless test wallets and logs for debugging. Do not upload the
lab directory. A sanitized JSON report contains public test results only. The
script refuses a nonempty work directory, and it terminates its owned processes
on failure. `--quick` omits only the competing-chain stage; it is not the full gate.

Windows uses `python` instead of `python3` and `.exe` binaries automatically.
The CI matrix runs actual Windows and Linux daemons separately; shared committed
block/RandomX vectors verify cross-platform consensus compatibility. This is not
a claim that a mixed Windows/Linux live LAN was exercised unless a separate
mixed-platform report is present.

## Manual three-node run

```sh
genorynd --network devnet --data-dir .devnet/a --p2p-listen 127.0.0.1:29410 --rpc-listen 127.0.0.1:29411
genorynd --network devnet --data-dir .devnet/b --p2p-listen 127.0.0.1:29420 --rpc-listen 127.0.0.1:29421 --peer 127.0.0.1:29410
genorynd --network devnet --data-dir .devnet/c --p2p-listen 127.0.0.1:29430 --rpc-listen 127.0.0.1:29431 --peer 127.0.0.1:29420
```

Create Alice and Bob:

```sh
genoryn-wallet-cli --wallet .devnet/alice.wallet create
genoryn-wallet-cli --wallet .devnet/bob.wallet create
genoryn-wallet-cli --wallet .devnet/alice.wallet address
genoryn-miner --node http://127.0.0.1:29411 --token-file .devnet/a/admin.token --wallet ALICE_RECEIVER --threads 1 --max-blocks 4
genoryn-wallet-cli --wallet .devnet/alice.wallet --token-file .devnet/a/admin.token send BOB_RECEIVER 1.0
genoryn-wallet-cli --wallet .devnet/bob.wallet balance
```

Mining one more block confirms a submitted transfer. Use `get-network-info` and
`get-supply` on `genoryn-cli` for public state. Proof verification and rescan are
CPU-intensive; avoid many parallel prover processes on a small machine.

## Aggregate checks

`scripts/test-all` on Linux or `scripts/test-all.ps1` on Windows calls the same
portable Python runner. It performs formatting, strict Clippy, unit/property
checks, launcher/notice tests, genuine proof fixtures, builds, an actual rendered
native window, stale-cache and configured-limit tests, the complete real payment
lab, isolated testnet mining, consensus vectors and the cryptographic/database benchmark. The CI matrix
invokes this exact practical runner on both platforms.

Use a desktop display or install the official `xvfb` package on headless Linux.
A missing native runtime prerequisite fails the suite; it is not counted as a pass.
Reports go to a fresh directory, record commit/dirty status, and remain failed
if execution stops before the last stage.

```sh
scripts/test-all
scripts/test-all --release
```

The `--release` suite additionally runs the real 15–30 minute rotation timer,
50,000 registered identities, dataset-backed RPC load, `cargo audit`, three
120-second fuzz campaigns, package generation and the packaged launcher. Install
its official developer tools beforehand:

```sh
cargo install cargo-audit --version 0.22.2 --locked
rustup toolchain install nightly --profile minimal
cargo install cargo-fuzz --locked
```

The release run may take an hour or more. PowerShell accepts the same `--release`
and `--output PATH` arguments. Neither suite launches mainnet, signs a production
release or executes the separately held additional trust-boundary regression.
Keep final reports tied to the exact source commit; a dirty local suite cannot
replace final clean-commit CI/artifact verification.

## Default-policy peer rotation

The transport regression uses four actual loopback TCP nodes and their ordinary
15–30 minute outbound rotation timers, without accelerated clocks, mining or
wallet creation. Allow up to about 35 minutes. It verifies redundant connectivity,
observes a real outbound rotation, then requires a new peer connection and
continued shared-genesis connectivity:

```sh
python3 scripts/test/peer_rotation.py --bin-dir target/debug \
  --work-dir .devnet/rotation-check --report .devnet/peer-rotation.json
```

Use a fresh work directory. The report distinguishes this network-only test from
the separate genuine mining/payment lab. Linux CI runs it before packaging;
Windows still runs the full ordinary three-node lab and pure rotation-policy tests.

## Full stale-cache regression

```sh
python3 scripts/test/peer_cache.py --bin-dir target/debug \
  --work-dir .devnet/peer-cache-check --report .devnet/peer-cache.json
```

This short test reserves all 128 stale loopback endpoints itself, starts a real
live peer whose address sorts after them, and starts another node with that full
cache plus the explicit live peer. It requires a real connection and preservation
of the explicit address within the 128-entry on-disk bound. It never contacts an
uncontrolled service and does not mine or create wallets. Both CI platforms run it.

## Configured resource limits

```sh
python3 scripts/test/policy_limits.py --bin-dir target/debug \
  --work-dir .devnet/policy-check --report .devnet/policy-limits.json
```

Three real loopback nodes exercise a one-connection inbound policy, repeated
observation of the cap, permit reuse after a peer stops, matching network identity
and actual database-size/peer-direction diagnostics. No mining or payments occur.

## Isolated testnet validation

```sh
python3 scripts/test/testnet.py --bin-dir target/debug \
  --work-dir .devnet/testnet-check --report .devnet/testnet.json
```

Two actual loopback nodes validate one real testnet RandomX/Halo2 block. The
check uses a fresh valueless test wallet, matching testnet identity and supply,
and confirms that unfinalized mainnet refuses startup before creating its data
directory. Mining is one thread, normally capped at 300 seconds; the Windows
practical suite allows 900 seconds, and `--max-seconds` is bounded to 30..1800.
A stochastic search that exhausts its budget is reported as a failure with actual
hash counts and timings. No public network
or external peer is contacted.

## Windows native owned-node controls

The Windows practical suite also posts ordinary mouse and close events to its
own actual wallet window. It checks real owned-node start, repeated start,
restart/reaping, stop and normal window-close cleanup on fresh loopback ports.
It never creates a wallet, mines, transfers value or contacts an external peer.
Cleanup handles are restricted to direct node children of the spawned GUI.
This complements rendered-frame and shared UI-state tests; a failed or unavailable
interactive desktop is not counted as a successful native-control test.
