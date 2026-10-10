# Network privacy and hardening review, October 2026

Scope: the node's P2P layer, discovery and seeds, transaction relay, RPC, node
startup and configuration, the wallet's network traffic and storage, and the
consensus paths touched by synchronization. This is an internal engineering
review, **not an independent cryptographic or security audit**. No consensus
rule, monetary rule or cryptographic construction was changed.

## Confirmed issues (reproduced in code) and their resolution

| # | Severity | Finding | Resolution |
|---|---|---|---|
| 1 | High (privacy) | A `.onion` name given as a DNS seed (`--dns-seed`, `[p2p] dns_seeds`) passed validation and was resolved with the system resolver, disclosing it to local DNS. The RPC clients (`genoryn-connect`, the wallet CLI) would likewise resolve an `https://….onion` endpoint through system DNS. | Onion names are parsed as onion addresses or rejected, never resolved; the system resolver is unreachable for them in every mode and for all names in tor-only mode; RPC clients refuse onion endpoints. Unit and integration tests. |
| 2 | High (privacy) | The handshake identity came from a persistent `peer-id` file and was sent to every peer, linking a node across restarts, IP changes and (with Tor) transports. | Random identity per run; a separate identity on Tor links; Tor links advertise no listen address. The old file is ignored. |
| 3 | Medium (eclipse) | Inbound and outbound connections shared one 16-permit pool with an inbound limit of 16, so inbound peers (four per IP) could take every permit and prevent all outbound connections. | Inbound peers may use only `connections - outbound_connections` slots; default 24 = 16 + 8. |
| 4 | Medium (availability) | Any `accept()` error (for example descriptor exhaustion during a connection flood) was returned from the P2P loop, which stopped the node's main task. | Accept errors are logged and the loop continues after a short pause. |
| 5 | Medium (privacy) | A transaction created by the node went to every peer at once after one shared delay, and relays were echoed back to their sender, giving first-spy observers a strong origin signal. | One-hop stem plus embargo for local transactions; per-peer exponential delays; a shared delay for inbound peers; no echo to the sender. |
| 6 | Low (eclipse) | Outbound selection had no network-group diversity; all eight outbound peers could come from two /16s. | At most two outbound peers per group (explicit peers exempt). |
| 7 | Low (privacy) | No proxy or Tor support existed: P2P, seed resolution and the published seed list always used direct connections and the system resolver. | Hybrid and tor-only modes with fail-closed policy; see [network privacy](../network-privacy.md). |
| 8 | Low (privacy) | The wallet checks for updates at launch by default, over direct HTTPS with system DNS; this was documented but not configurable per privacy need. | Unchanged in direct/hybrid (documented); routed only through Tor in tor-only mode, failing closed. |

### Follow-up: built-in Tor and delivery (same branch, later the same day)

| # | Severity | Finding | Resolution |
|---|---|---|---|
| 9 | High (availability, Windows) | The embedded Arti client never finished bootstrapping on Windows (stuck at 15 %, one core busy). Root cause: `saturating-time` 0.5.0 searches for the platform's `SystemTime` limits by halving a step to 1 ns and stops only when a step fails; on Windows (100 ns clock) sub-tick steps always succeed, so the search never ends. `tor-netdoc` calls it for every consensus. Reproduced with a ten-line program outside GENORYN. | Vendored 0.5.0 with a termination fix and lazy evaluation, byte-verified provenance, a coarse-clock regression test that fails on the original; Arti bootstraps on Windows. [ADR 0015](../adr/0015-built-in-tor-client.md). |
| 10 | High (availability over Tor) | Peer frames were written without a flush. TCP sends immediately, but Tor streams buffer until a cell fills, so handshakes over Tor timed out on both sides. | One write per frame plus flush; regression test with a buffering writer (fails without the flush). Verified onion-to-onion on the live Tor network. |
| 11 | Medium (censorship) | A dropping stem peer plus any second peer echoing the transaction back stopped every wider broadcast of a local transaction. | An echo from a non-stem peer is relayed onward once like any transaction; otherwise one broadcast at embargo end. Adversarial test fails on the old behaviour. |
| 12 | Medium (reliability) | A local transaction created without an acceptable peer (for example during a Tor outage) was abandoned after about two minutes and stayed unsent in the local mempool; a restart lost it. | No deadline for the stem search (Tor links only in tor-only mode); new peers receive the mempool; own pending transactions persisted and re-validated after restart. Verified with a 300 s simulated outage and a restart in the middle. |
| 13 | Informational (dependency) | `cargo audit` reports RUSTSEC-2023-0071 (Marvin timing attack on RSA private-key operations) for `rsa` 0.9.10, pulled in by Arti. No fixed version exists. | Not reachable here as far as reviewed: a Tor client and v3 onion service only verify RSA signatures; GENORYN creates no RSA keys and onion keys are ed25519/x25519. Ignored in `.cargo/audit.toml` with this justification, approved by the maintainer. |
| 14 | Release blocker (licensing) | 55 resolved crates (47 Arti crates, plus 8 others, one of them `LGPL-3.0-or-later OR MPL-2.0`) ship no license text in their archives; the packager fails closed on them. | Fixed: upstream license files pinned by commit and SHA-256 in `third-party/license-supplements`; `priority-queue` used under MPL-2.0. |

## Theoretical or residual risks (not fixed)

- **Low-difficulty side branches.** Sync has no headers-first download or
  minimum chain work. Valid side blocks from early, cheap heights are stored, and
  each side block replays the active chain up to its fork point. Senders of
  far-behind side blocks now earn strikes, but onion-service peers cannot be
  banned by address and storage remains bounded only by attacker effort.
  Fixing this needs new messages and a versioned rollout.
- **Sybil and eclipse.** Netgroup quotas, reserved outbound slots and onion
  quotas raise the cost but do not prevent a well-resourced attacker from
  dominating discovery for a new node. Onion identities are free.
- **Stem peer and timing.** The first relay peer of a local transaction learns
  its origin; a well-connected observer can still use timing. Full Dandelion++
  was evaluated and deferred (see the network privacy document).
- **Pending transactions on disk.** `local-transactions.json` shows, to anyone
  who can read the data directory, which pending transactions this node
  created (shielded bytes only, deleted once they leave the mempool).
- **Mempool to new peers.** Each new connection receives up to the mempool
  limit (256 transactions); a peer that connects repeatedly can make the node
  resend it. Bounded by connection limits and the per-peer relay queue.
- **Arti maturity.** The embedded client and especially its onion-service code
  are younger than C Tor; operators can keep using an external Tor.
- **Public relay and onion RPC.** With `--public-rpc`, anyone can hand the node a
  transaction (`POST /v1/relay`), which costs the same validation as a peer relay
  and is bounded by the public RPC limits; through the onion service all such
  clients look alike, so no per-client limit exists beyond 16 concurrent streams.
- **Light wallet trust.** Remote sources learn sync timing and the transactions a
  light wallet hands them, and can withhold blocks; validation is local and full.
- **Tor exits.** In tor-only mode, clearnet peers are reached through exits that
  can read, delay or alter unencrypted P2P traffic.
- **Windows file permissions.** The onion key, wallet files and admin token rely
  on the user-profile ACL of their directory; no explicit ACL is applied.
- **Clipboard.** Copied payment proofs and addresses are never cleared.
- **Wallet password in memory.** The GUI keeps the password (zeroizing buffer)
  while unlocked so it can run CLI operations; memory scraping by local malware
  is out of scope.
- **Control-port passwords.** With `HASHEDPASSWORD`, the password is sent to
  whatever answers on the configured control address. SAFECOOKIE is preferred.

## Reviewed without new defects

Wallet storage (BIP39 24-word entropy from the OS RNG, Argon2id m=64 MiB t=3 p=1,
XChaCha20-Poly1305 with version/network/salt/nonce as associated data,
zeroization of entropy/seed/keys, 0600 files on Unix, refusal of broad Unix
permissions), the encrypted scan cache, stdin-only secret IPC, sensitive editor
undo clearing, five-minute auto-lock, constant-time admin token comparison and
loopback-only admin binding, bech32m address validation with network separation,
on-chain username resolution through the user's own node, cross-network replay
protection (network in transaction context, signature hash and block header),
the 21,000,000 GNR integer schedule and its tests, coinbase/treasury checks,
nullifier uniqueness and reorg replay.

## Documentation inconsistencies found and corrected

- `SECURITY.md`, `DEVELOPMENT_STATUS.md`, the threat model, the October review
  and the wallet GUI document said mainnet was disabled; the code runs mainnet by
  default (`Network::ensure_runnable` accepts every network, the wallet's network
  is `Mainnet`, `/v1/info` reports `mainnet_finalized: true`) and the README
  records the 10 October 2026 launch.
- `docs/rpc/api.md` said mainnet finalization remained disabled; every genesis
  record reports `finalized: true`.
- The networking document said no public seed was built in; `seeds.rs` contains
  a mainnet seed and `seeds/mainnet.txt` lists two.
- The README's status section cited a pre-mainnet CI baseline without noting
  that 0.4.0–0.5.0 have no recorded exact-commit evidence.

## Items requiring independent review

1. The Tor control-port client (`apps/genoryn/src/net/tor.rs`): parsing,
   SAFECOOKIE flow, key storage and failure handling.
2. The transport policy and connector (`net/policy.rs`, `net/transport.rs`,
   `p2p.rs` dial paths), including the absence of any direct path in tor-only mode.
3. The address-exchange extension and feature announcement (wire compatibility,
   resource bounds, onion validation in `net/addr.rs`).
4. The relay privacy design (`relay.rs`) and its interaction with mempool policy.
5. The P2P hardening changes (slot reservation, diversity, low-work strikes) for
   unintended partition or liveness effects.
6. Consensus, Orchard/Halo 2 integration and wallet key handling, which still
   have never been independently audited.
7. The Arti embedding (`net/embedded.rs`), the authenticated loopback SOCKS
   bridge (`net/bridge.rs`) and the vendored `saturating-time` patch.
8. The local-transaction delivery changes (`relay.rs`, `broadcast_local`,
   mempool announcement and persistence in `node.rs`).
