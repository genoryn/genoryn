# Native share rewards compared with pool mining

| | Pool mining (most coins) | Genoryn Krypton |
|---|---|---|
| Who counts shares | the pool operator's server | every node, by consensus rules |
| Who builds block templates | the pool (one operator for many miners) | each miner's own node |
| Fee | usually 0.5 % to 3 % | none |
| Payout | by the pool, on its terms | by the chain, every 360 blocks, one note per miner |
| Can it withhold or misreport? | yes: you trust the operator | no: shares and payouts are checked by every node and committed in the block header |
| Variance | low | low (about 32 shares per block network-wide) |
| Needs an account | usually | no |
| What you need online | the pool | a node (your own, or any public one) |

## What is the same

- You still need a steady connection: shares only count once they reach the network.
- Earnings still depend on your share of the network's hashrate.
- A share found too late (outside the 32-block window, or on a fork that lost) does not count, as stale shares do not in a pool.

## What is different

- No operator can take a fee, change the payout terms, block a miner or disappear with unpaid balances.
- The block finder cannot leave other miners' shares out to raise its own reward: a share's credit does not depend on which other shares a block includes, and a left-out share can be included by any later block in its window.
- Payouts are public per address, as coinbase rewards are everywhere: mine to a wallet of its own if you want privacy for it.

There are no third-party pools for Genoryn Krypton, and none are needed. Pool-tracking sites
that list coins by their pools (such as MiningPoolStats) therefore have nothing of the usual
kind to show; see [integrations/mining-listings](../../integrations/mining-listings/).
