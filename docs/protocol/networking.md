# Peer protocol v1

TCP frames start with four-byte network magic, a little-endian u32 length and a
canonical typed message. Frames over 2,100,000 bytes are rejected before allocation.
Handshake validates version, network, genesis and peer identity. The reader owns
partial frames continuously; interval cancellation cannot discard consumed bytes.
Read/write timeouts, bounded event channels and connection limits constrain work.

Messages support block locators, one-block responses, block announcements,
shielded-transaction relay, peer exchange and ping/pong. Fork sync tracks the
remote branch progress even before its work exceeds the local tip. Discovery uses
explicit peers, persistent cache, exchange and operator-supplied DNS seeds. No
company domain or seed is a consensus authority.

## Network privacy modes

Every outbound connection is opened by one connector after one policy decision;
see [network privacy and Tor](../network-privacy.md) for the full behavior of the
`direct` (default), `hybrid` and `tor-only` modes, Tor setup, onion services and
the remaining limits. In short: tor-only nodes reach every peer through Tor,
resolve no peer names locally, accept inbound peers only through an onion
service and never fall back to direct connections.

## Listening and discovery

`genorynd` listens on all interfaces (`0.0.0.0`) at the network's base port unless
`--p2p-listen` or `[p2p] listen` says otherwise (tor-only nodes listen on
clearnet only when a loopback address is configured). A node bound to loopback
is a local lab: its discovered peers remain loopback-only and it never contacts
the built-in seeds. Any other node is public: discovery rejects
private, loopback, link-local, multicast and invalid endpoints, and the built-in
bootstrap hosts in `apps/genoryn/src/seeds.rs`, plus the list published at
`seeds/mainnet.txt` (or `seeds/devnet.txt`, `seeds/testnet.txt`) on the main repository's `main` branch,
are used at startup like operator DNS seeds (`--no-default-seeds` opts out of
both). The published list is fetched over HTTPS (through Tor in tor-only mode)
with a 5-second timeout, a 16 KiB bound and at most 16 names; when it is
unreachable, other discovery continues. Whoever controls a seed list can steer a
new node's first contacts, which is why cached and exchanged peers, explicit
peers and full validation remain independent. Explicit peers are an operator
choice and may be `IP:PORT`, `[IPV6]:PORT`, `<v3>.onion:PORT` or `HOST:PORT`.
A peer listening on a wildcard address is recorded at the address its direct
connection was observed from; connections through Tor never contribute an
observed address. Nodes behind NAT or a VPN work through outbound connections
alone; at least one reachable node is needed for others to meet.

The handshake identity is random for every run, and Tor links use a separate
identity and advertise no listening address (`0.0.0.0:0`, which peers never
record), so a node cannot be linked across restarts or transports by it.

## Feature announcement and address exchange v2

Upgraded nodes send `Pong(0x474E5246_xxxxxxxx)` right after the handshake; v1
nodes ignore unsolicited pongs. The low 32 bits are feature flags; bit 0 means
address exchange v2. After a peer announces it, a node may send:

- `GetAddrV2`: request up to 32 addresses (shares the `GetPeers` rate limit);
- `AddrV2(Vec<WireAddr>)`: IPv4, IPv6 and v3 onion addresses (32-byte key and
  port), admitted only as the reply to the node's own request;
- `Announce(WireAddr)`: the sender's own onion address, sent only over Tor links
  and admitted at most once per connection.

These variants follow `Pong` in the message enum, so v1 encodings are unchanged.
A node never sends them to a peer that has not announced support, and an
extension received before an announcement earns a strike. Onion addresses are
validated before admission and may fill at most 64 of the 128 cache entries.

## Misbehavior, limits and diversity

Repeated invalid data earns strikes and disconnection, with temporary bans for
severe inbound frame/misbehavior failures on clearnet connections. This is not
production-grade Sybil immunity.

Strikes distinguish misbehavior from ordinary races. Policy refusals that honest
relays reach through timing or capacity (full mempool, duplicate or conflicting
transaction, anchor or expiry not yet valid here, block timestamp beyond the
local clock bound) cost nothing; a block whose parent is not stored yet costs one
strike and triggers a sync request; consensus-invalid blocks cost 20 and invalid
transactions 10, with disconnection at 100. A valid side block whose branch
trails the tip by more than 720 blocks of current-difficulty work costs 2: such
blocks are still validated and stored and fork choice is unchanged, but a peer
streaming cheap historical branches is disconnected. Per connection, `GetBlocks`
is limited to 300 and `Ping` to 20 per ten seconds (excess requests earn a strike
and are not answered), `GetPeers`/`GetAddrV2` are answered at most three times per
ten minutes, and only the single reply to our own request is admitted. Locators
are resolved through the stored block index, not by hashing the active chain.

The default limits are 24 connections, 16 inbound and 8 outbound. Inbound peers
may use only `connections - outbound_connections` slots, so an attacker who fills
every inbound slot cannot crowd out the outbound peers this node chooses. A
remote IP may hold at most four inbound connections (loopback labs are exempt).
Onion-service connections all arrive from Tor on loopback, cannot be told apart
or banned by address, and are therefore limited to 30 new arrivals per minute
within the inbound budget. The discovery cache admits at most four addresses per
IPv4 /16 or IPv6 /32, so a single operator cannot fill it; at most two outbound
connections share a network group (explicit peers are exempt); and a discovered
address that fails five dials in a row is forgotten, but failures caused by Tor
being unavailable do not count. Explicit peers are never forgotten. The ban table
holds at most 4,096 unexpired entries. Transient accept errors are logged and no
longer stop the peer loop. Public RPC has its own concurrency budget, so public
load cannot starve the loopback administrative RPC.

Outbound selection shuffles the cached candidate set with the maintained OS RNG.
Outbound connections rotate independently after 15–30 minutes only while another
validated peer remains. A small network may reconnect to the same peer; rotation
does not prove unlinkability or protect against a malicious discovery majority.
Inbound connections and the sole remaining peer are not deliberately rotated.
The discovery cache has at most 128 entries. Explicit operator peers are retained
when a stale cache is full and are tried before discovered alternatives; both
groups are shuffled. Failed/duplicate dials back off from 3 seconds to at most
5 minutes. IPv4-mapped IPv6 addresses use the same discovery restrictions as
their IPv4 form. These limits are transport policy, not proof that peers are honest.

## Synchronization limits

Sync downloads full blocks one at a time; every block is fully validated before
it is stored. There is no headers-first download or minimum-chain-work rule yet.
Both need new messages and a versioned rollout and are recorded as remaining
work in the [threat model](../security/threat-model.md).

## Transaction relay

Relayed transactions are sent to each outbound peer after an independent
exponential delay and to all inbound peers after one shared delay per
transaction; they are not echoed to the peer that sent them. Transactions
created by this node first go to a single relay peer and are broadcast more
widely only if they are not seen spreading within a random embargo. See
[network privacy](../network-privacy.md#transaction-broadcast-privacy) for the
design and its limits. This is not a proof of source anonymity.

The `genoryn-connect` RPC SDK is separate from native P2P: browsers require an
opted-in HTTPS-compatible RPC endpoint and cannot dial arbitrary TCP peers.
