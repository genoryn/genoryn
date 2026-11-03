# KryptoHash v1

`B64(m, len)` is the 64-byte BLAKE3 output of one block of `len ≤ 64` bytes;
`fnv(a, b) = (a · 0x01000193) xor b` on u32.

* **Epoch:** 16,384 blocks. `seed_e = H("KryptoHash/v1/epoch-seed", chain_seed, e)`; the chain
  seed is the genesis hash.
* **Cache:** 262,144 items of 64 bytes (16 MiB). `c[0] = B64(seed)`, `c[k] = B64(c[k−1])`,
  then 3 passes of `c[k] = B64(c[k−1] xor c[c[k][0] mod n])`.
* **Dataset:** `2 GiB + e · 4 MiB`, capped at 12 GiB, in 64-byte items. Item `i`:
  `m = B64(c[i mod n] with m[0] ^= i)`, then 64 rounds `p = fnv(i xor j, m[j mod 16]) mod n;
  m[w] = fnv(m[w], c[p][w])`, item `= B64(m)`.
* **Hash:** `s = B64(pow_hash || nonce_le64, 40)`; mix (32 words) = `s || s xor 0x9E3779B9`;
  64 accesses: `p = fnv(a xor s[0], mix[a mod 32]) mod pairs`, read items `2p, 2p+1`,
  `mix[w] = rotl(fnv(mix[w], d[w]), (a mod 16) + 1)`; compress 4 words → 1 with fnv;
  result `= BLAKE3(s || cmix)` (32 bytes).
* **Target:** the result as a little-endian 256-bit integer `x` is valid iff
  `x · difficulty < 2^256` (difficulty is u128).

Verification needs only the cache (128 items computed on the fly, ~1 ms). The CPU, CUDA and
OpenCL implementations share one C core (`crates/krypton-pow/kernels/kh_core.h`) and are
cross-checked in tests.

| Device | Measured |
|---|---|
| RTX 5090, CUDA | 74.2 MH/s, dataset built in 0.11 s |
| Core Ultra 9 285K, 24 threads | 1.37 MH/s, dataset built in 4.2 s |
