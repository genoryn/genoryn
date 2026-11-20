# Fairness and security of native mining rewards

## What protects miners

| Threat | Why it does not work |
|---|---|
| A node makes you mine for someone else | The miner checks that the template's header pays your own owner tag and refuses it otherwise (`verify_work`). A short address is resolved by the node but checked against its hash. |
| Someone re-points your share to their address | The owner is part of the proof-of-work input: changing it invalidates the share. |
| A share is counted twice | Consensus refuses a hash that an earlier main-chain block already credited. A node answers a share it already has as known. |
| The block finder drops other miners' shares to earn more | A share's credit depends only on the pool and its own weight, never on which other shares the block includes. A dropped share can still be included by any later block in its 32-block window. |
| A huge share takes the pool | A share counts at most 4 × the block difficulty (`MAX_SHARE_WEIGHT`). |
| Rewards are invented or hidden | The header commits every payout (`payout_digest`). Every full node and every light client recomputes them from public data. |
| A hidden fee | There is none: subsidy and fees go into the pool, and the pool pays only shares. Minted equals pool + credits + paid notes, checked by the test suite at every height. |

## What miners must know

- **Stale shares:** a share outside the inclusion window, or found on a fork that lost, does not count. Keep latency low and the miner online.
- **The node you use** sees your address and your shares. It cannot steal them, but it can fail to relay them. Run your own node for a farm.
- **Privacy:** payouts are public per owner tag. Mine to a separate wallet if you want that income unlinked from your other addresses.
- **No guarantees:** earnings depend on your share of a network hashrate that changes all the time. Nothing here is a promise of income or of the value of GNR.

## Reporting a problem

Anything that could let someone earn more than their shares, or less: report it privately
([SECURITY.md](../../SECURITY.md)), not in a public issue.
