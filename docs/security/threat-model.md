# Threat model

This document states what GENORYN protects, against whom, how, and what it does
not protect. It describes release 0.6.0. GENORYN has **not** been independently
audited; everything below is an engineering claim backed by tests, not a proof.

## What GENORYN protects

GENORYN has two independent privacy layers and one integrity layer.

| Layer | Protects | Mechanism | Always on? |
|---|---|---|---|
| On-chain privacy | Amounts, senders and receivers of transfers | Orchard shielded notes with Halo 2 proofs; there is no transparent mode | Yes |
| Network privacy | This computer's IP address, as seen by peers | Tor: the node's built-in client (Arti) or an operator-run Tor daemon | New wallet installations start in tor-only; earlier installations keep their mode (`direct` sees no Tor) |
| Integrity | The chain, the 21,000,000 GNR cap, balances | Every node and wallet validates every block: RandomX work, Halo 2 proofs, signatures, anchors, nullifier uniqueness, exact issuance | Yes |

Public deterministic issuance and transfer fees stay visible by design, so anyone
can audit the supply. A wallet finds its own notes by scanning every ciphertext
locally; no server is ever asked for a balance.

## Trust assumptions

- The cryptography of Orchard/Halo 2, RandomX, ed25519 and the Tor protocol holds.
- The dependencies do what they claim, including the built-in Tor client and the
  small, recorded termination fix vendored into one of its dependencies
  ([ADR 0015](../adr/0015-built-in-tor-client.md)).
- The user's computer is not compromised.
- The consensus composition, the Tor integration and wallet key handling are
  correct. None of these has had an independent review.

## Adversaries

| Adversary | Can | Mitigations | Residual risk |
|---|---|---|---|
| Chain observer | Read every block | Shielded notes; no transparent mode | Fees, sizes, timing, a small anonymity set |
| Malicious peer | Send invalid or excessive data, lie about addresses | Full validation of every block and proof, bounded frames and rates, strikes and bans, netgroup and onion quotas | Not production-grade Sybil resistance |
| Peer that suppresses payments | Drop a payment it was handed, or echo it back to fake spreading | An echo from another peer is relayed onward; otherwise one wider broadcast after a short embargo; every new peer receives pending payments | A node surrounded only by attackers stays isolated |
| Eclipse attacker | Fill inbound slots and the address cache | Outbound slots reserved from inbound, outbound netgroup diversity, explicit peers kept and tried first, separate caches per mode | A discovery majority can still isolate a new node |
| Clearnet peer (direct, hybrid) | See this node's IP and the first copy of its payments | One-hop stem to a single relay peer, per-peer relay delays, Tor links preferred for own payments in hybrid | The stem peer learns the origin; timing analysis |
| Local network or ISP | See traffic to peers or to Tor | Tor-only mode; no DNS for peers in tor-only; never DNS for onion names | That Tor is used is visible; traffic volume and timing |
| Malicious Tor exit (tor-only to clearnet peers) | Read, delay, drop or alter unencrypted peer traffic | Full validation; a circuit per peer; onion peers need no exit | Censorship or isolation of exit-routed peers |
| Remote node serving a light wallet | See when a light wallet syncs and which payment it hands over; withhold blocks | Tor only; a circuit per source; the whole chain is validated on the device; several sources compared; payments sent through a different source | Withholding by every configured source; sync timing |
| Global passive adversary | Correlate both ends of Tor circuits | None at the application level | Deanonymization by correlation |
| Process impersonating an external Tor's control port | Ask for authentication | SAFECOOKIE requires Tor to prove knowledge of the cookie first; raw COOKIE authentication is never used | A password sent to a fake HASHEDPASSWORD port |
| Compromised computer | Everything | None | Total loss of privacy and funds |

## Threats and mitigations

### Money and consensus

- **Inflation**: exact integer schedules, no mint transaction class, a fixed
  coinbase value balance and treasury receiver checks. The mainnet treasury share
  goes to a provably unspendable address.
- **Double spends**: nullifiers are unique across the chain and the mempool;
  reorganizations replay state per branch and revalidate pending transactions.
- **Locked treasury spending** (devnet, testnet): locked commitments are excluded
  from every admitted spend anchor; knowing the public test key does not bypass
  maturity.
- **Burned value**: name prices go to an address whose spend key is a
  hash-to-curve point with no known discrete logarithm; breaking that would also
  break Orchard spend authorization.
- **Mempool flooding**: bounded count and bytes; a full pool admits a transaction
  only by displacing strictly lower fee rates, and only after its proofs verify.

### Peers and the network

- **Malicious peers**: length caps, timeouts, version and genesis checks, proof
  and work validation, bounded connections, misbehaviour disconnection. Side
  branches must pass header and RandomX checks before any replay; request rates,
  inbound per-IP limits and per-netgroup discovery limits bound flooding and
  cache poisoning. Senders of far-behind side branches earn strikes.
- **Payments that never arrive**: a payment created while no acceptable peer is
  reachable waits without a deadline (in tor-only mode only Tor links qualify),
  is kept on disk and re-validated after a restart, and is handed to every peer
  that connects later. The same bytes are resent; no new spend is created.
- **Peer protocol compatibility**: the v1 wire format is unchanged; extensions
  are sent only to peers that announce support. Tested against an unupgraded
  peer and a mixed 0.4.0 / 0.5.0 / 0.6.0 network.

### Network privacy (Tor)

- **No fallback**: one connector opens every outbound peer connection after one
  policy decision. Tor-only mode has no direct route; learned addresses,
  including hostile ones, are dialed only according to that policy. Nothing is
  dialed before Tor is ready. Tested with a hostile peer, a canary listener and
  OS-level socket sampling on the live Tor network.
- **DNS leaks**: names are never resolved while parsing; onion names never reach
  the system resolver in any mode; in tor-only mode bootstrap names and auxiliary
  HTTPS are sent to Tor by name (`socks5h`). Local RPC clients without Tor refuse
  onion endpoints; the light wallet reaches remote nodes only through Tor.
  Verified on Linux with a logging resolver that saw no node query.
- **Linking identities**: a new handshake identity per run and another one on Tor
  links, no listening address advertised on Tor links, the onion address
  announced only over Tor links, separate peer caches per mode, a circuit per peer.
- **Secrets**: the built-in client keeps its onion key in its own keystore under
  `<data-dir>/tor/state` (owner-only on Unix; the user-profile ACL on Windows); an
  external Tor's control password is read only from a file. The auxiliary Tor
  bridge accepts only random per-process credentials handed out on the admin RPC.
  Logs show salted labels instead of peer addresses; tested by capturing every
  log line of a full Tor-mode scenario.
- **Bootstrap through Tor**: tor-only nodes need peers Tor can reach. Onion seeds
  are published separately (`seeds/mainnet-onion.txt`) so nodes before 0.6 never
  see onion names. If no seed is reachable, the node and wallet say so instead of
  connecting directly.

### Wallet and keys

- **Wallet theft**: BIP39 entropy from the operating system, encrypted files
  (Argon2id, XChaCha20-Poly1305), owner-only permissions on Unix, zeroization of
  secrets where supported, automatic lock after five minutes.
- **Username impersonation**: on-chain names are first-come in chain order,
  signed by a seed-derived owner key, permanent and resolved by the user's own
  node. Lookalike names (`maxl` for `max1`) remain a social risk; check the full
  name. Squatting costs a burned price but is not prevented. A reorganization
  deeper than a registration's confirmations can change which of two competing
  registrations won. The legacy local registry is first-come per node and not
  globally unique.

### Node operation and releases

- **RPC exposure**: administrative actions bind only to loopback and need a
  private capability token. Public RPC is explicit opt-in; it serves chain data
  and accepts transactions for relay (`POST /v1/relay`, validated like a peer
  relay), never reveals the network mode, Tor status or onion address, and, with
  the built-in onion service, is also reachable as an onion service (at most 16
  streams at once).
- **Database corruption or crash**: SQLite WAL with full sync, integrity checks,
  atomic tip commits, canonical block replay and schema, network and genesis checks.
- **Release compromise**: the wallet never installs anything by itself; packages
  carry SHA-256 checksums and a manifest with their source commit; every
  dependency's license text and vendored source is pinned by hash. Releases are
  not yet signed with a production signing key.

## What privacy does not protect

Compromised computers, malware, copied seeds, weak passwords, screenshots,
clipboard contents, voluntary disclosures, transaction timing and fee patterns,
small anonymity sets, or the peer that first relays a broadcast can reveal
information. Relay delays and the one-hop stem are not a proof of source
anonymity. Tor hides the IP address from peers but not from an adversary who can
correlate both ends of a circuit, and application checks do not replace
operating-system isolation (for example Whonix or Tails). Auxiliary traffic is
listed in [network privacy](../network-privacy.md#auxiliary-traffic); links opened
from the wallet use the browser's own network settings. A username publishes its
receiving address; payments to it stay shielded. A malicious majority, an eclipse
attack or a chain partition can reverse low-confirmation payments.

## Known limitations

- **Sync**: block-by-block sync has no headers-first download, checkpoints or
  minimum chain work. Early heights were mined at very low difficulty, so stored
  low-work side blocks there are cheap; each still needs valid work and a valid
  coinbase proof, senders earn strikes, but disk use is bounded only by attacker
  effort and per-connection throttling, and a side block forces a replay up to
  its fork point. Fixing this needs new messages and a versioned rollout.
- **Nullifier root**: it hashes the whole nullifier set for every block; an
  incremental commitment should replace it in a future protocol version.
- **Light wallet**: each operation currently downloads and validates the whole
  chain; remote sources learn sync timing.
- **Tor reachability on mainnet**: on 10 October 2026 the mainnet seeds accepted
  direct connections but not connections from Tor exits, so new tor-only nodes
  depend on onion seeds being published.
- GENORYN does not claim absolute anonymity, deterministic proof-of-work finality,
  ASIC-proof mining, production anti-Sybil protection or an inherited audit. Ten
  confirmations are a default display policy, not an economic guarantee.

## Mainnet status

The mainnet launched on 10 October 2026 (release 0.4.0, genesis
`c5fc87d9…d708`) without the gates this document once required: an independent
review of the cryptography, consensus and network code, authenticated release
signing and a seed-discovery review are still outstanding. The mainnet treasury
allocation is paid to a provably unspendable address, so no treasury key exists.
No administrator has a financial override or a global viewing key.
