# Consensus specification (v1)

All integers are little-endian. `H_tag(x)` is `SHA3-256(len(tag) as u16 || tag || x)`;
`H_tag(a, b, ...)` length-prefixes every part with a u64.

## Networks

| | mainnet | testnet | devnet |
|---|---|---|---|
| chain id | `genoryn-krypton-mainnet-1` | `genoryn-krypton-testnet-1` | `genoryn-krypton-devnet-1` |
| magic | `GKM1` | `GKT1` | `GKD1` |
| P2P / API port | 30333 / 30334 | 31333 / 31334 | 32333 / 32334 |
| address prefix | `gnr1`, short `gnrs1` | `tgnr1`, `tgnrs1` | `dgnr1`, `dgnrs1` |
| reward maturity | 100 | 20 | 2 |
| payout interval | 360 | 360 | 4 |
| PoW sizes | standard | standard | tiny (tests only) |

## Genesis

`genesis.prev = H("Krypton/genesis/v1", chain_id, message, anchor, subsidy, halving, cap,
maturity, payout_interval, share_ratio, share_window, pool_spread, max_share_weight, algorithms)`
where `anchor` is the fair-launch Bitcoin block (height and hash). Height 0 contains no
transactions and issues nothing. The PoW chain seed is the genesis block hash.

## Header (332 bytes)

```
version u32 | height u64 | prev [32] | mmr_root [32] | tx_root [32] | share_root [32]
| note_root [32] | nullifier_digest [32] | output_digest [32] | payout_digest [32]
| miner [32] | timestamp u64 | difficulty u128 | nonce u64
```

* `pow_hash = H("Krypton/header/pow", header without nonce)`; block id `= H("Krypton/block", header)`.
* `mmr_root`: root of the MMR whose leaves are `(block_hash, difficulty)` of blocks `0..height-1`
  (see [light-client.md](light-client.md)).
* `note_root`: Rescue root of the note tree after this block.
* `tx_root`: binary SHA3 Merkle tree of txids, odd nodes promoted, committed with the count.
* `miner`: the payout owner (a canonical Rescue digest) of whoever mined this header.
* `share_root`, `payout_digest`: commitments to the shares and payouts (see [rewards.md](rewards.md)).

## Block body

`header | u32 n | n × share header (332 bytes, ascending by hash) | u32 m | m × transaction`.
There is no coinbase transaction: rewards are paid by consensus.

## Validity of a block at height h

1. Version 1, `prev` = parent hash, `height = parent.height + 1`, `miner` canonical.
2. `timestamp > median(last 11 timestamps)` and `timestamp <= now + 300 s`.
3. `difficulty = LWMA(last 91 blocks)` (below); proof of work meets it.
4. `mmr_root` equals the MMR over all previous blocks.
5. Serialized size ≤ 4 MiB; at most 256 shares, strictly ascending by hash.
6. `tx_root`, `nullifier_digest`, `output_digest`, `share_root` match the body.
7. Every share is valid for this block ([rewards.md](rewards.md)).
8. Every transaction is a structurally valid transfer with `mint = 0`, its proof verifies,
   nullifiers are unique (within the block and against the chain), the anchor equals the
   note root of a main-chain block at most 2,880 blocks old, and it is not expired.
9. `payout_digest` equals the digest of the payouts computed by the reward rules.
10. The note tree after appending, **in order**, the payout notes of block `h − maturity`
    (if any) and then every transfer output, has root `note_root`.

Rule 10 defers reward notes: they cannot be spent before maturity because they are
not in any spendable tree.

## Emission

`subsidy(0) = 0`; `subsidy(h) = 500,000,000 >> ((h − 1) / 2,100,000)` gori for `h ≥ 1`
(zero after 64 halvings). Total issuance converges below 21,000,000 GNR. Subsidies and
fees enter the reward pool; nothing is minted outside it.

## Difficulty (LWMA-1)

Over the last `n = min(90, len−1)` solve times, each clamped to `[1, 360]` s using
monotonic timestamps: `next = Σ D · T · (n + 1) / (2 · max(Σ i·solve_i, n²T/20))`, with
`T = 60`. Fewer than 7 blocks of history use the network's initial difficulty.

## Fork choice

The valid chain with the greatest cumulative difficulty wins; ties keep the current chain.
Side branches are stored after header and PoW checks and are fully validated only when
they become heavier. Reorganizations use per-block undo records (nullifiers, note tree,
reward pool, miner accounts and credited shares are all restored exactly).
