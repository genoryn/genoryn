# GENORYN Monetary Policy & Early Mining Incentives

This document describes the consensus emission schedule of **GENORYN Mainnet 1.0**
(chain ID `genoryn-mainnet-1.0`). Every rule below is enforced by every full node
when it validates a block; the source of truth is
[`crates/genoryn-core/src/emission.rs`](https://github.com/genoryn/genoryn/blob/main/crates/genoryn-core/src/emission.rs).
All arithmetic is integer arithmetic in *gori*: 1 GNR = 10^12 gori. No floating
point is used anywhere in consensus.

## Summary

| Item | Value |
|---|---|
| Maximum supply | 21,000,000 GNR (hard cap, checked on every block) |
| Genesis block (height 0) | Issues nothing. No premine, no allocation |
| Blocks 1–500 | **1,000 GNR per block**, the whole block subsidy |
| Early mining issuance | **500,000 GNR** (≈ **2.38 %** of the maximum supply) |
| From block 501 | 4.825437595129 GNR per block to the miner, halving every 2,102,400 blocks |
| Halving interval | 2,102,400 blocks (four years of 60-second blocks) |
| Treasury | 1 GNR per block for blocks 501–210,500 (210,000 GNR), **burned on mainnet** |
| Transaction fees | Paid to the miner of the block, on top of the subsidy |
| Coinbase maturity | 60 blocks |
| Target block time | 60 seconds |
| Never minted (integer rounding) | 0.000049936 GNR |

## Blocks 1–500: early mining incentive

Each of the first 500 blocks pays **exactly 1,000 GNR** to the miner of that block.
This is the *entire* block subsidy: there is no normal reward and no treasury output
on top of it. Transaction fees, as in every block, go to the miner in addition.

* Total: 500 × 1,000 GNR = **500,000 GNR**.
* Share of the maximum supply: 500,000 / 21,000,000 = **2.380952 %**.
* At the 60-second target, 500 blocks take about 8 hours and 20 minutes. The
  actual duration depends on the hashrate present at launch and on how quickly
  the difficulty adjustment follows it.

The purpose of this period is to reward early participation: running a miner and
the infrastructure around it while the network is new and small.

**There is no founder-only minting privilege.** The reward goes to whoever mines a
valid block, under the same rules as every other block. There is no special key,
no administrator mint, no premine and no allocation in the genesis block.

## Block 501 onward: normal schedule

From block 501 the miner receives the normal reward, which halves every 2,102,400
blocks:

| Epoch | Heights | Miner reward per block | Starts |
|---|---|---|---|
| 0 | 501 – 2,102,900 | 4.825437595129 GNR | launch |
| 1 | 2,102,901 – 4,205,300 | 2.412718797564 GNR | ~year 4 |
| 2 | 4,205,301 – 6,307,700 | 1.206359398782 GNR | ~year 8 |
| 3 | 6,307,701 – 8,410,100 | 0.603179699391 GNR | ~year 12 |
| 4 | 8,410,101 – 10,512,500 | 0.301589849695 GNR | ~year 16 |
| 5 | 10,512,501 – 12,614,900 | 0.150794924847 GNR | ~year 20 |
| … | each epoch halves the reward (integer right shift) | | |

The reward reaches zero after 43 epochs (height 90,403,700). There is no tail
emission.

The initial normal reward is derived from the remaining budget so the geometric
series of halvings stays within it:

```
NORMAL_BUDGET         = 21,000,000 − 500,000 (early) − 210,000 (treasury) = 20,290,000 GNR
INITIAL_NORMAL_REWARD = NORMAL_BUDGET / (2 × 2,102,400)  (integer division)
                      = 4,825,437,595,129 gori = 4.825437595129 GNR
```

## Treasury (burned on mainnet)

Blocks 501–210,500 each contain a separate treasury output of 1 GNR (210,000 GNR in
total). On mainnet the treasury receiver is a provably unspendable address, so
these coins are **burned**: they are counted as issued but can never be spent. The
amount that can ever circulate is therefore at most about 20,789,999.99995 GNR.
(On the development and test networks the treasury goes to a public test key, with
ten yearly unlock tranches starting in year 10, so that code path stays tested.)

## Lifetime issuance and the 21 million cap

| Component | GNR |
|---|---|
| Early mining (blocks 1–500) | 500,000.000000000000 |
| Normal mining (all epochs) | 20,289,999.999950064000 |
| Treasury (burned on mainnet) | 210,000.000000000000 |
| **Total ever issued** | **20,999,999.999950064000** |
| Never minted (rounding dust) | 0.000049936000 |

Every block is rejected if the cumulative issuance at its height would exceed
21,000,000 GNR. The figures above are asserted by the consensus test suite
(`emission::tests`), including a property test over random heights that issuance
never exceeds the cap and increases exactly by each block's reward.

Check any height yourself:

```
genoryn-emission --height 500
genoryn-emission --height 501
```

or ask a node: `GET /v1/supply`.

## Early-miner concentration: please read

The first 500 blocks distribute a large amount of GNR in a short time, and
whoever mines at launch receives it.

* **At launch, the only known miner is run by the GENORYN project operator**, on
  the project's own server. If nobody else mines during the first hours, the
  operator will receive most or all of the 500,000 GNR. This is stated here so it
  is never hidden.
* Early holdings may therefore be highly concentrated. Concentrated holdings can
  affect liquidity and price, and a large holder could sell at any time.
* Mining early is not a promise of profit. Early participation does not guarantee
  that independent nodes, a market or any price will ever exist.
* A small network is easier to attack. Until hashrate is spread across many
  independent miners, wait for more confirmations before treating payments as
  final.

## Relation to the retired pre-1.0 network

The retired network (`genoryn-mainnet-v1`, genesis `c5fc87d9…`) paid 6.920566971080 GNR
per block during its first 50,000 blocks: a normal reward of 4.920566971080 GNR plus a
2 GNR bootstrap bonus. Mainnet 1.0 replaces that bootstrap entirely with the 500 early
blocks described above, so the two are never added together. Coins of the retired
network do not exist on Mainnet 1.0, and reusing a recovery phrase does not carry
any balance over.
