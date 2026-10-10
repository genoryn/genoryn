# ADR 0014: Tor network privacy through an operator-run Tor daemon

[ADR 0015](0015-built-in-tor-client.md) makes a built-in Tor client the default
and changes how local transactions are delivered; this record still describes
the operator-run daemon option and the shared transport policy.

## Context

Nodes connected only over direct TCP, resolved seeds with the system resolver,
kept a persistent handshake identity (`peer-id`) and broadcast new transactions
to every peer at once. Peers and network observers could therefore learn a
node's IP address, link it across restarts and often identify the origin of its
transactions. Mainnet is live, so peer protocol v1 compatibility is mandatory.

## Decision

1. **External Tor, maintained client libraries.** GENORYN uses a Tor daemon (or
   Tor Browser) that the operator runs: SOCKS5 through the maintained
   `tokio-socks` crate (also used by `reqwest` for `socks5h`), and a minimal,
   bounded control-port client for `PROTOCOLINFO`, `AUTHCHALLENGE`/
   `AUTHENTICATE` (SAFECOOKIE via `hmac`/`sha2`, or a password from a file),
   `GETINFO status/bootstrap-phase` and `ADD_ONION`. No Tor cryptography is
   implemented here. Tor is never downloaded, installed or launched.
   Embedding Arti was not chosen: its onion-service support was not yet
   recommended for security-sensitive use and it would add a very large
   dependency tree; it can be revisited.
2. **Three explicit modes** (`direct` default, `hybrid`, `tor-only`), with a
   single `TransportPolicy::route` decision and a single `Connector` that opens
   every outbound P2P connection. Tor-only cannot produce a direct route and
   never falls back; readiness gates dialing.
3. **Onion addresses as first-class peers** (`PeerAddr`), validated (v3 version,
   SHA3-256 checksum, ed25519 key of large order) and never resolved through the
   system resolver. Persisted caches stay text arrays, so v1 `peers.json` files
   still load; each mode has its own cache file.
4. **Backward-compatible protocol extension.** Features are announced in an
   unsolicited `Pong` (ignored by v1 nodes); `GetAddrV2`/`AddrV2`/`Announce` are
   appended variants sent only after a peer announces support. Handshakes on Tor
   links advertise no listening address and use a separate per-run identity.
5. **Persistent onion identity** stored as Tor's `ED25519-V3` key blob in the data
   directory, with a non-detached service so it disappears with the node.
6. **One-hop stem relay** for locally created transactions plus per-peer
   exponential relay delays (see [network privacy](../network-privacy.md)).
7. **P2P hardening that does not touch consensus:** outbound slots reserved from
   inbound peers (default 24 total = 16 inbound + 8 outbound), at most two
   outbound peers per network group, a 64-entry onion share of the 128-entry
   cache, a per-minute bound on onion-service arrivals (they cannot be banned by
   address), strikes for side blocks whose branch trails the tip by more than
   720 blocks of current work, and transient accept errors no longer stop the
   P2P loop.

## Consequences

- Operators must run and secure Tor themselves; status is visible through the
  admin RPC and the wallet. A failed control port degrades to SOCKS-only
  readiness and is reported.
- Onion discovery spreads only among upgraded hybrid/tor-only nodes and from
  configured onion peers until more nodes upgrade.
- Inbound capacity with the old explicit `connections = 16, outbound = 8` is now
  8 instead of 16; the new default keeps 16 inbound by raising the total to 24.
- Headers-first synchronization and a minimum-chain-work rule were evaluated and
  not implemented: they need new messages and a versioned rollout. The low-work
  strike only throttles senders of cheap historical side blocks; such blocks are
  still validated and stored, and fork choice is unchanged.
- No claim of anonymity is made. Application-level checks do not replace OS
  firewall isolation and cannot defeat traffic correlation or a compromised host.
