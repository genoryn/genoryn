# Capacity: registered identities, RPC clients and payments are different

The target is tens of thousands of **registered identities**, not tens of
thousands of simultaneous payments. Wallet keys are created locally and do not
need a central account registration. Optional public usernames are separate
signed receiver records. Neither username counts nor RPC connections reveal the
number of private wallets, balances or holders.

## Measured and bounded components

- Resolver storage: an indexed SQLite table, atomic per-record writes, a
  100,000-record bound, schema/network checks and validated legacy migration.
  The capacity benchmark creates 50,000 distinct wallet receivers and signing
  owners, durably registers them, reopens the database, checks indexed signed
  lookups, and verifies renewal without changing the record count. Report actual
  results only after the run completes.
- Public read RPC: an isolated 5,000-request test covers 1, 4, 16, 64 and 128
  client workers against a real fresh daemon. Report status counts, p50/p95/p99,
  throughput and post-test liveness. This is a bounded genesis-state benchmark,
  not a production SLA or proof of 10,000 concurrent connections.

The completed 50,000-identity run used 35,586,048 database bytes and took
272.3 seconds including actual key generation and durable registration. Reopen
plus integrity checks took 1.34 seconds; verified indexed lookup p50/p95/p99 was
79/252/1,111 microseconds. Ten signed renewals preserved the 50,000 record count.
The dataset-backed daemon completed 5,000 mixed info/name RPC reads across all
five concurrency levels, with HTTP 200 and consistent payloads throughout.
Sanitized measurements are in `evidence/registered-identity-capacity.json` and
`evidence/rpc-registered-capacity.json`. Timings are one local environment/run.
- Cryptographic work: proof creation, verification, note trial-decryption and
  RandomX measurements are separate and hardware-dependent. Proof workers,
  requests, peer frames, mempool and connection counts remain bounded.

## Consensus throughput and history growth

The user-specified block target is 60 seconds. The current provisional cap is
64 ordinary transfers per block, an upper bound of about 1.07 transfers/second
before other resource constraints. Registered-user capacity does not require
changing that rule. A larger transaction-throughput target requires a distinct
consensus decision, adversarial validation and measured propagation/proof costs.

Full nodes retain encrypted history. The current wallet performs conservative
full validation and scanning rather than an address-indexed remote query.
Long-history incremental scanning, pruning and efficient private light-client
protocols need further engineering and measurement. Avoid converting a short
devnet benchmark into a claim of indefinite production scale.

## Reproduce

```sh
cargo run --locked -p genoryn --bin genoryn-identity-bench -- \
  --count 50000 --workers 4 --database .devnet/capacity/usernames.sqlite
python3 scripts/test/rpc_load.py --work-dir .devnet/rpc-capacity \
  --report .devnet/rpc-capacity.json --requests 1000
```

Use fresh directories. The benchmark identities are valueless test fixtures;
their spend keys are discarded, never logged or published. Do not send funds to
these receiving identities. No test contacts an external target or mines by itself.
