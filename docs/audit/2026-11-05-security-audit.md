# Security audit of Genoryn Krypton 1.0.6 (fixed in 1.0.7)

**Date:** 5 November 2026 · **Code reviewed:** 1.0.6 (`6de8e57`), live mainnet at height ≈1,550 ·
**Fixes released in:** 1.0.7

*Internal, AI-assisted review. Four independent reviewers each covered one area in depth
(node and peer-to-peer layer; consensus and rewards; wallet and light client; website,
explorer and deployment). Every finding was checked against the code before it was fixed.
The live server was inspected read-only, and the dependency tree was scanned with
`cargo audit` and `cargo deny`. This is not a substitute for an audit by a specialised firm.
It follows the [1.0.2 review](2026-11-security-review.md), and all of that review's fixes
were confirmed still in place.*

## Summary

No issue was found that lets anyone mint coins beyond the schedule, spend someone else's
coins, break the privacy of transfers or forge a block. The cryptographic core (zk-STARK
spend proofs, post-quantum note encryption, release signatures) and the reward
accounting held up.

The serious findings were at the edges:

* **Wallet UI and light clients.** A malicious node could inject script into the desktop
  wallet window (W-1). A light wallet could be shown a cheap, low-difficulty fake chain
  (W-2).
* **Denial of service.** A public node could be made to do unbounded work through a
  crafted request (N-1), to fill its disk with low-work side branches (N-2), or to stall
  its block download (N-3).
* **Payment proofs on the explorer.** The explorer called a proof "valid" next to a
  recipient the payer had chosen (E-1).

All of them are fixed in 1.0.7. **1.0.7 changes no consensus rule**: blocks valid under
1.0.6 are valid under 1.0.7 and vice versa, and all versions interoperate. Two economic
findings need a consensus change and are scheduled rather than patched (R-1, R-2).

| Area | Critical | High | Medium | Low | Info | Fixed in 1.0.7 |
|---|---|---|---|---|---|---|
| Wallet and light client | 1 | 1 | 4 | 7 | – | all |
| Node, API and P2P | – | 3 | 6 | 5 | 1 | all |
| Consensus and rewards | – | – | 1 | 4 | 4 | 3 Low fixed; R-1/R-2 scheduled |
| Website, explorer, deployment | – | 1 | 2 | 4 | 1 | all except pinned CI actions |
| Dependencies | – | – | – | 1 | 3 | all |

## Findings and fixes

### Wallet and light client

| ID | Severity | Finding | Fix in 1.0.7 |
|---|---|---|---|
| W-1 | **Critical** | Block summaries and chart data from a node were placed into the wallet window as HTML without escaping. A malicious or impersonated node could run script there and send funds from unlocked wallets. | All node values are rendered as numbers or escaped text. The wallet backend also strips markup from any node JSON before the UI sees it, and the window has a Content-Security-Policy that blocks contact with any other host. |
| W-2 | High | Light-client verification accepted the difficulty a server claimed, and recent headers were optional. A server could present a chain of difficulty-1 headers at almost no cost, showing fake balances or making a forged payment proof verify. | Every release carries a **checkpoint** (block 1,400 for mainnet). Light clients require it in the chain via an MMR proof and refuse any header above it with less than 1/16 of its difficulty. The difficulty rule must be checkable on the latest 91 headers, and unrequested samples are rejected before any proof-of-work work. |
| W-3 | Medium | The multi-node cross-check compared a height that the first node could steer, for example to genesis. | Nodes must reach the checkpoint and also agree on it. |
| W-4 | Medium | Unrequested FlyClient samples could keep the sync busy, and the 24-hour lock ran inside that same task. | Samples are limited to the requested heights. The 24-hour lock has its own timer, and handlers that use a wallet (send, proof, showing the phrase) refuse an expired one immediately. |
| W-5 | Medium | A node could trigger a rollback, or a "foreign chain" reset, through unverified headers. | Rollbacks and resets happen only when the proven chain no longer contains the wallet's last block, and the fork point is found against the proven chain. |
| W-6 | Medium | A release without signed checksums still produced an update notice and a download link. | An update is announced only after the release key's signature over `SHA256SUMS` verifies. The page link is built by the wallet, not taken from the API. |
| W-7 | Low | Payment-proof confirmations used the node's claimed height. | Recorded; a proof itself is bound to a proven header (unchanged). |
| W-8 | Low | Node and update responses were read without a size limit. | Every response is read through a limited reader (64 MiB from nodes, 1 MiB for checksums). |
| W-9 | Low | Remote nodes could be configured over plain HTTP. | Remote nodes must use `https://`; plain HTTP is allowed only on this computer. |
| W-10 | Low | The installer was written to a predictable path and not re-hashed before it ran. | It goes to a fresh random folder, is created exclusively, and is hashed again just before launch. |
| W-11 | Low | Project links allowed `..` segments (`github.com/genoryn/../other`). | Dot and empty path segments are refused. |
| W-12 | Low | Some lock handlers blocked the async runtime, and a wallet switch during sync could cache one wallet's balance under another. | The lock handlers run on blocking threads. Synced data is attributed by the wallet's fingerprint. |
| W-13 | Low | Passwords and phrases from requests stayed in memory. | They are zeroised when the request is dropped. The session token is compared in constant time. |

### Node, API and peer-to-peer

| ID | Severity | Finding | Fix in 1.0.7 |
|---|---|---|---|
| N-1 | High | `/v1/subtree/{chunk}`: a huge chunk number wrapped the bounds check (`chunk << 12`), and the node then hashed across the whole note tree for every request. | Chunk counts are compared without shifting. Verified live: the attack request returns 404 immediately. |
| N-2 | High | Blocks of low-work side branches were stored in full. A cheap low-difficulty fork from deep history could fill a node's disk. | Side-branch blocks are stored only while their branch is within about 1,000 blocks of work of the main chain. |
| N-3 | High | The set of blocks being downloaded had no size limit or expiry. A peer could exhaust memory, or stall sync by announcing a block it never sent. | Each entry records the peer and time and expires after 30 s, with total and per-peer caps. Lost requests are retried elsewhere, and unsolicited `Hashes` messages are ignored. |
| N-4 | Medium | Stale shares were cheap to spam in batches below the per-message threshold. | Shares not on the main chain are rejected with one lookup. Uncreditable shares are counted per peer across messages. |
| N-5 | Medium | `/v1/mining/submit` copied a full block (up to 4 MiB) and hashed twice on the async runtime. | Only the header is copied; one hash is computed on a blocking thread. |
| N-6 | Medium | Rate limits were global, so one client could lock out honest miners and publishers. | **Per-client budgets** for every endpoint class, with IPv6 grouped by /64; the real client address comes from the proxy. A global cap on concurrent requests and a 30 s timeout. Mining jobs are bounded by bytes (256 MiB), and stale templates are pruned. |
| N-7 | Medium | Transactions with failing proofs submitted over HTTP cost CPU without penalty. | Per-client budget plus at most four concurrent proof verifications. |
| N-8 | Medium | Inbound P2P slots could be held without the peer ever introducing itself. | A peer must send Hello within 30 s. Limits and bans apply per IPv6 /64. |
| N-9 | Medium | WebSocket connections were unbounded and could hang on a non-reading client. | At most 256 subscribers, a 10 s send timeout and 1 KiB incoming messages. |
| N-10 | Low | Heavy reads (`/v1/blocks`, `/v1/flyclient`, `/v1/mmrproof`) ran on the async runtime. | They run on blocking threads. |
| N-11 | Low | Hello could add many addresses and claim any height. | Only the first Hello adds an address, the book is capped at 5,000 entries, and claimed heights are capped by elapsed time. |
| N-12 | Low | A full send queue dropped frames silently. | The peer is disconnected instead. |
| N-13 | Low | `/v1/verify-payment` could not confirm recipients. | The node resolves published short addresses (see E-1). |
| N-14 | Low | CORS allowed any site to drive POST endpoints from a visitor's browser. | CORS allows only GET. |
| N-15 | Info | `/v1/mmrproof` accepted heights above the tip. | Refused. |

### Consensus and rewards

| ID | Severity | Finding | Status |
|---|---|---|---|
| R-1 | Medium | A miner that withholds its shares and includes them only in its own blocks earns about 4–6 % more than its hashrate share, at honest miners' expense. The cause: credits are taken from the pool at inclusion time. | **Scheduled, consensus change.** Credit each share from the pool as it was when the share was mined. This will be activated at an announced height together with R-2. |
| R-2 | Low | When summed share weights exceed the pool budget (after a large difficulty drop), credits are scaled together, and leaving others' shares out raises the finder's own credit. | **Scheduled, consensus change** (lower `MAX_SHARE_WEIGHT`, charge the shortfall to the excess shares only). |
| S-1 | Low | A block rejected as "from the future" during a reorganisation was marked invalid for good (a clock step could split a node off). | **Fixed:** "future" is never remembered as invalid. |
| S-2 | Low | Database read errors during validation read as "not spent" or "no account". | **Fixed:** any failed read during validation becomes a storage error. The block is neither accepted nor marked invalid. |
| P-1 | Low | Templates picked pending shares by hash, so old shares could expire uncredited. | **Fixed:** the oldest shares are picked first. |
| I-1 – I-4 | Info | Deep reorgs return honest shares' credit to the pool. Fees are shared with the pool. Payout commitments are predictable (duplicate leaves are harmless because nullifiers depend on position). Nullifier order in undo records is non-deterministic but used only as a set. | Recorded. |

### Website, explorer and deployment

| ID | Severity | Finding | Fix in 1.0.7 |
|---|---|---|---|
| E-1 | High | The explorer showed "Valid payment proof" next to a recipient the payer chose, so a scammer could "prove" a payment to someone else. | The node now resolves the recipient against published addresses. The explorer and the wallet say **"Real payment, recipient not verified"** unless the recipient is proven, and "Not a payment to that address" on a mismatch. |
| E-2 | Medium | No Content-Security-Policy and no protection against framing. | Strict CSP (`script-src 'self'`, no inline script anywhere), `frame-ancestors 'none'`, `X-Frame-Options`, COOP and HSTS. Headers are also applied to the API. |
| E-3 | Medium | Publishing and mining limits were global (see N-6). | Per-client budgets, with the client address taken from Cloudflare. The origin accepts web traffic only from Cloudflare, so that address can't be forged. |
| E-4 | Low | Explorer values from the API were interpolated into HTML (safe only because the node sends hex and numbers). | Hashes must be hex, numbers are coerced, and all text is escaped. |
| E-5 | Low | One 8 MiB body limit for every route. | 64 KB for every route except transactions and JSON-RPC (8 MiB). |
| E-6 | Low | Miner service hardening could be tighter. | Recorded (the node service is fully sandboxed; the miner runs unprivileged with no capabilities). |
| E-7 | Low | CI actions are pinned by tag, not commit SHA. | Open; releases are built locally and signed, CI artifacts are never published. |
| E-8 | Info | A malformed `#/address/%E0` link froze the explorer view. | Handled with an error message. |

### Dependencies

`cargo audit` over 612 crates: `zip` 2.6.1 had four advisories (RUSTSEC-2026-0367, -0368,
-0369, -0429), all in `ZipArchive::extract`. The installer never calls it: it extracts
each entry itself through `enclosed_name()`, from an archive embedded at build time.
Upgraded to `zip` 9 anyway; **0 vulnerabilities remain**. Accepted warnings:
`proc-macro-error` and `glib` (Linux GTK window stack, compile-time and windowing only) and
`ttf-parser` (unmaintained, reads only the installer's embedded font). `cargo deny`: bans and
sources pass.

### Live server

Ubuntu 24.04 LTS with automatic security updates and fail2ban. SSH accepts keys only.
The node and miner run as an unprivileged user with systemd sandboxing (`ProtectSystem=strict`,
no capabilities, `MemoryDenyWriteExecute`); the node API listens on 127.0.0.1 only. **No
mainnet recovery phrase or key is stored on the server**; the miner knows only its payout
address.

Hardened in 1.0.7:

* Web ports accept only Cloudflare.
* SSH: forwarding disabled and `MaxAuthTries 3`.
* The unused testnet P2P port is closed.

Remaining operator recommendation: restrict SSH to known addresses.

## What was checked and found correct

* **Emission and supply:** subsidy and issuance never exceed the cap, the reward pool's only inflow is subsidy plus fees, total credited never exceeds the pool, every payout equals its account, and all additions are checked.
* **Shares:** the proof of work covers every field including the payout owner, so shares can't be redirected. Shares can't be credited twice; the window and history arithmetic can't overflow; withholding to exploit weight caps and censoring others' shares both lose money.
* **Connect and undo:** pool, accounts, payout queue, shares, nullifiers, payouts, note tree, anchor roots, frontiers and transaction index are restored exactly. The 1.0.2 anchor-root fix holds.
* **Difficulty:** LWMA with monotonic timestamps and clamped solve times, median-time-past, and a future-drift limit. There are no floats and no hash-map iteration in consensus paths.
* **Message decoding:** every length is bounded before allocation, trailing bytes are rejected, and frames are capped. Bad proofs and malformed data get peers banned.
* **Desktop wallet:** every API route requires the 192-bit session token. It is served on 127.0.0.1 with no CORS, which blocks CSRF and DNS rebinding. Navigation is restricted to project links, there is no path traversal, the lock order is consistent, and the 24-hour lock uses the later of the monotonic and wall clocks.
* **Updater:** strict `vX.Y.Z` tags, the installer name bound to the version, both post-quantum signatures required, the hash compared, and downgrades refused.
* **Wallet file and phrase:** Argon2id (256 MiB) parameters are authenticated, writes are atomic, and buffers are zeroised. Phrases carry 384-bit entropy with a checksum.
* **Light sync:** compact blocks are hash-linked and checked against output, nullifier and payout digests and the note root. Notes are decrypted only if their commitment matches. Spend witnesses are checked against the verified root.
* **Payment proofs:** the recipient binding and forged-recipient detection from 1.0.2 hold, and proofs are bound to proven headers.
* **Website:** no third-party scripts, fonts or analytics. No secrets in the repository or its history. Workflows run with read-only permissions and never use untrusted input in shell steps.

## Transaction fees (changed in 1.0.7)

Fees were effectively fixed at 0.002 GNR per transfer, which would become expensive if
GNR's value rose. Fees are **not** a consensus rule; any fee is valid in a block. 1.0.7
makes them a market:

* **Relay floor:** lowered 100-fold to 10 gori per KB, about 0.00001 GNR for a typical 100 KB transfer.
* **Estimates:** nodes publish `GET /v1/fee`, with economy, normal and fast rates from the mempool backlog. While blocks have room, every estimate is the floor.
* **Wallets:** the desktop wallet offers Economy / Normal / Fast / Custom, shows the exact fee before sending, and refuses fees above 0.1 GNR as a likely typo. The CLI uses the normal estimate by default (`--fee` overrides it).

## Verification

* **Tests:** `cargo test --workspace --release`: 72 tests, 0 failures. There are new regression tests for the checkpoint, the difficulty floor, too few recent headers, unrequested samples, client budgets and the fee floor.
* **Clippy:** `cargo clippy --workspace --all-targets -D warnings`: clean.
* **Live node checks:** the subtree attack returns 404. Per-client limits apply through the proxy (five publishes, then HTTP 429) but never to local software. An empty forwarded address is limited rather than trusted. CORS allows only GET.
* **Live light sync against mainnet with the checkpoint:** `verified 1554/1554`.
* **Multi-wallet end-to-end suite:** 32 checks, all passing.
