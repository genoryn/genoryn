# Shared mining (PPLNS) — design, policy and operation

GENORYN 1.1 adds an optional, open-source mining pool: `genoryn-pool` (service) and
`genoryn-miner --pool` (client). **No consensus rule changes.** A block the pool finds pays
its single coinbase output to the pool's own address, exactly like a solo block; the pool
then divides the matured reward among the miners whose verified work found it and pays them
with ordinary shielded transfers. Solo mining is unchanged and stays the default.

The pool is **custodial**: between maturity and payout, rewards sit in the operator's wallet.
It is not trustless or non-custodial, and GENORYN does not operate or endorse any pool. Anyone
can run one; miners choose and can switch at any time.

## How a reward reaches a miner

| State | Meaning | Spendable? |
|---|---|---|
| Verified shares | Work the pool hashed again with real RandomX and committed to its ledger | No |
| Estimated earnings | `window share × current reward`; a forecast, not income | No |
| Immature pool reward | The pool found a block on the canonical chain; under 60 blocks deep | No |
| Matured credit | The miner's exact share after 60 confirmations, kept by the pool | No (pool custody) |
| Pending payout | In a payout transaction the pool's wallet stored and relayed | No |
| Received | The payout transaction is in a block; it is in the miner's own wallet | Yes |

A pool only distributes rewards it actually earned: shares earn nothing until the pool finds
a block, and nothing is credited before the block is 60 blocks deep (the consensus coinbase
maturity). Estimated or pending amounts are never shown as wallet balance.

## Exact reward division

For a block with coinbase value `R` (reward plus fees), pool fee `f` basis points and PPLNS
window `W`:

```
fee            = floor(R × f / 10 000)
distributable  = R − fee
window         = the newest verified work, back to N = window_factor × block difficulty
credit_i       = floor(distributable × work_i × 2^32 / window_work)     (in 2^-32 gori)
dust           = distributable × 2^32 − Σ credit_i                      (< 2^-32 gori per miner)
```

- **Integers only.** `distributable < 2^63` and `work < 2^64`, so every product fits in 128
  bits; no floating point touches a balance.
- **Window.** Work rows are read newest first until `N` work is reached; the row that crosses
  the boundary counts only the part that fits. A young pool with less than `N` total work uses
  all of it. Defaults: `window_factor = 2`, `fee = 100` (1%).
- **Sub-gori amounts count.** Credits are kept in units of 2^-32 gori, so a 10 H/s miner's
  share of a single block is recorded even when it is far below one gori, and accumulates
  across blocks. Only whole gori are paid; the fraction stays in the balance.
- **No rounding inflation or silent loss.** `Σ credits + dust + fee = R` exactly (property
  tested with random rewards, fees and windows). Dust and fees are recorded in the operator's
  ledger row; the audit checks conservation on every status call.
- **Work, not accounts.** A share is credited with exactly its share difficulty (its expected
  number of hashes). Splitting the same hashrate across many addresses earns the same total
  (minus rounding), so fake identities gain nothing.
- **VarDiff never changes value.** Each session's share difficulty adapts so it submits about
  one share every 15 s (configurable); because a share is worth its difficulty, the expected
  credit per hash is identical at every share difficulty. A share that happens to meet the
  network target is still credited only its share difficulty.

## Activation height

The requested mainnet accounting activation is height **1150** (`MAINNET_ACTIVATION_HEIGHT`).
It is a pool-service setting, not a consensus rule, and does not affect solo miners.

- Blocks below the activation height are never shared (`ineligible`), and the gateway does not
  open for shares until the next block can reach it.
- Rewards are never backdated. A new mainnet pool whose activation height has already passed
  **refuses to start**; the operator must choose a future height and confirm it explicitly
  (`--activation-height H --confirm-activation-height`). A ledger records its activation height
  on first use and refuses to change it.
- Observed: genoryn.io reported mainnet height 1136 on 2026-10-11 ~13:12 UTC; a local node
  synchronized from the onion seeds reported 1181 at ~14:40 UTC and 1293 at 15:47 UTC. Height
  1150 has therefore passed without any pool running, and `genoryn-pool serve --network mainnet`
  refuses it (recorded output in `docs/testing/evidence/pool-activation-mainnet.txt`). Deploying a
  mainnet pool requires an explicitly approved future activation height.

## Protocol (`genoryn-pool/1`)

One JSON object per line (at most 4,096 bytes). Plain TCP is accepted by the miner only on
loopback, a private network or an onion service (through Tor); public pools use `tls://`.

```
→ {"id":1,"method":"login","params":{"address":"gnr1…","worker":"rig1","agent":"genoryn-miner/1.1.0"}}
← {"id":1,"result":{"protocol":"genoryn-pool/1","share_difficulty":1000,"pool_fee_bps":100,…}}
← {"method":"job","params":{"job_id":"gateway-1-42","height":1301,"header":"<hex>","seed":"<hex>",
     "share_difficulty":1000,"network_difficulty":13638833,"nonce_start":"<hex u64 LE>","nonce_bits":40}}
→ {"id":2,"method":"submit","params":{"job_id":"gateway-1-42","nonce":"<hex u64 LE>"}}
← {"id":2,"result":{"accepted":true,"receipt":{"hash":"…","work":1000,"sequence":81723,…}}}
```

- `header` is the canonical header encoding with nonce zero; the nonce is bytes 94..102,
  little endian. Each session owns 2^40 nonces (`nonce_start`), so sessions never repeat each
  other's work and a share cannot be resubmitted by another session.
- Every submission is hashed again by the pool. Rejections: `unknown_job`, `stale` (the chain
  moved past the job), `duplicate`, `out_of_range`, `low_difficulty`, `rate_limited`, `busy`
  (bounded queue full: backpressure, never unbounded memory), `not_logged_in`,
  `invalid_address`, `not_open_yet`. A session sending many invalid shares is closed.
- A share is acknowledged only after it is committed to the ledger; the receipt names the work
  row it was recorded in, so miners can check the pool's per-block allocation.

## Components and scaling

```
miners ──TCP/TLS/Tor──► gateway (any number) ──► verification threads (RandomX)
                              │                         │
                              └──── durable share sink ─┴──► ledger (SQLite or PostgreSQL)
operator's node ◄── templates / block submission                │
                                         accountant (exactly one) ┤  maturity, reorgs, PPLNS
                                         payout executor ─────────┘  through genoryn-wallet-cli
```

- **Gateways** are stateless apart from their sessions; several can share one PostgreSQL
  ledger (`--ledger-url-file`). Each holds its own templates from its own node.
- **Verification** is the real bottleneck and is measured separately (below). `--fast-verify`
  uses one 2 GiB full-memory RandomX dataset shared by all verification threads; hashes are
  identical to light mode (checked before every benchmark).
- **The share sink** groups verified shares per miner every 200 ms into one work row each and
  commits them in one transaction; per-share cost is O(1) and never depends on the number of
  miners. PPLNS runs once per found block over the window only.
- **PostgreSQL** transactions are serializable and retried on conflict, so concurrent gateways,
  the accountant and the payout executor never lose a balance update. SQLite takes the write
  lock up front. Redis is not used; the ledger is the only record of earnings.
- **Payouts** never mean one transaction per share or per miner per block: matured balances
  above the threshold are paid in transactions of up to 15 recipients, within the consensus
  limits (16 actions per transaction, 64 transactions per block). At most
  `64 × 15 = 960` payees fit in one block (about one per 62.5 ms network-wide), so millions of
  miners imply thresholds and batching, not instant payment of every share.

### Measured capacity (one Intel Core Ultra 9 285K, 24 logical CPUs, Windows 11, release build)

| Measurement | Result | Evidence (`docs/testing/evidence/`) |
|---|---|---|
| Light-mode RandomX share verification, 16 threads | 348 shares/s (≈5,200 miners at 15 s shares) | `pool-verify-capacity-1.1.json` |
| Full-memory share verification, 16 threads (`--fast-verify`) | 1,982 shares/s (≈29,700 miners at 15 s shares); dataset built in 52 s | `pool-verify-capacity-fast-1.1.json` |
| Ledger, SQLite, 1,000,000 simulated miners | record 1,000,000 work rows in 2.4 s; PPLNS window 0.14 s; division 21 ms; credit maturity 7.6 s; audit balanced | `pool-ledger-sqlite-1m-1.1.json` |
| Ledger, SQLite, 100,000 simulated miners | record 200,000 rows in 0.42 s; maturity 0.44 s; balanced | `pool-ledger-sqlite-100k-1.1.json` |
| Ledger, PostgreSQL 17 (Docker on the same machine), 100,000 miners | record 200,000 rows in 1.6 s; window 0.09 s; maturity 0.85 s; payout planning 20 s for 1,000 batches; balanced | `pool-ledger-postgresql-100k-1.1.json` |
| One gateway, 10,000 simulated sessions | 10,000 logged in and received a job, 0 failed | `pool-connections-10k-1.1.json` |
| One gateway, 100,000 simulated sessions (4 ports, 4 loopback sources) | 99,856 logged in with a job, 144 failed (0.14%); gateway peak working set 1.58 GB | `pool-connections-100k-1.1.json` |
| Real end-to-end pool on a devnet (2 miners, real blocks and payouts) | 13 blocks found, 11 matured, both miners paid exactly what the ledger says | `scripts/test/pool.py` |

Login latencies in the connection reports include the load generator's own queue (it opens 512
connections at a time), so they describe the ramp-up, not a single login. The million-session
target was not run as sockets: one Windows client cannot open that many connections (shared
ephemeral port range, handles). PostgreSQL payout planning still updates one row per payee and is
the next candidate for set-based statements.

Verification capacity scales with machines: at 15 s shares, 1,000,000 concurrent miners need
about 34 such machines with full-memory verification (about 9 with 60 s shares). These are
measurements of one computer, not a guarantee; simulated sessions are not independent miners.

### Load-test scenarios

```
# 10,000 / 100,000 sessions against one gateway (spread over loopback source addresses)
genoryn-pool bench connections --target 127.0.0.1:29790 --connections 10000 --address gnrd1… --hold-seconds 60
genoryn-pool bench connections --target 127.0.0.1:29790 --target 127.0.0.1:29792 \
  --source 127.0.0.2,127.0.0.3,127.0.0.4,127.0.0.5 --connections 100000 --address gnrd1… --hold-seconds 120
# 1,000,000 miners: the accounting path (no sockets) and verification capacity
genoryn-pool bench ledger --network devnet --ledger bench.sqlite --miners 1000000 --rows-per-miner 1
genoryn-pool bench verify --threads 16 --seconds 20 --fast
```

One client machine cannot open a million TCP sessions (port and handle limits); the million-
miner scenario is therefore measured as accounting at that size plus verification capacity
per machine, and labelled as such.

## Running a pool

```
# 1. Your own node (admin RPC on loopback) and a pool wallet
genorynd --network mainnet
genoryn-wallet-cli --network mainnet --wallet pool.gnr create

# 2. Gateway + accountant (SQLite on one machine)
genoryn-pool serve --network mainnet --ledger pool.sqlite --token-file ~/.genoryn/mainnet-1.0/admin.token \
  --pool-address gnr1… --activation-height H --confirm-activation-height --fast-verify true

# 3. Payouts (a separate process; the gateway never holds the wallet's keys or password)
genoryn-pool payout --network mainnet --ledger pool.sqlite --token-file … --wallet pool.gnr --password-file pool.pw

# Several gateways: --ledger-url-file pg.url, --gateway gateway-2 --account false on all but one
genoryn-pool status --network mainnet --ledger pool.sqlite      # audit, recent blocks, open payouts
```

- The read-only statistics API (`/v1/pool/status`, `/v1/pool/miner?address=…`) listens on
  loopback by default. Publishing it reveals per-address pool balances to anyone who knows an
  address; keep that in mind before exposing it.
- Payout network fees are paid by the operator (from fee income). The payout executor merges
  the pool wallet's notes when a payout would need more than 16, and pays each batch at most
  once (its id is the wallet's payment reference).
- After a reorganization: blocks that leave the chain before maturity are `orphaned` (their
  credits never count); after maturity they are `reversed` (credits taken back from unpaid
  balances; already-paid amounts are recorded as operator deficit). The audit stays exact.

## Mining in the wallet

Mining → **Shared (pool)**: enter the pool address (`tls://…` or an onion `tcp://…`), optionally a
Tor SOCKS proxy. In Tor-only mode the pool connection goes through the node's built-in Tor; its
proxy credentials are passed in the environment, never in the process arguments. The page shows
accepted shares, credited work and the five reward stages, and states the custody risk.

## Not in this release

- Native P2Pool or any non-custodial pool: it needs consensus support for multiple coinbase
  outputs, which would be a hard fork. Not done.
- Miner-signed share receipts and a published per-block allocation file: receipts carry the
  committed work row today; publishing allocations is a planned addition.
- TLS termination in `genoryn-pool` itself: run it behind a TLS terminator (or an onion service)
  for public pools. The miner already supports `tls://`.
