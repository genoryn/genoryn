# Network privacy and Tor

GENORYN has two independent privacy layers. Read this before choosing a mode.

| Layer | Protects | Mechanism |
|---|---|---|
| On-chain privacy | Amounts, senders and receivers of transfers | Orchard/Halo 2 shielded notes, in every network mode |
| Network privacy | This computer's IP address, as seen by peers | Optional Tor routing (hybrid or tor-only mode) |

Neither layer makes anyone completely anonymous, and the wallet never shows a
"100% anonymous" indicator.

## Network modes

The mode is chosen with `--network-mode`, `[network_privacy] mode` in the node
TOML, or the wallet's Network page. A **new wallet installation starts in
tor-only mode with the built-in Tor**. An existing installation (earlier
settings, a wallet or a chain in the data directory) keeps its mode and is
never switched silently. `genorynd` started on its own without a mode stays
`direct`, so upgraded node operators are not switched either. Tor modes need no
further setup because Tor is built in.

Tor-only nodes need bootstrap peers that Tor can reach. On 10 October 2026 both
mainnet seeds answered direct connections but not connections from Tor exits
(the built-in client and, independently, C Tor: no attempt succeeded). Until
seed operators publish onion addresses in `seeds/mainnet-onion.txt` of the
main repository (read only by hybrid and tor-only nodes) or accept Tor exits, a new tor-only node finds no mainnet
peer by itself; the node and wallet then show a warning. An onion `--peer`
works at any time.

| | Direct | Hybrid | Tor only |
|---|---|---|---|
| Outbound clearnet peers | Direct TCP | Direct TCP | Through Tor exits |
| Outbound onion peers | Refused | Through Tor | Through Tor |
| Inbound peers | Clearnet listener | Clearnet listener and optional onion service | Onion service only (a clearnet listener is allowed only on loopback) |
| Peer discovery DNS | System resolver | System resolver (never for `.onion`) | None: host names are sent to Tor (SOCKS5 remote resolution) |
| Published seed list (HTTPS) | Direct | Direct | Through Tor only; skipped if Tor is not ready |
| Payments created here | One relay peer first (see below) | Prefers Tor-routed peers | Tor-routed peers only |
| If Tor fails | n/a | Onion peers paused | All peer networking paused; nothing falls back to direct |
| Peer cache file | `peers.json` | `peers-hybrid.json` | `peers-tor.json` |

**Direct** and **hybrid** reveal this computer's public IP address to every
clearnet peer. The wallet shows a privacy warning for both.

## Using Tor

Tor is built in: hybrid and tor-only modes use a Tor client inside `genorynd`
([Arti](https://gitlab.torproject.org/tpo/core/arti), see
[ADR 0015](adr/0015-built-in-tor-client.md)). Nothing has to be installed or
kept running. Its directory information and keys live in `<data-dir>/tor`;
the first start downloads about 10 MB of Tor directory data.

```sh
# Everything through Tor, no inbound peers:
genorynd --network-mode tor-only

# Also accept inbound peers through a persistent v3 onion service:
genorynd --network-mode tor-only --onion-service

# Clearnet plus onion peers:
genorynd --network-mode hybrid

# Onion bootstrap peers:
genorynd --network-mode tor-only --peer <56-character-name>.onion:29610
```

An existing Tor daemon or Tor Browser can be used instead with `--tor-socks`
(GENORYN never installs or starts it):

- Tor daemon: SOCKS `127.0.0.1:9050`, control port (if enabled) `127.0.0.1:9051`.
- Tor Browser: SOCKS `127.0.0.1:9150`, control port `127.0.0.1:9151`.

```sh
genorynd --network-mode tor-only --tor-socks 127.0.0.1:9050 \
  --tor-control 127.0.0.1:9051 --onion-service
```

Proxy and control addresses must be IP literals, so reaching Tor itself never
needs DNS. A SOCKS proxy that is not on loopback triggers a warning, because the
traffic to it crosses the local network unencrypted. The control-port settings
below apply only to an external Tor.

### Control-port authentication

The control port is optional; it provides bootstrap progress and the onion
service. GENORYN authenticates with, in order:

1. a password read from `--tor-control-password-file` / `control_password_file`
   (Tor `HashedControlPassword`); passwords are never accepted as arguments;
2. `SAFECOOKIE`, with the cookie path reported by Tor or `--tor-cookie-file`.
   Tor must prove it knows the cookie (`SERVERHASH`) before GENORYN sends anything
   derived from it, so a process impersonating the control port learns nothing;
3. an unauthenticated (`NULL`) control port, reported as such.

Plain `COOKIE` authentication, which sends the raw cookie, is never used.
Credentials, cookies and onion private keys never appear in logs or RPC output.

### Onion service

With the built-in client, `--onion-service` launches a v3 onion service inside
the node. Its key is created on first use and kept in Arti's keystore under
`<data-dir>/tor/state` (0700 on Unix; on Windows the user-profile ACL of the
data directory), so the address survives restarts, including after a crash.
Publishing a new service's descriptor can take several minutes. Delete
`<data-dir>/tor/state/keystore` to obtain a new address.

With an external Tor, GENORYN asks it (`ADD_ONION`) for a v3 onion service
forwarding the network port to a loopback listener (default base port + 3, for
example `127.0.0.1:29613` on mainnet). The service key is generated by Tor on
first use and stored in `onion_v3_private_key` in the node data directory
(mode 0600 on Unix; on Windows it inherits the user-profile ACL of the data
directory). The same address is restored after restarts and after Tor restarts.
The service is created without `Detach`, so Tor removes it as soon as the node's
control connection ends, including after a crash.

Delete `onion_v3_private_key` to obtain a new onion address. Back it up only if
you need the same address after reinstalling.

### Readiness and failure

The built-in client reports its own bootstrap progress; no peer connection is
attempted before it is ready for traffic, and it re-bootstraps after network
loss. For an external Tor, a monitor probes the SOCKS port (a SOCKS5 method negotiation, which creates no
circuit) and, if configured, polls `GETINFO status/bootstrap-phase`. Outbound
Tor connections are attempted only when the proxy answers and, with a working
control port, bootstrap reports 100 %. While Tor is not ready nothing is dialed,
and addresses keep their standing so a Tor outage does not erase the peer cache.
A failed SOCKS connection at the proxy pauses dialing at once. When the control
connection is lost (for example a Tor restart), GENORYN reconnects and
re-registers the onion service with the stored key. `genoryn-cli
reconnect-network --token-file …` or the wallet's **Retry Tor now** button
re-checks Tor and retries peers immediately.

Status is available on the loopback administrative RPC only:
`genoryn-cli get-network-privacy` (`GET /v1/network`). Public RPC never reveals
the mode, Tor status or onion address.

## Leak protections (application level)

- **One dialing path.** Every outbound P2P connection goes through
  `net::Connector`, which consults `TransportPolicy::route`. In tor-only mode the
  policy cannot produce a direct route, and the connector refuses one anyway.
- **No fallback.** A SOCKS refusal, proxy failure or bootstrap delay never leads
  to a direct attempt.
- **No local DNS for onion names, ever.** Parsing never resolves names; an
  invalid `.onion` name is an error, never a DNS query. The RPC clients
  (`genoryn-connect`, the wallet CLI) refuse `.onion` endpoints because they
  have no Tor transport.
- **Remote name resolution.** In tor-only mode bootstrap host names are sent to
  Tor as SOCKS5 domain names (ATYP 3). Auxiliary HTTPS uses `socks5h`.
- **Validated onion addresses.** v3 addresses are checked for version, SHA3-256
  checksum and a valid ed25519 key before they enter the peer cache.
- **Unlinkable identities.** The handshake identity is random per run (it was
  previously a persistent `peer-id` file), and Tor links use a second identity.
  Tor links advertise no listening address. The node's onion address is
  announced only over Tor links, never over clearnet connections.
- **Stream isolation.** Each Tor peer connection gets its own circuit: a fresh
  isolation group in the built-in client, random SOCKS credentials
  (IsolateSOCKSAuth) with an external Tor.
- **Built-in Tor bridge.** Auxiliary HTTPS reaches the built-in client through
  a loopback SOCKS5 bridge that requires per-process random credentials, handed
  out only on the admin RPC (`GET /v1/network/proxy`), and forwards names
  unresolved.
- **Separate caches.** Each mode keeps its own peer cache, so peers learned
  while connecting directly do not shape (or fingerprint) Tor connections.
- **Log redaction.** In Tor modes, and for every onion address, logs show a
  per-run salted tag such as `ipv4#1a2b3c4d` instead of the address. Proxy and
  control addresses are not logged.

### Auxiliary traffic

| Traffic | Direct / hybrid | Tor only |
|---|---|---|
| Published seed list (`raw.githubusercontent.com`) | Direct HTTPS | Through Tor (`socks5h`); skipped if Tor is not ready |
| Wallet update check (`api.github.com`, opt-out in Settings) | Direct HTTPS | Through Tor (`socks5h`; with the built-in client via the local node's bridge); fails rather than connecting directly |
| Wallet RPC | Configured endpoint | Only a loopback node; remote endpoints are refused before any request |
| Links opened from the wallet | The browser's own network settings | The browser's own network settings |

### What these protections do not cover

Application-level checks do not replace operating-system isolation. For strong
guarantees, run GENORYN in an environment that forces all traffic through Tor
(for example Whonix or Tails) or apply firewall rules that block direct
traffic from the node. These protections also cannot help against:

- a compromised computer, malware or a malicious Tor binary;
- an adversary who can observe both ends of a Tor circuit (traffic correlation);
- malicious Tor exits, which can read, delay, drop or alter the unencrypted
  peer traffic of clearnet peers reached in tor-only mode (altered blocks still
  fail validation, but an exit can censor or isolate a connection; onion peers
  avoid exits);
- traffic outside GENORYN, such as browsing or other applications;
- the timing and size of your node's traffic and its online periods.

## Transaction broadcast privacy

Earlier releases broadcast a new transaction to every peer after one shared
random delay, so the first peers to see it could often identify its origin.

Now:

- Relays go to each outbound peer after an independent exponential delay (mean
  2 s); all inbound peers share one delay per transaction (mean 5 s), so opening
  many inbound connections does not give an observer an earlier copy.
- A transaction created by this node is first sent to **one** relay peer
  (preferring outbound Tor links). If it comes back through any other peer it
  is relayed onward once, as any node relays a transaction it did not create;
  otherwise it is broadcast once when a random 20–45 s embargo ends, within
  the mode's scope (Tor links only in tor-only mode; Tor links first in hybrid
  mode). A dropping relay peer therefore cannot suppress it, even with an
  accomplice echoing it back.
- Delivery does not give up: without an acceptable relay peer (for example
  during a Tor outage) the node waits, never falling back to clearnet in
  tor-only mode. Peers that connect later receive the mempool, except own
  transactions still waiting for their relay peer. Own pending transactions are
  kept in `local-transactions.json` (already-shielded bytes only) and
  re-validated after a restart; they are forgotten once mined, expired,
  conflicting or evicted. The same bytes are resent, never a new spend.
- Transactions are no longer echoed back to the peer they came from.

This is a one-hop "stem" in the spirit of Dandelion++. Full Dandelion++ (multi-hop
stems, epoch-based relay graphs, a separate stem pool) was evaluated and not
implemented now: it requires a new message type that only upgraded peers
understand, is ineffective while few peers support it, and its stem pool adds
denial-of-service surface. The one-hop stem needs no protocol change, so
unupgraded peers simply relay the transaction. Validation, nullifier conflict
checks and mempool limits are unchanged.

It does **not** provide anonymity: the chosen relay peer learns that this node
sent the transaction first, and timing analysis by a well-connected observer
remains possible.

## Light wallets

A wallet can run without a node of its own (wallet CLI `"light": true` with
`"sources"`, or **Light wallet** on the wallet's Network page). It reaches the
listed nodes' public RPC only through Tor (its own built-in client in
`<wallet>.tor`, or `tor_socks`), each source on its own circuit; onion sources
may use plain HTTP, clearnet sources need HTTPS. It downloads the chain from the
source with the highest tip and **fully validates it locally** (RandomX work,
Halo 2 proofs, state roots) before scanning, rejects a source whose chain fails
and tries the next, and reports whether the other sources agree, are behind or
are on a different branch. Payments go out through `POST /v1/relay` of a source
other than the one it synced from. Node operators offer this with `--public-rpc
--onion-service`.

The remote nodes never see addresses, viewing keys or balances; they see when a
wallet syncs and which transaction it hands over, and they can withhold new
blocks, which is why two or more independent sources are recommended. Usernames,
names and mining still need a node of one's own. Each operation currently
downloads and validates the whole chain; a local validated chain and compact
blocks are planned (see the [proposal](wallet/light-wallet-proposal.md)).

## Peer protocol compatibility

Peer protocol v1 is unchanged on the wire. Upgraded nodes announce features in
an unsolicited `Pong` nonce (high half `GNRF`), which v1 nodes ignore. Only after
a peer announces support are the extension messages `GetAddrV2`, `AddrV2` and
`Announce` sent; they are appended after the v1 variants, so their encoding
never changes v1 messages. A v1 peer never receives a message it cannot decode;
this is tested against a peer that implements only the original schema.

Onion addresses spread only between upgraded hybrid and tor-only nodes (a direct
node does not store them) and from configured onion bootstrap peers.

## Testing

`apps/genoryn/tests/network_privacy.rs` and `relay_reliability.rs` run real
nodes and sockets against controlled local stand-ins for Tor and adversarial
peers; nothing contacts an external network. `scripts/test/tor_live.py` is an
opt-in run over the real Tor network: two tor-only devnet nodes with onion
services, block sync, shielded transactions both ways, restart persistence and
OS-level sampling of every socket the nodes open (on Linux also a logging
resolver and a simulated Tor outage). See
[testing evidence](testing/network-privacy-evidence.md). Operators should still
verify their own setup (for example with `genoryn-cli get-network-privacy`).
