# Privacy boundaries

All ordinary monetary transfers use shielded notes. There is no transparent send
mode. Fees and protocol issuance are public. Each output uses the maintained
protocol's one-time encryption material; repeated payments do not publish a
stable receiving identity as destination. Balances and history are reconstructed
locally from successfully decrypted notes and current nullifier state.

Spend authority, full viewing capability and receiving identity are distinct
upstream key types. The full viewing key stays local. Future watch-only export
must be deliberate and clearly disclose its scope. The current user-facing
selective proof exposes an individual note opening rather than a global view key.

## On-chain privacy versus network privacy

Shielded notes protect **what** is paid and **to whom**, in every network mode.
They do not hide **where** a transaction entered the network. Network privacy is
a separate, optional layer: in `hybrid` and `tor-only` modes the node can reach
peers through Tor, which hides this computer's IP address from those peers. See
[network privacy and Tor](../network-privacy.md).

## What remains observable

The implementation does not prove complete anonymity.

- In `direct` and `hybrid` modes, clearnet peers see this computer's IP address.
- Even through Tor, peers observe connection timing; an adversary who sees both
  ends of a Tor circuit can correlate traffic.
- The first relay peer of a transaction created here learns that this node sent
  it first. The one-hop stem and per-peer relay delays reduce, but do not
  remove, origin inference.
- Public fees, transaction sizes, action counts and expiry heights are visible.
- Username lookups go to the user's own node; legacy resolvers see lookups.
- Small anonymity sets, compromised endpoints, voluntary proofs and off-chain
  metadata can reveal information.

The new consensus integration has not received an external audit. See the
[threat model](../security/threat-model.md) and corrected-circuit ADR.
