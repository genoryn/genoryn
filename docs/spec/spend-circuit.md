# Shielded spend circuit (StarkSpend v1)

## Primitives

`H(d; a, b)` is one Rescue-Prime Rp64_256 permutation over the Goldilocks field on
`[d, 0, 0, 0, a0..a3, b0..b3]`, output `state[4..8]` (4 field elements ≈ 256 bits).
Domains: OWNER, NULLIFIER, NULLIFIER_KEY, AUTH_KEY, NOTE_COMMIT, NOTE_BODY, MERKLE
(`crates/krypton-crypto/src/rescue.rs`).

Keys from a secret `sk`: `ak = H(AUTH; sk, 0)`, `nk = H(NK; sk, 0)`, `owner = H(OWNER; ak, nk)`.
Note commitment: `cm = H(CM; H(BODY; owner, [v,0,0,0]), rcm)`.
Nullifier of the note at tree position `p`: `nf = H(NF; nk, [p,0,0,0])`; dummy inputs use
domain `NF + 2^24`, so they never collide with real nullifiers.

## Statement

Public: `root, nf_0, nf_1, cm_out_0, cm_out_1, fee, mint, sighash`.
Private: for each input `sk, v, rcm, p, path[40], dummy`; for each output `owner, w, rcm`.

* For each non-dummy input: `cm` is the leaf at position `p` of the depth-40 tree with root `root`.
* `nf_i` is computed as above; dummy inputs have value 0.
* `cm_out_j` commits to `(owner_j, w_j, rcm_j)`.
* All four values are < 2^60 (bit decomposition); `v_0 + v_1 + mint = w_0 + w_1 + fee`.
* `sighash` (SHA3 of the full transaction without the proof, mapped to 4 field elements) is a
  public input: the Fiat-Shamir transcript binds the proof to this exact transaction, which
  replaces a signature. Knowing `sk` (a hash preimage) is the authorization.

## Trace

512 rows × 149 columns: two key/note lanes (8 Rescue cycles each), two Merkle lanes (40
cycles), 4 range-check accumulators with bit columns, 88 constant registers, 1 random
masking column. Rows 320–511 are uniformly random. Transition constraints: 461, maximum
degree 8 (Rescue round × periodic selectors). Options: 36 queries, blowup 16, 16 grinding
bits, cubic extension, FRI folding 8: ~160 bits conjectured security. Proof ~92 KB, proving
~0.5 s, verifying ~2 ms.

## Zero knowledge

Winterfell has no built-in zero-knowledge mode, so the prover masks the trace itself:

1. Every column has 192 random rows outside the constrained region. Each column opens at
   most 2 (out-of-domain) + 36 (queries) + 36 (next-row influence through the composition)
   evaluations, fewer than the random degrees of freedom, so opened values are distributed
   independently of the witness.
2. One fully random column enters the DEEP composition with a non-zero random coefficient,
   which masks the DEEP/FRI polynomials and the FRI remainder.

This is a standard trace-randomization argument; it has **not** yet been formally reviewed
and should be audited before mainnet value relies on it.

## Tests

`crates/krypton-zk/src/prover/tests.rs`: valid proofs, coinbase with dummy inputs, rejection of
any change to the statement or proof bytes, refusal of invalid witnesses, and proofs from a
dishonest prover (inflation, fake membership) failing verification.
