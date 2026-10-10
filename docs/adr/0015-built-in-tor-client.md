# ADR 0015: Built-in Tor client and reliable delivery of local transactions

Status: accepted for the `privacy/tor-network-hardening` branch; not released.
Extends [ADR 0014](0014-tor-network-privacy.md), which remains valid for an
operator-run Tor daemon.

## Context

ADR 0014 required users to install and run Tor themselves. Wallet users should
not need to: hybrid and tor-only modes must work with nothing else installed,
on Windows and Linux alike. Writing Tor cryptography ourselves is out of the
question, so the client must be an existing, maintained implementation.

## Decision

### Arti, in-process

`genorynd` embeds Arti (`arti-client` 0.47, rustls/ring, static SQLite shared
with the node) when `--tor-socks` is not given. External Tor stays available
with `--tor-socks` (plus `--tor-control` for its onion service).

- State and cache live in `<data-dir>/tor/{state,cache}` (0700 on Unix).
  Arti's own directory-authority list, fallback directories, certificate and
  signature checks are used unmodified; nothing is bypassed or hardcoded.
- Peer streams come from `TorClient::connect` with a fresh isolation group per
  connection (separate circuits), onion names resolved by Tor.
- `--onion-service` launches an Arti v3 onion service (`genoryn-p2p`) whose
  key persists in Arti's keystore under `tor/state`. Accepted streams are handed
  to the P2P listener in-process; there is no loopback port to forward.
- No peer stream is attempted before Arti reports it is ready for traffic.
- Auxiliary HTTPS that must use Tor (published seed list; the wallet's update
  check in tor-only mode) goes through a loopback SOCKS5 bridge into Arti. The
  bridge accepts only username/password authentication with random
  per-process credentials, which the node hands out only on its admin RPC
  (`GET /v1/network/proxy`), and forwards names unresolved (`socks5h`).

### Windows root cause and the vendored patch

On Windows, Arti never got past 15 % bootstrap with one CPU core fully busy,
while the same build bootstrapped on Linux. Bisection traced it to
`saturating-time` 0.5.0 (an Arti workspace crate used by `tor-netdoc` for every
consensus): `saturating_sub` evaluates `unwrap_or(min_value())` eagerly, and
`min_value()` searches for the smallest `SystemTime` by halving a step down to
1 ns, stopping only when a step fails. Windows `SystemTime` counts 100 ns
ticks, so a sub-tick step truncates to zero and always succeeds: the search
never ends. A ten-line reproducer outside GENORYN hangs on Windows and returns
in under a microsecond on Linux. No fixed upstream release exists yet.

`third-party/saturating-time` is the exact crates.io 0.5.0 source with a
termination fix (a step that does not move counts as reaching the limit) and
lazy evaluation (`unwrap_or_else`); see
[the provenance manifest](../../third-party/saturating-time-source.json) and
[the patch](../../third-party/saturating-time-windows-limits.patch). Limits on
Unix are unchanged (upstream's exact-value tests pass). A coarse-clock
regression test fails on the original algorithm. The release packager refuses
any `[patch.crates-io]` entry without byte-verified provenance. The patch
should be dropped once upstream ships a fix.

### Frames are flushed

Arti buffers stream writes until a cell fills. Peer frames are now written in
one piece and flushed; without that, handshakes over Tor stalled until timeout.

### Delivery of transactions created here

The one-hop stem from ADR 0014 had two delivery weaknesses: a stem peer that
dropped the transaction plus any other peer echoing it suppressed every wider
broadcast, and a transaction created during a long outage was abandoned after
about two minutes, staying in the local mempool unsent.

- An echo from a non-stem peer is relayed onward once, exactly as any node
  relays a transaction it did not create; otherwise it is broadcast once when
  the embargo ends. Neither depends on the stem peer.
- The search for a stem peer has no deadline; in tor-only mode only Tor links
  qualify, so waiting never means clearnet.
- A newly connected peer receives the mempool (bounded by mempool limits),
  except own transactions still in their stem phase, so delivery resumes as soon
  as any acceptable peer connects and own transactions look like relayed ones.
- Own pending transactions are saved as already-shielded bytes in
  `local-transactions.json` and re-validated after a restart (expiry,
  nullifiers, anchor); invalid ones are dropped. The same bytes are resent, so
  no new spend is created. They are forgotten when they leave the mempool.

No message, consensus rule or wallet format changes; v0.4/v0.5 peers receive
only ordinary `Transaction` messages.

## Consequences

- About 200 additional crates; Arti's MIT/Apache-2.0 crate archives carry no
  license files, so the packager needs license supplements before a release
  (it fails closed until then).
- First start downloads Tor directory information (about 10 MB); later starts
  use the cache. Measured times are in the testing evidence, not here.
- Arti is younger than C Tor. Its onion-service support is newer than its
  client; operators who prefer C Tor can keep using it.
- A dedicated, independent review of the embedding, the bridge and the patch is
  still required.
