# ADR 0008: bounded randomized outbound peers and conservative rotation

The initial cache was a sorted set whose first eight addresses were always
dialed. This deterministic prefix could exclude eligible alternatives. Preserve
the stable cache for recovery, but shuffle a snapshot with the maintained OS RNG
before each dial pass. Cap all pending/active outbound attempts at eight and all
connections at sixteen; shuffling must not expand the resource limits.

Each outbound connection gets an independent 15–30 minute rotation deadline.
Only rotate when another validated peer remains. Do not intentionally rotate the
sole connection, and do not use this policy to terminate inbound connections.
Serialize rotation reservations with a shared one-minute cooldown so simultaneous
deadlines cannot deliberately rotate every connection from the same peer-count
snapshot. External disconnections can still remove remaining peers at any time.
The existing three-second bounded dial loop discovers a replacement after a
rotation; this may be the same peer in a very small local network. Rotation is
ordinary disconnection and never counts as misbehavior or a ban.

This is application transport policy, not a consensus change. No new cryptographic
primitive, source-anonymity guarantee, Dandelion++ implementation, or anti-eclipse
proof is claimed. Source IP and timing remain observable over TCP. A malicious
majority or attacker-controlled discovery set remains a production review concern.

Tests cover selection beyond the sorted prefix without inventing or losing
candidates, unchanged persisted ordering, bounded deadlines, and retention of
the sole peer. Real local peer reconnection/rotation evidence is separate from
these pure policy tests. Tor support was added later in
[ADR 0014](0014-tor-network-privacy.md), which also raised the default total to
24 connections (16 inbound, 8 reserved outbound); I2P remains future work.

An independent local-process review then reproduced another sorted-prefix bug:
128 stale cached addresses excluded a higher-sorting explicit live peer before
the shuffle. Normalize startup admission to at most 128 records, retain every
explicit operator peer, and fill remaining slots from a shuffled cache snapshot.
Dial explicit peers first (randomized within that group), then other candidates.
Hello and peer-exchange admission share the same bound. Explicit discovery
preferences never grant consensus authority.

Unreachable/duplicate dial attempts use bounded exponential delays from 3 to
300 seconds. A stable connection or orderly rotation resets the delay. Retry
state is bounded by the same admitted endpoint set. Peer advertisements sample
eligible entries without a permanent sorted prefix. Normalize IPv4-mapped IPv6
addresses before applying discovery restrictions so private/loopback/multicast
IPv4 endpoints cannot acquire a different policy through address representation.
