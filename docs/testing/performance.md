# Measure, do not assume

Run the bounded local benchmark with two proving workers:

```sh
RAYON_NUM_THREADS=2 cargo run --locked -p genoryn-core --example benchmark
```

It measures actual canonical parsing, verifier/prover key construction, a real
corrected one-action Halo 2 proof, signature/proof validation, RandomX cold-cache
and warm-hash time, note trial decryption, real block validation/durable SQLite write, indexed block
lookups, and database reopen/full replay. The database fixture is one actual
committed block, not a long-history capacity claim. Setup and steady-state work are
reported separately. It does not advertise these single-machine results as
network throughput, user-wallet latency or production capacity.

The JSON report states its build profile, architecture, proof size and iteration
counts. Compare identical compiler/dependency/circuit versions and hardware under
similar load. The real devnet lab additionally records per-operation wallet durations (including
CLI startup, KDF, RPC and cryptographic setup), miner template/hashing/submission
times, node restart/replay time, and same-host three-node acceptance spans for
the first seven blocks. Acceptance spans include propagation, validation and
scheduling; they are not pure wire latency. A synthetic subsecond target does
not excuse skipping cryptographic checks.

The current full-rescan wallet and full branch replay prioritize correctness for
the local milestone. They are not yet optimized for years of chain history.
