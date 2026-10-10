# ADR 0013: on-chain usernames as derived state of ordinary transfers

## Context

The v1 local registry kept names per node. A name registered on one node was unknown to
every other node, and two people could hold the same name on different nodes. Users need
one global answer to "who is @max", visible from the chain, with a real cost against
squatting, without weakening payment privacy.

## Decision

Encode name operations inside ordinary kind-0 transfers: an output to a provably
unspendable burn address whose public viewing key lets every node read a signed record in
the memo. Apply valid operations in chain order to a name table that is part of the
derived chain state but not of block validity or header commitments.

Prices are burned and depend on length (1,000 / 100 / 10 GNR for 3–4, 5–6 and 7+
characters; 1 GNR per update). Names are permanent. Owner keys derive from the seed.

## Alternatives considered

- **A new transaction kind or block field.** Changes the canonical encoding, so every
  existing node would reject blocks containing it and the network would split at an
  activation height. Rejected: the devnet already has independently run nodes.
- **Gossiping signed bundles between nodes.** No global order, so partitions produce
  conflicting owners. Rejected.
- **Paying the price to miners.** Lets a miner register names for free by including its
  own transactions. Rejected in favour of burning.

## Consequences

- No fork: nodes without name support validate the same chain; registrations are just
  transfers to them. Upgraded nodes agree on names because the rules are deterministic.
- An invalid or losing registration still burns its value. Wallets check availability
  and the mempool first; two registrations of one name in the same block resolve by
  transaction order.
- Burned value stays inside the 21,000,000 GNR issuance accounting but can never be
  spent, which permanently reduces the spendable supply.
- The unspendability of the burn address rests on the hardness of the discrete logarithm
  for a hash-to-curve point on Pallas, the same assumption as Orchard's spend authority.
- Changing prices or rules later is a coordinated rule change for name state (not for
  block validity) and needs its own activation height.
