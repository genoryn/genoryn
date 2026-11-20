# Architecture audit (November 2026, release 1.0.46)

What this repository actually does today, checked against the code. Each item says where it
is implemented. Section 9 lists what does not exist, so nothing is assumed.

## 1. Chain and consensus

| | Verified | Where |
|---|---|---|
| Consensus | Proof of work, longest cumulative work; full rules in the spec | [`docs/spec/consensus.md`](spec/consensus.md), `crates/krypton-core/src/consensus.rs` |
| Block interval | 60 s target | `crates/krypton-core/src/difficulty.rs` (`TARGET_SPACING`) |
| Difficulty | LWMA-1 over the last 90 blocks, every block | `difficulty.rs` (`next_difficulty`) |
| Subsidy | 5 GNR, halving every 2,100,000 blocks | `crates/krypton-core/src/emission.rs` |
| Supply cap | 21,000,000 GNR (1 GNR = 100,000,000 krypton); no premine, no developer fund | `emission.rs` |
| Networks | mainnet (P2P 30333, API 30334), testnet (API 31334), devnet (API 32334) | `crates/krypton-core/src/params.rs` |
| Transactions | Shielded only; spend proofs are zk-STARKs (Winterfell, Rescue-Prime) | `crates/krypton-zk`, [`docs/spec/spend-circuit.md`](spec/spend-circuit.md) |
| Note encryption | X25519 + ML-KEM-1024 (FIPS 203), XChaCha20-Poly1305 | `crates/krypton-crypto` |
| Peer protocol | TCP; handshake X25519 + ML-KEM-768, ChaCha20-Poly1305 | [`docs/spec/addresses-and-p2p.md`](spec/addresses-and-p2p.md), `crates/krypton-p2p` |

## 2. Mining algorithm: KryptoHash v1

- BLAKE3-based and memory-bound. A 2 GiB dataset grows by 4 MiB per epoch of 16,384 blocks, up to 12 GiB. Light verification needs only the 16 MiB cache.
  - Code: [`docs/spec/kryptohash.md`](spec/kryptohash.md), `crates/krypton-pow/src/lib.rs`.
  - Constants: `EPOCH_LENGTH = 16_384`, `ACCESSES = 64`, `PowParams::STANDARD`.
- **Backends:** CPU (`cpu.rs`), NVIDIA through CUDA (`cuda.rs`; the driver is loaded at run time and the toolkit is not needed), AMD and Intel through OpenCL (`opencl.rs`).
  - Every GPU solution is checked again on the CPU before it is submitted.
- **It is not RandomX, Ethash or any algorithm the mining platforms list today.** A GPU needs at least the dataset (2 GiB and growing) in memory.

## 3. The miner (`apps/krypton-miner`)

- **Flags:** `--node` (default `http://127.0.0.1:30334`, env `KRYPTON_NODE`), `--address` (env `KRYPTON_MINER_ADDRESS`), `--address-file`, `--backend auto|cpu|cuda|opencl`, `--threads`, `--max-cpu`, `--devices`, `--batch`, `--json`, `--list-devices`, `--version`.
- **Talking to a node:** blocking HTTP or HTTPS (rustls).
  - `POST /v1/mining/template {"address"}` and `POST /v1/mining/submit {"job_id","nonce"}`.
  - Every 30 s it also reads `GET /v1/info` and `GET /v1/miner/{address}`.
  - There is no WebSocket and no Stratum.
- **The payout address is checked:** the template header must pay the miner's own address, or the work is refused (`verify_work`). A node cannot make a miner work for someone else.
- **Output:**
  - A status line every 10 s, as plain text or `--json`: `hashrate_hs`, `hashes`, `accepted_blocks`, `accepted_shares`, `rejected`, `height`, `device`, network fraction, expected GNR per day, unpaid.
  - In JSON mode, one line per accepted share or block.
- **What it does not have** (1.0.46): a worker name, per-device hashrates, invalid-share counters separate from rejected ones, and a local stats API.
- **Platforms:** release builds exist for Linux x64 and Windows x64. CI builds on Ubuntu, Windows and macOS. There is no arm64 build.

## 4. Native, share-based rewards (consensus)

- **How rewards are paid** ([`docs/spec/rewards.md`](spec/rewards.md), `crates/krypton-core/src/reward.rs`, `params.rs`):
  - Block rewards and fees go into a reward pool kept by consensus.
  - Shares are proofs of work at 1/32 of the block difficulty (`SHARE_RATIO = 32`). A share counts within `SHARE_WINDOW = 32` blocks, with at most `MAX_SHARES_PER_BLOCK = 256`.
  - Each miner is credited `pool · min(w, 4·D) / (8 · 32 · D)` per share (`POOL_SPREAD = 8`, `MAX_SHARE_WEIGHT = 4`).
  - Accounts are paid every `payout_interval = 360` blocks (about 6 hours), at most 64 payouts per block; coinbase matures after 100 blocks on mainnet.
- **No pool operator:** any `kryptond` accepts shares from any miner and relays them to its peers, and whoever finds the next block includes them.
- **Fees:** there is no pool fee and no developer fee.
- **Duplicates and stale shares:**
  - A share the node already has is answered as known.
  - A share already credited on chain is refused by consensus ("share already credited").
  - Shares outside the window or off the main chain are refused.
- **How a miner checks its rewards:** `GET /v1/miner/{address}` returns unpaid, since, next payout height, recent and pending shares. Payouts are ordinary notes in the wallet, and light clients check them against the block's payout commitment.
- **Tests:** `reward.rs`, `crates/krypton-core/tests/chain.rs` (`proportional_rewards_on_devnet_blocks`) and `crates/krypton-store/src/tests.rs` (`shares_rewards_and_reorgs_are_exact`).

This supports the statement: **"No external pools. No pool fees. Every miner is rewarded
directly by the blockchain in proportion to their valid shares."** A miner still needs a node
to submit to: its own `kryptond`, or a public one such as `https://genoryn.io`.

## 5. Read-only statistics

| Endpoint | Gives |
|---|---|
| `GET /v1/info` | height, difficulty, hashrate (estimated from recent blocks), supply, next subsidy, reward pool, share difficulty and ratio, payout interval, peers, version |
| `GET /v1/supply` | issued, max supply, subsidy, halving interval and next halving height |
| `GET /v1/stats?range=` | time series for charts (block time, difficulty, hashrate, issued) |
| `GET /v1/shares` | the node's pending share pool |
| `GET /v1/miner/{address or owner}` | one miner's unpaid credit and recent shares |

The public node limits requests per IP (`apps/kryptond/src/guard.rs`); mining templates have
their own budget.

## 6. Wallet

- **Desktop app** (`apps/krypton-wallet`):
  - A local axum server, reached only from this computer with a token, serves one web page (`src/ui.html`, about 2,500 lines of inline HTML, CSS and JS).
  - Keys never leave the wallet file, which is encrypted with Argon2id and an AEAD.
  - Sync runs in light mode (compact blocks checked with proof of work and FlyClient) or in full mode beside a local `kryptond`.
- **Tabs:** Wallet, History, Swap, Tokens, Mining, Network, Console, Security, About. There is no separate Settings tab: settings live in `wallet-settings.json` (`main.rs`, `Settings`).
- **Recovery phrases:** 24 words (wordlist and checksum in `crates/krypton-wallet-core`). They must never be translated.
- **Text:** English only. The Rust code returns English status sentences (swap engine `say()`, offer board, errors) that the page shows as they are.

## 7. Website

- Static HTML pages built by `website/build.py`, without npm. The build inserts the shared head, navigation, footer, SEO metadata, FAQ and release history, and adds a content hash to every asset address.
- About 23 pages and 35,000 words, English only (`lang="en"`, `hreflang="en"`).
- Caddy serves clean URLs and proxies `/v1` to the node (`deploy/caddy/genoryn.io.caddy`).
- Fonts are self-hosted Latin subsets (about 230 glyphs): Inter, Space Grotesk, JetBrains Mono. They contain no Cyrillic and no CJK.

## 8. Release, CI and tests

- **CI** (`.github/workflows/ci.yml`): `cargo fmt --check`, `clippy -D warnings`, `cargo test --workspace`, and the release tests of `krypton-zk`, on Ubuntu, Windows and macOS.
- **Release artifacts** are built offline: the Windows installer and zip, a Linux `.deb` and `.tar.gz` with the CUDA and OpenCL miner.
  - `SHA256SUMS` is signed with the hybrid post-quantum release key (`docs/release-key.pub`, ML-DSA-87 + SLH-DSA, tool `apps/krypton-release`).
  - The wallet installs updates only when that signature checks out.
  - `.github/workflows/release.yml` builds unsigned CI zips for comparison.
- **Tests:** unit and integration tests in every crate (32 test groups in 1.0.46). End-to-end tests of swaps on devnet with Bitcoin regtest live outside the repository.

## 9. Not present

- HiveOS (or any mining-OS) integration.
- Listings on MiningPoolStats, WhatToMine or Hashrate.no: KryptoHash is not an algorithm they know, and there are no pools and no exchange price to report.
- A worker name in the miner, per-device hashrate and a local miner stats API.
- Translations of the website or the wallet, and a language setting.
- arm64 builds.
