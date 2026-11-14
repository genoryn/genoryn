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

**Work rules (1.0.33).** Each release carries a checkpoint (mainnet: block 14,800). Above
it:

* every sampled and recent header needs at least 1/16 of the checkpoint's difficulty, and the
  blocks since the checkpoint must carry that much work on average, as proven by the MMR;
* no header may be dated more than 2 hours (plus the 5-minute consensus drift) ahead of the
  client's clock, so a fork cannot lower its own difficulty quickly;
* the node may send at most 64 recent headers beyond the difficulty window.

`verify_flyclient` returns the **total work** the tip commits to: every MMR node hashes the
work beneath it, so an inclusion proof (`mmr::verify_work`) proves the work of all leaves
before and after its own. The wallet remembers the most work it has verified and refuses a
chain with less (an honest chain only gains work), so a cheap fork cannot replace the chain
it already follows; faking confirmations for it costs as much work as the honest network
spends on them. When a node proves a different block at the wallet's height inside an at
least as heavy chain, the wallet goes back to the fork point (or rescans) and follows it; a
node that only sends bad proofs proves nothing and changes nothing.

**Limitation.** A wallet that has never synced trusts the first chain shown to it above the
checkpoint. Configure several independently operated nodes (wallets refuse to sync when
they disagree), or run your own node for large swaps.
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
