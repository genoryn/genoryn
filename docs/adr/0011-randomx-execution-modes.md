# ADR 0011: supported RandomX execution modes with identical hashes

The original integration forced `FLAG_DEFAULT`, RandomX's portable interpreter.
The Windows practical lab measured about five seconds for each first hash/cache
setup, and a bounded stochastic testnet block search exhausted five minutes.
That observation does not by itself isolate warm-hash cost; future failed search
reports must preserve actual hash counts and phase timings.

Use only the pinned upstream implementation's CPU-recommended hardware AES,
Argon2 CPU optimizations and JIT flags, still in light mode. Add `FLAG_SECURE`
whenever JIT is selected, requesting the upstream W^X page policy.
Mask out every other flag, including full-memory mode, large pages and algorithm
version changes. No privileged allocation or operating-system security setting
is requested. Returned construction errors use the portable interpreter. This does not promise
recovery from every runtime failure under OS JIT restrictions; see
[the binding correction and limits](0012-randomx-binding-safety.md).
Set `GENORYN_RANDOMX_PORTABLE=1` before starting the node/miner/wallet to force
the portable interpreter without requesting JIT execution. Hash validity is unchanged.

Execution modes must produce the same RandomX hash. This changes neither the
algorithm version, input bytes, epoch key, target comparison, initial difficulty,
block timing nor validity rules. The node still independently checks real work.
Differential tests compare portable and selected execution on multiple seeds and
inputs, and existing cross-platform golden vectors remain mandatory.

The [pinned Rust API](https://docs.rs/randomx-rs/1.6.0/randomx_rs/struct.RandomXFlag.html)
and [upstream API definition](https://github.com/tevador/RandomX/blob/master/src/randomx.h)
document these flags and portability tradeoffs. GENORYN's compiled dependency
and algorithm remain pinned by Cargo.lock; upstream master is explanatory, not
a floating build input. Native/JIT code remains an upstream dependency and needs
external review as part of the independent GENORYN integration.

Keep mining thread/time limits and report cold setup separately from hashing.
No fixed short wall-clock deadline can guarantee winning a probabilistic PoW
search. A timeout is a failed/incomplete integration observation, never a pass or
an excuse to lower consensus difficulty or substitute a fake proof.
