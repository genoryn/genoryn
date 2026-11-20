# MiningPoolStats

Status: **not submitted.**

## What is known

- MiningPoolStats (<https://miningpoolstats.stream>) lists coins with their pools, the pools'
  hashrate, the network hashrate, difficulty, the last block, 24-hour emission, price and volume.
- Its pages are rendered by JavaScript; its submission process and requirements could not be read
  from the public pages on 2026-11-20. **They are unverified.** Confirm them with MiningPoolStats
  before sending anything.
- Pools usually expose a pool-level JSON API for it (hashrate, workers, fee, last block).

## What fits Genoryn Krypton

There are no pools: every miner is paid by consensus shares. The honest data to offer is the
network itself:

| Field | Source |
|---|---|
| Network hashrate (H/s) | `https://genoryn.io/v1/info` → `hashrate` |
| Difficulty | `/v1/info` → `difficulty` |
| Height, last block | `/v1/info` → `height`, `tip`; `/v1/blocks` |
| Emission | 5 GNR per block, 1,440 blocks a day (`/v1/supply`) |
| Miners | `/v1/info` → `stats.miners_24h`, `stats.miners_last_hour` (owners with shares) |
| "Pool" | none; the protocol pays shares (fee 0 %) |

Do not present the public node as a pool, and do not report it as holding the network's
hashrate: it relays shares, it does not own them.

## Before submitting

1. Ask MiningPoolStats how they list a coin with native share rewards and no pools.
2. Price and volume come from markets; GNR has only atomic swaps. Expect that to be asked about.
