# WhatToMine

Status: **not submitted.** Requirements checked 2026-11-20 at
<https://www.whattomine.com/contacts/new>.

## What WhatToMine asks for

> When requesting for a coin to be added, please provide btctalk ANN, explorer / pool with
> API, reward halving schedule, source to fetch total coins from and supported exchange
> information. Coin must be on one of those exchanges with healthy orderbook and volume:
> Binance, Bitfinex, Gate.io, MEXC, Nonkyc, Poloniex, SafeTrade or WhiteBIT.

| Requirement | Genoryn Krypton | Ready |
|---|---|---|
| Bitcointalk ANN | draft only (genoryn-ref, not posted) | no |
| Explorer / API | `https://genoryn.io/explorer`, `https://genoryn.io/v1/info` (height, difficulty, hashrate) | yes |
| Halving schedule | 5 GNR, halving every 2,100,000 blocks (`/v1/supply`: `halving_interval`, `next_halving_height`) | yes |
| Total coins source | `https://genoryn.io/v1/supply` → `issued` (atomic units, 10^8 per GNR) and `issued_gnr` | yes |
| A supported exchange with a healthy order book | none: GNR trades only by BTC atomic swap | **no (blocker)** |

## Blocker

WhatToMine prices coins from its listed exchanges. Without a market on one of them, a request
will not be accepted. Listing on an exchange is a separate decision for the project; nothing
here should promise one.

## When it becomes possible

Send the contact form with the facts from [project-metadata.json](project-metadata.json) and
a link to the ANN thread. The algorithm (KryptoHash) is new to WhatToMine, so also give the
measured hashrates from the README with the exact device and miner version.
