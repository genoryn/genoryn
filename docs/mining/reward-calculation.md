# How a mining reward is calculated

The exact rule, with the constants in force today (`crates/krypton-core/src/reward.rs`,
`params.rs`; specification: [`docs/spec/rewards.md`](../spec/rewards.md)).

## Per block

For a block at height `h` with difficulty `D`:

```
P  += subsidy(h) + fees of the block          (the reward pool)
for every share i the block credits (its own header first), with weight w_i = its difficulty:
    c_i = P · min(w_i, 4·D) / (8 · 32 · D)    (integer arithmetic)
if Σ c_i > P:  c_i = P · c_i / Σ c_i
P  -= Σ c_i
```

- `subsidy(h)` = 5 GNR, halved every 2,100,000 blocks.
- A share normally has the block's difficulty `D` as its weight, so `c_i ≈ P / 256`. With about 32 shares per block, a block pays out about `32/256 = 1/8` of the pool, and the pool settles at about 8 × the subsidy.
- In steady state the pool pays out about one subsidy (plus fees) per block, so each share is worth about `(subsidy + fees) / 32`.

## Example (illustrative, not a forecast)

The network finds 32 shares per block and your rig finds 1 of them on average (about 3 % of
the hashrate). At 5 GNR per block and 1,440 blocks a day:

```
your shares per day    ≈ 1 × 1,440 = 1,440
your credit per day    ≈ 1,440 × 5 / 32 ≈ 225 GNR
```

If the network's hashrate doubles, you find half as many shares per block and earn half as
much. The wallet and `krypton-miner` show the same estimate from the live network hashrate
(`/v1/info`); it is labelled as an estimate.

## When you are paid

Credits collect in your account. At the first block where `since + 360 ≤ height`, the
account is paid as one note (at most 64 accounts per block, oldest first). The note is
spendable 100 blocks later on mainnet. `GET /v1/miner/<address>` shows `unpaid`, `since` and
`next_payout_height`.
