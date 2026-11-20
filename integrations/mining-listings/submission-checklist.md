# Listing submission checklist

Nothing is submitted without the project owner's approval. Every number sent must come from the
live API or a real measurement on the day it is sent.

## Before any submission

- [ ] Project owner approved this submission.
- [ ] Facts in [project-metadata.json](project-metadata.json) re-checked against `https://genoryn.io/v1/info` and `/v1/supply`.
- [ ] The latest release is the one linked, with its `SHA256SUMS`.
- [ ] Hashrates name the device, driver, miner version and (for Hashrate.no) every overclock value. No estimates.
- [ ] No price, return or profit promises anywhere in the text.
- [ ] Contact goes through a project address (`@genoryn.io`), not a personal one.

## Per platform

| Platform | Blocker | Prepared |
|---|---|---|
| WhatToMine | a supported exchange (Binance, Bitfinex, Gate.io, MEXC, Nonkyc, Poloniex, SafeTrade, WhiteBIT) | [whattomine.md](whattomine.md) |
| Hashrate.no | a supported exchange, healthy volume; benchmarks need overclock data | [hashrate-no.md](hashrate-no.md) |
| MiningPoolStats | process unverified; no pools by design | [miningpoolstats.md](miningpoolstats.md) |

## Common to all

- Bitcointalk ANN thread (a draft exists outside this repository; it is posted only after approval).
- Logo: `website/assets/icon-512.png` (square PNG).
- Short description: "Post-quantum, private proof-of-work coin. KryptoHash GPU mining with native share rewards: no pools, no fees."
