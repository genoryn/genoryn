# Shielded transactions v1

Each transaction contains a signed context and one Ironwood-v3 bundle. Context
binds protocol version, GENORYN network, kind, height, expiry and a nonce. A
transfer uses kind 0, height 0, a random nonce and bounded block-height expiry.
Miner/treasury issuance uses kind 1/kind 2, its actual height, no expiry and the
previous block hash as nonce. Cross-network signature reuse is rejected.

Actions carry only nullifiers, randomized authorization keys, note commitments,
ephemeral keys, encrypted note payloads, outgoing ciphertexts, value commitments
and signatures. The bundle also carries its flags, anchor, public fee/value
balance, canonical-length Halo 2 proof and binding signature. No username, ordinary
receiver, transfer amount or plaintext memo field exists in the wire transaction.

Validation checks all signatures, the corrected proof, flag/amount constraints,
recognized mature anchor, duplicate nullifiers and chain/mempool conflicts.
Ordinary transfers have 2..16 actions; coinbase classes have exactly one. Maximum
encoded transaction size is 200,000 bytes. Canonical fixed-width little-endian
bincode 1 encoding rejects trailing data and must roundtrip byte for byte.

The signature digest uses GENORYN domain separation, network genesis, context,
upstream bundle commitment, anchor, flags and value balance. The transaction ID
additionally binds the entire authorized encoded transaction. Upstream APIs are
used for point/field/proof serialization; custom circuits are not introduced.

Wallets generate and scan locally. A selective payment proof reveals one note
opening and matches it to a validated chain transaction. It does not disclose a
spend key or wallet-wide viewing key; the chosen output's recipient/amount become
visible to the verifier. This is intentional disclosure, not public metadata.
