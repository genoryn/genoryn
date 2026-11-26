# The GNR market price (`gnr-price-2`, from 1.0.53)

The market price of GNR comes only from completed atomic swaps with bitcoin. Offers,
swaps in progress and outside prices never move it, and no one can set it. This page
specifies how it is computed: code in `crates/krypton-swap/src/price.rs`, used by every
offer board (`kryptond`) and by wallets that check it (`verifymarketprice` in the wallet's
console).

## Inputs

A swap counts once its GNR claim is on the GNR chain and the board has found its bitcoin
on the Bitcoin chain: paid into the swap's contract and taken out with the swap's secret.
For each swap the inputs are:

* `at`: when the board recorded it (seconds);
* `gnr`, `sats`: the amounts its wallets reported (amounts are private on the GNR chain);
* `paid`: the bitcoin the board found taken out of the contract (0 if none).

The bitcoin a swap counts for is `moved = min(paid, sats)`. Only swaps with
`moved ≥ 100,000` sat (0.001 BTC) count: many tiny swaps between one's own wallets cannot
carry the price, and each swap pays two Bitcoin network fees. Swaps whose two wallets
reported different amounts count for nothing.

The board publishes these inputs: `GET /v1/swap/history` lists every swap with `at`,
`gnr`, `sats` and `paid_sats`.

## Arithmetic

Every quantity is an integer, so the same swaps give the same price, bit for bit, on any
computer:

* price `p = sats × 10^8 × 10^6 / gnr` (sat per GNR × 10^6, integer division);
* weights in satoshi (times integer factors);
* ratios in parts per million.

The API shows prices divided by 10^6.

## The target: what the swaps of a window say

For a time `τ` and a window of `W` seconds, take the swaps with `τ − W < at ≤ τ`:

1. **Decay.** `w = moved × (2W − (τ − at)) / (2W)`: a swap at the window's end weighs half
   as much as a new one.
2. **Outliers by factor.** With weights capped as in step 5, take the weighted median `m`.
   Swaps with `p > 4m` or `p < m/4` are left out (typos and fake reports).
3. **Outliers by spread.** Recompute `m`. Then take each swap's deviation
   `d = |p − m| × 10^6 / m` and their weighted median, the MAD. A swap is left out if
   `d > max(5 × MAD, 500,000)`, unless that would leave fewer than 3 swaps. A swap within
   ±50 % is therefore never left out, so a real move is not suppressed.
4. **Worth.** A swap weighs no more than its GNR is worth at the median:
   `w ≤ gnr × m / 10^14`. An odd price cannot buy weight with one large side.
5. **Share cap.** No swap weighs more than a fifth of the total. The cap is the largest
   `c` with `c ≤ (Σ min(w, c)) / 5`, found by iteration. With five swaps or fewer, every
   swap weighs as little as the least.
6. **Block cap (new in v2).** Swaps are grouped in six-hour blocks (`at / 21,600`). No
   block weighs more than a quarter of the total; its swaps are scaled down if it would.
   With four blocks or fewer, every block weighs as much as the lightest. A burst of
   swaps in a few hours, large or small, cannot outweigh the rest of the window. A move
   that lasts collects weight block after block.
7. **Median.** The target is the weighted median of `p`. Swaps are sorted by price; the
   target is the first price where the running weight reaches half the total. If it
   reaches exactly half, the target is the midpoint with the next price.

The window is the narrowest of these that has enough swaps:

* 24 hours, only when they hold at least 8 swaps in at least 3 of the day's six-hour
  blocks (a busy market);
* otherwise 72 hours, then 7 days, then 30 days, each needing 3 swaps.

## The published price

The price is rebuilt hour by hour over the last 7 days, starting from the hour boundary
7 days back. Each hour it moves toward that hour's target, by at most 10 % of the
current price. The last, partial hour moves by its share of that. An hour without a
target leaves the price where it is and saves no larger move for later. The price is
`None` until some hour has a target. Restarts change nothing: anyone with the same swaps
gets the same price.

## Confidence and status

Confidence, 0 to 100, multiplies four factors, each between 0 and 1:

| Factor | Formula | One half at |
|---|---|---|
| Liquidity | `sats / (sats + 1,000,000)`, over the bitcoin behind the target | 0.01 BTC |
| Effective swaps | `ESS / (ESS + 3)`, with `ESS = (Σw)² / Σw²` (Kish) | 3 swaps |
| Agreement | `0.15² / (0.15² + MAD²)` | 15 % spread |
| Freshness | 1 while the newest swap is under 6 h old, falling linearly to 0.2 at 72 h | about 47 h |

The status names the weakest point:

| Status | When |
|---|---|
| `unavailable` | no swap's bitcoin was ever found |
| `insufficient` | no window has 3 swaps of at least 0.001 BTC |
| `stale` | the newest such swap is more than 72 hours old |
| `low_liquidity` | less than 0.005 BTC, or fewer than 3 effective swaps, behind the target |
| `low_confidence` | confidence below 40 |
| `moderate` | confidence 40 to 69 |
| `reliable` | confidence 70 and above |

While there is no published price, the board still shows an estimate as before 1.0.48
(`status: forming`): the volume-weighted median of the verified swaps, or the average of
the reported ones. Its confidence is 0 and its depth `thin`, and Quick Swap keeps to
10 % of it.

## What the board shows

`GET /v1/swap/market` keeps every field of earlier versions and adds:

* `algorithm`, `reference_price`, `market_status`;
* `confidence_score` (0–100; `confidence` is the same divided by 100);
* `change_24h` (percent), `freshness_secs`;
* `ess`, `window_secs`;
* `verified_volume_24h` (sat), `spread` (best ask over best bid, percent).

`price` is the published price when there is one. `last_price`, `best_ask` and
`best_bid` are what they say. None of them is the price you can actually trade at: that
is the best offer on the side you take.

## Comparison and simulations

The simulator in `price.rs` (`cargo test -p krypton-swap sim_table -- --ignored --nocapture`)
builds deterministic random markets with a known true price of 5,000 sat per GNR. Honest
swaps come once an hour on average, with 0.002 to 0.01 BTC each and 2–3 % noise. Attacks
come from a swapper trading with himself during the last six hours. The table shows each
method's price after the scenario: the error on an honest market, the price 30 hours
after a real move, or the move an attack causes.

| Scenario | VWAP 72h | TWAP 72h | median 72h | vw median 72h | v1 (1.0.48) | v2 (1.0.53) |
|---|---|---|---|---|---|---|
| Honest market: mean error | 0.29 % | 0.25 % | 0.33 % | 0.34 % | 0.36 % | 0.75 % |
| Real move 5,000 → 8,000, 30 h later | 5928 | 6336 | 5114 | 5086 | 5644 | 7984 |
| Wash trading: 300 × 0.001 BTC at 2× | +24.9 % | +81.3 % | +100.0 % | +2.4 % | +1.8 % | +3.4 % |
| Micro-trades: 3,000 × 0.0009 BTC at 10× | +0.2 % | +0.1 % | +0.4 % | +0.5 % | +0.5 % | +0.8 % |
| One whale: 1 × 0.5 BTC at 2× | +35.6 % | +1.6 % | +0.4 % | +100.0 % | +1.3 % | +1.4 % |
| Ten swaps: 10 × 0.05 BTC at 1.5× | +21.2 % | +6.4 % | +0.7 % | +50.0 % | +47.7 % | +2.8 % |
| Outliers: 5 × 0.002 BTC at 100× | +2.4 % | +669.0 % | +0.6 % | +0.6 % | +0.5 % | +0.8 % |

What this shows:

* **Plain averages are easy to move.** VWAP and TWAP move by tens of percent under every
  attack, and TWAP by 669 % under five outliers.
* **Count-based medians are slow and fall to many small swaps.** The plain median is
  moved +100 % by 300 swaps of 0.001 BTC.
* **Bitcoin-weighted medians fall to a few large swaps.** v1 is moved +47.7 % by ten
  swaps of 0.05 BTC. A swapper trading with himself only pays network fees for that,
  because the same bitcoin is reused.
* **v2 resists all of these.** Every attack moves it by less than 4 %, and a real move
  shows within a day (7,984 after 30 hours; v1: 5,644). The 24-hour window of a busy
  market and the block cap make this possible.
* **The cost is a little noise.** On a quiet honest market v2 errs by 0.75 % instead of
  0.36 %, because a busy day's window holds fewer swaps.

**Calibration of the confidence.** Over 400 random markets (0.05 to 3 swaps an hour,
1–30 % noise, 0–60 hours since the last swap), the mean error of the target falls with
confidence:

| Confidence | 0–19 | 20–39 | 40–59 | 60–79 | 80–100 |
|---|---|---|---|---|---|
| Mean error | 7.0 % | 3.1 % | 2.2 % | 1.2 % | 0.5 % |
| Markets | 54 | 64 | 41 | 51 | 20 |

The score therefore means what it says, and the wallet and genoryn.io show it. The test
suite asserts that this order holds (`simulations`).

**On the real board** (26 November 2026: 13 swaps, 7 of them verified, 2 of at least
0.001 BTC), v2 gives no published price yet (`insufficient`). The shown estimate stays
120 sat per GNR, as before. The confidence falls from v1's 0.996 (`deep`) to 0 (`thin`):
v1 counted GNR volume, and at a low price much GNR is little bitcoin.

## Limits

* **Wash trading cannot be fully told from trading.** In a permissionless, private
  market, anyone can trade with themselves through real contracts and pay only network
  fees. The defences limit how much this buys:
  * the 0.001 BTC minimum and the share and block caps;
  * the 10 %-an-hour limit;
  * statuses that say when a price rests on little.

  A swapper who trades with himself steadily for days, with more bitcoin than everyone
  else, moves the price. Such trading is indistinguishable from demand and pays fees
  every hour.
* **Amounts are reported by the two wallets of a swap.** The board checks the bitcoin
  on the Bitcoin chain. The GNR amount is private on the GNR chain and is taken from the
  report; the outlier rules and the worth cap bound what a false one can do.
* **The board records the swaps.** Any board can compute the price from its own
  records, and anyone can recompute a board's price from its history (`verifymarketprice`).
  Two boards with different records may show different prices.
* **Dollar values** come from the bitcoin price of mempool.space through genoryn.io
  (`/v1/btc-price`). They are shown only as a convenience and never enter the GNR price.
  Swaps and the board work without them.
