# ADR 0001: reviewed shielded primitives, independent integration

Status: accepted for experimental devnet; no production security claim.

Use the maintained `orchard = 0.16.0` Rust implementation, dual MIT/Apache-2.0,
with the corrected `PostNu6_3` Action circuit and fresh GENORYN commitment state.
Never accept the historical `InsecurePreNu6_2` circuit. Orchard provides Halo 2
proofs, commitments, nullifiers, RedPallas authorization, one-time outputs and
authenticated note encryption. GENORYN does not introduce a new primitive or
edit a proof circuit. This is not a fork of the Zcash blockchain or its monetary
policy. Separate GENORYN network-bound signatures, deterministic mint validation,
anchor admission, nullifier uniqueness and canonical decoding are required.

Upstream cryptographic review does **not** audit this integration. The 2026
soundness advisory is material: `orchard < 0.14.0` and `halo2_gadgets < 0.5.0`
contained an under-constrained circuit. The fix changes verifying keys, and
canonical proof length is mandatory. Pin the Cargo lockfile, circuit version,
bundle version and consensus vectors. Dependency upgrades require review.

Source evidence checked 2026-10-07:
- [Official security advisory](https://github.com/zcash/zcash/security/advisories/GHSA-ghc3-g8w4-whf9)
- [ZIP 257, fixed circuit and proof-length consensus rules](https://zips.z.cash/zip-0257)
- [Orchard circuit versions](https://docs.rs/orchard/0.16.0/orchard/circuit/enum.OrchardCircuitVersion.html)
- [Upstream source, license and maintenance](https://github.com/zcash/orchard)

Security assumptions include discrete logarithm hardness on the Pasta curves,
Halo 2 soundness, collision-resistant hashes and secure randomness. Halo 2 uses
transparent parameters rather than a GENORYN trusted-setup ceremony. Proof and
verification costs must be measured, not advertised. Two-action bundles are
several kilobytes and proving is substantially more expensive than signing.
The library supports portable Rust targets; actual Windows/Linux builds and
canonical vectors remain release gates.

All ordinary transfer value stays shielded; only fees and deterministic issuance
are public. The public API has no address-balance query. Treasury commitments
are withheld from spendable anchors until consensus maturity; no private key
can bypass that rule. Domain separation and maturity integration need external
review before a mainnet genesis can be finalized.
