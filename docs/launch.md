# Mainnet launch

Genoryn Krypton mainnet launched on **3 November 2026**.

| | |
|---|---|
| Chain id | `genoryn-krypton-mainnet-1` |
| Genesis hash | `3c10136cd72a74d673257249752522e0102b6268f5b8c348ba34ae36740ca3b3` |
| Genesis time | `1793738591` (2026-11-03 20:43:11 UTC) |
| Bitcoin anchor | block **974,438**, `00000000000000000001075275c9ea6c90ce99dd94214d855d2b726fb173bed9` |
| Initial difficulty | 6,000,000,000 (about 80 s for one RTX 5090) |
| Premine | none: block 0 issues nothing |

## Why the anchor proves a fair start

The genesis block and the proof-of-work chain seed are derived from the hash of Bitcoin block
974,438, the newest Bitcoin block at the moment of launch. That hash could not be known before
the Bitcoin block was mined, so no Genoryn Krypton block can be older than it. Every block, from
block 1 on, adds the same 5 GNR to the reward pool, which pays every miner in proportion to the
shares it found ([spec/rewards.md](spec/rewards.md)).

## Verify it yourself

1. Look up Bitcoin block 974,438 on any Bitcoin explorer and compare its hash.
2. Build from source and start a node: `cargo build --release && ./target/release/kryptond`.
3. The node prints `genesis=3c10136c…a3b3`; `GET /v1/info` returns the same value and the anchor.

## Future networks

New networks (or a relaunch) follow the same rule, preferably with an announcement first:
announce a future Bitcoin height, wait for it, and publish the release that commits to its hash.
