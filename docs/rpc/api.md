# RPC v1

Default mainnet ports: 29610 P2P, 29611 loopback administrative RPC, 29612 optional
read-only public RPC. Devnet uses 29410-29412 and testnet 29510-29512.
Administrative endpoints answer HTTP 401 when the bearer token is missing or wrong.
No wallet secret service is exposed: wallet construction/scanning stays local.

Read endpoints:
- `GET /v1/genesis`: deterministic height-zero record, canonical preimage and hash;
  genesis creates no currency. Every network's genesis is finalized (mainnet since 0.4.0).
- `GET /v1/info`: network/chain/genesis/version, validated height/tip, difficulty,
  target-implied hash rate, peer/mempool count, inbound/outbound counts, database
  bytes, public-RPC mode and safety state. The hash-rate estimate is next
  difficulty divided by 60 seconds, not observed miner telemetry. Database bytes
  include chain/registry SQLite, WAL and SHM files, excluding wallets and logs;
  an unavailable filesystem measurement is null rather than a fabricated zero.
- `GET /v1/supply`: canonical integer issuance and reward fields as decimal strings.
- `GET /v1/blocks?from=H&limit=1`: one full encrypted block, starting at height 1.
- `GET /v1/names?name=name`: on-chain username status: `registered` (receiver, owner,
  sequence, heights, txid, burned amount) or null, `pending` (a registration in the
  mempool), `available`, `price_atomic` and `update_price_atomic`. Invalid names return
  400 with the reason. See [on-chain usernames](../protocol/usernames.md).
- `GET /v1/names/owner?owner=<64 hex>`: names registered by one owner key, plus `pending`
  registrations of that owner in the mempool.
- `GET /v1/mempool/nullifiers`: `{"nullifiers": [hex, …]}` spent by transactions waiting
  in the mempool (already public to every peer). Wallets skip coins these spend, so a
  second transfer before the next block does not conflict with the first.
- `POST /v1/relay`: a signed private transaction from a light wallet, without a
  token. It is validated exactly like one received from a peer and relayed the same
  way (it is not the node's own: no stem, not kept on disk). Answers
  `{"txid", "status": "relayed"}` or 400 with the reason.

A node started with `--public-rpc` and `--onion-service` (built-in Tor) also serves
its public RPC through its onion service on the base port + 2, for example
`http://<56 characters>.onion:29612` on mainnet; `GET /v1/network` shows it as
`tor.onion.rpc_address`. At most 16 such streams are open at once.
- `GET /v1/usernames?username=name`: legacy local signed receiver bundle, checked locally.
- `GET /v1/peers`: local administrative listener only. Each peer has `address`,
  `inbound`, `height`, `link` (`clearnet`, `tor`, `onion-inbound`) and `network`
  (`ipv4`, `ipv6`, `onion`, `name`).
- `GET /v1/network`: local administrative listener only. Network privacy mode and
  notice, warnings, clearnet listener and onion target, Tor status (SOCKS
  reachability, control-port state and authentication method, bootstrap
  progress, readiness, onion service state and address, reconnections), the
  connection mix by transport, known address counts and transport counters
  (direct and Tor attempts, failures, refusals). It never contains Tor control
  credentials or the onion private key. Public RPC never reveals any of this.
- `GET /v1/usernames/record?username=name`: local listener only; authenticated
  historical record for owner renewal, including expired records. Payment
  resolution continues to use the strict nonexpired `/v1/usernames` endpoint.

Authenticated loopback-only actions (`Authorization: Bearer <admin.token>`):
- `POST /v1/mine/template` with `{"wallet":"gnrd..."}`: genuine reward proof/template.
- `POST /v1/mine/submit`: full candidate block.
- `POST /v1/transactions`: signed private transaction; success means mempool only.
- `POST /v1/usernames`: signed ownership bundle.
- `POST /v1/network/reconnect`: re-check Tor at once and retry peers without
  waiting for their backoff (`genoryn-cli reconnect-network --token-file …`).

Public mode omits all mutating/admin routes. Administrative non-loopback binding
is rejected, tokens are not sent through redirects or to remote endpoints, and
private keys/passwords never belong in RPC requests. Requests and responses are
bounded, concurrency is limited, and proof work is serialized. Production HTTPS
and external rate limiting need an operator-managed proxy; deployment is outside
this local milestone.

The example TOML's `[limits]` section configures bounded local RPC, connection
and mempool admission. Consensus maxima remain fixed; see
[the resource-policy decision](../adr/0010-resource-policy.md).

There is no `get_balance(address)`, wallet-history lookup, rich-list endpoint,
global viewing key, token price or USD consensus field. Block totals and public
fees can support a future explorer without publishing private accounting.
