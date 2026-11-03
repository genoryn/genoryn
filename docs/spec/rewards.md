# Proportional mining rewards (v1)

Genoryn Krypton pays miners in proportion to their work, without pools and without
luck deciding who takes a whole block. A miner with 1 % of the hashrate earns about 1 %
of all rewards, one with 88 % about 88 %.

## Shares

A **share** is a block header whose proof of work meets the share target:
`share_difficulty = max(1, difficulty / 32)`. Every hash a miner computes is checked
against it, so the network finds about 32 shares per block (one of them is the block
itself, which also counts as a share of its miner). Shares are relayed peer to peer
and included by later blocks.

A share `s` may be credited by a block `B` at height `h` when:

1. `s.version = 1`, `1 ≤ s.height ≤ h` and `s.height + 32 > h` (the inclusion window).
2. `s.miner` is a canonical owner digest.
3. If `s.height = h`: `s.prev = B.prev` and `s.difficulty = B.difficulty` (a sibling).
   Otherwise: `s.prev` is the main-chain block at `s.height − 1`, `s.difficulty` equals the
   main-chain block's difficulty at `s.height`, and `s` is not itself that main-chain block.
4. `s` meets `share_difficulty(s.difficulty)`; its hash differs from `B`'s and was never
   credited by an earlier main-chain block.

The owner is part of the proof-of-work input, so a share cannot be re-pointed to another
miner. Other header fields of a share (roots, timestamp) carry no meaning.

## Reward pool

Consensus keeps a pool balance `P`. For block `B` at height `h` with difficulty `D`:

```
P   += subsidy(h) + Σ fees(B)
for every credited share i (B's own header first, weight w_i = its difficulty):
    c_i = P · min(w_i, 4·D) / (8 · 32 · D)          (integer, see mul_div)
if Σ c_i > P: c_i = P · c_i / Σ c_i
P   -= Σ c_i
```

A share's credit depends only on `P` and its own weight, never on which other shares the
block includes. Leaving out other miners' shares therefore gains a block finder nothing,
and a left-out share stays includable by any later block in its window. The pool drains
over about 8 blocks, so payouts per block track the subsidy closely; whatever is not yet
credited simply waits in the pool. `subsidy + fees` minted equals pool + unpaid credits +
paid notes at every height (checked by the test suite).

## Accounts and payouts

Credits accrue per owner in an account `(amount, since)`, where `since` is the height of
the first credit after the last payout. A block at height `h` pays every account with
`since + payout_interval ≤ h` (360 blocks, about 6 hours, on mainnet), oldest first and at
most 64 per block, as **one note each**, then deletes the account. Large and small miners
alike receive a few notes per day instead of thousands of dust notes.

A payout note is built entirely from public data:

```
rcm = digest_from_uniform(H("Krypton/payout/rcm", height, index, owner))
cm  = note_commitment(owner, value, rcm)
```

`payout_digest = H("Krypton/payouts", n, (owner, value)…)` commits the payouts in the
header. The note enters the tree `maturity` blocks later (100 on mainnet), first among the
block's leaves. It is spent like any other note; the spend stays private because the
nullifier `H(nk, position)` needs the owner's secret nullifier key.

Mining income is therefore public per owner (as coinbase rewards are on every chain);
use a separate wallet for mining if you want it unlinked from your receiving address.

## Light clients

Compact blocks carry the block's payouts (checked against `payout_digest`) and the payouts
maturing in it (their commitments are recomputed from height, index, owner and value and
must reproduce `note_root`). A server cannot invent, alter or hide a payout.

## Why not pools?

Pools concentrate block templates in a few operators. Here every miner builds its own
templates and the protocol itself splits the reward: the low variance of a pool, with the
decentralization of solo mining. Mining needs a live connection, because shares only count
once they reach the network.
