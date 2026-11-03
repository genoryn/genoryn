# Light-client protocol

## Header MMR

Leaves: `L(b) = H("Krypton/mmr/leaf", hash(b), difficulty(b))` with work = difficulty.
Parents: `H("Krypton/mmr/node", left, right, work_sum)`. The root bags the peaks right to
left (`Krypton/mmr/bag`) and commits leaf count and total work (`Krypton/mmr/root`). The
header at height `h` commits to the MMR of blocks `0..h−1`.

## Verifying the tip (FlyClient-style)

The client chooses sample heights with density increasing toward the tip
(`x = 1 − δ^u`, `δ = 1/height`), requests them (`GET /v1/flyclient?heights=...`) and checks:

1. the tip's proof of work;
2. for each sample: its proof of work and its MMR inclusion proof against `tip.mmr_root`;
   every requested height must be present;
3. the recent window (101 headers): linkage, proof of work and the LWMA difficulty rule.

An attacker who forks away from the honest chain must produce proof of work for every
sampled height on its branch; with enough samples the probability of a successful fake is
negligible, while the cost stays logarithmic in the chain length.

## Compact blocks

`GET /v1/compact?from=h&count=n` returns, per block: header, tree size before the block, all
outputs (commitment + ciphertext) in transaction order, all nullifiers, the block's reward
payouts and the payouts maturing in it. The client verifies the output, nullifier and payout
digests against the header, recomputes the matured payout commitments from their public
data ([rewards.md](rewards.md)) and the note-tree root from its frontier. A fake or altered
compact block is rejected.

## Private witnesses

`GET /v1/subtree/{chunk}` returns the 4,096 leaves of a chunk plus the 28 upper siblings.
The wallet computes its path locally and checks it against the root of its verified header.

## Scanning

Each output is trial-decrypted (hybrid KEM decapsulation + AEAD) and accepted only if it
re-commits to the published commitment under the wallet's owner tag. Spends are detected
by recomputing the wallet's own nullifiers. The wallet keeps 120 verified checkpoints for
reorganizations.
