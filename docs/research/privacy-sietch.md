# Transaction metadata on Genoryn Krypton, and what Sietch would add

Research note for 1.0.53. Nothing here changes consensus or the wallet's cryptography; the
measurements come from the public chain with the read-only tool in
[`tools/chain-metadata`](../../tools/chain-metadata/chain_metadata.py).

## Summary

* **The transaction shape gives nothing away.** All 716 transactions on mainnet up to height
  31,907 spend exactly two notes and create exactly two. Consensus requires it, so the number
  of payments, recipients or change outputs cannot be read from a transaction. This is what
  Sietch adds to Zcash-style chains, and Krypton already has it.
* **Fees are a weak tag.** Five fee values appear. The two wallet defaults cover 93% of the
  transactions; a hand-picked fee (3 transactions) marks those transactions as one person's.
* **Size does not link transactions.** Sizes vary from 92,199 to 98,788 bytes, almost always
  differently, because the proof's length varies. The variation comes from the proof itself, not
  from the amount, the keys or the wallet, so two transactions of the same size are no more
  likely to share an owner.
* **The clearest signal is timing.** 66% of transactions share their block with others, mostly
  from note merging: a wallet with many small notes (typical after mining) merges them in
  parallel rounds, one block per round. On a quiet chain the transactions of a round are
  probably one owner's. The amounts and addresses stay hidden.
* **The network layer** is where an observer learns most. In 1.0.53 the wallet can send all its
  connections through Tor (see the README, Connection mode).

## Measurements (mainnet, heights 0 to 31,907)

| Measure | Value |
|---|---|
| Blocks / blocks with transactions | 31,908 / 328 |
| Transactions | 716 |
| Kind | transfer: 716 |
| Shape | 2 in / 2 out: 716 (100%) |
| Fee values (count) | 2,560 (455), 1,280 (209), 960 (49), 2,000 (2), 200,000 (1) |
| Fee entropy | 1.24 bits |
| Size range / distinct sizes | 92,199 – 98,788 bytes / 488 |
| Distinct proof lengths | 472 |
| Transactions sharing their block with another | 66.5% |
| Most transactions in one block | 26 |
| UTC hour-of-day entropy | 4.00 of 4.58 bits |

Reproduce it with `python tools/chain-metadata/chain_metadata.py --node https://genoryn.io`.
The tool only reads public block data and keeps a cache, so later runs only fetch new blocks.

### Reading the numbers

**Fees.** 1,280 is the minimum fee (10 per kB for a 128 kB transaction) and 2,560 the
wallet's default (twice that). 960 comes from wallets before the current fee rule. Any other
value was typed by hand. Such a fee is not a key, but it groups transactions that used it.

**Size.** A transaction is two nullifiers, two commitments, the encrypted notes and a STARK
proof of 36 queries. Each query carries authentication paths; paths that share nodes are sent
once, so the length depends on where the queries fall. That position comes from the proof's
own randomness. The measured lengths match this: 472 distinct proof lengths in 716 proofs,
spread over a narrow band. A fixed-size proof encoding (padding every proof to the maximum)
would remove even this. It would change the transaction format, which needs a consensus
change, so it is listed under future work and not done here.

**Timing.** The per-block counts show the merge rounds: 240 blocks with one transaction and a
tail of blocks with 5 to 26. The hour-of-day spread is close to uniform, but with few users it
can still hint at a time zone.

## Sietch, and what it would add here

Sietch is a privacy feature from the Hush project. Its wallets add extra zero-value shielded
outputs to every transaction, so an observer cannot count a transaction's real outputs or
build a graph of who paid how many people. It protects one metadata channel: the transaction's
shape.

On Krypton that channel is already closed by consensus: every transaction is 2 in / 2 out.
The wallet fills an unused output with a dummy note, and an unused input with a dummy input
when only one note is spent. Adding dummy outputs
is not possible without changing the transaction format, and it would bring nothing that the
fixed shape does not already give. **Sietch as such is therefore not adopted.** The channels
that remain open, ordered by what they reveal:

1. **Network origin (IP address).** Addressed in 1.0.53: an optional Tor or SOCKS5 proxy for
   all of the wallet's connections. The node already relays its own transactions to one random
   peer first and announces them to everyone only after 20 seconds.
2. **Merge clusters.** Wallet-side mitigations, none of which needs consensus:
   * merge in the background ahead of time, while the wallet is idle, with random gaps
     between rounds, so merges are not tied to the moment of a payment;
   * spread a round's transactions over several blocks with random delays (slower payments);
   * fewer, larger mining payouts, which mean fewer notes to merge.
   Each one makes payments from many small notes slower or uses more fees, so they are proposed
   for a later release with the trade-off shown to the user, not switched on silently.
3. **Fees.** Keep fees to the wallet's standard levels. A later wallet could warn before a
   hand-typed fee, because it groups the transactions that use it.
4. **Proof length.** Random and not linking; a padded proof encoding would need a consensus
   change and is left to future research.

## Limits of this study

* 716 transactions is a small sample; the numbers describe today's chain, not a steady state.
* The study sees only what is on chain. It cannot see network-level observation, which
  depends on who runs the nodes a wallet connects to.
* "Probably one owner" for a merge round is a heuristic. It holds on a quiet chain and weakens
  as more people transact in the same blocks.
