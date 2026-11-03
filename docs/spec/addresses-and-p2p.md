# Addresses, notes and the peer protocol

## Keys

The 24-word recovery phrase (BIP-39, 256-bit entropy) yields a 64-byte seed. SHAKE256 with
purpose tags `"<network>/<account>/spend|kem|mldsa"` derives the spend secret, the hybrid KEM
seed (X25519 + ML-KEM-1024 via FIPS 203 deterministic key generation) and an ML-DSA-87 seed.

## Address

`version (1) | network (1) | owner (32) | X25519 pk (32) | ML-KEM-1024 ek (1568)`, followed by
a 6-byte SHA3 checksum, encoded as lowercase RFC 4648 base32 after the prefix (`gnr1`,
`tgnr1`, `dgnr1`). The short address is `prefix + "s1" + base32(H("Krypton/short-address", payload))`;
nodes resolve it from a directory and the wallet re-checks the hash.

## Note encryption

`ct = kem_ct (32 + 1568) || XChaCha20-Poly1305(key, nonce = H(cm)[..24], aad = cm,
value u64 || rcm 32 || memo 128)`, 1,784 bytes per output. The KEM secret is
`H("Krypton/HybridKEM/X25519-MLKEM1024/v1", ss_mlkem, ss_x25519, ct, pk)`.

## Peer protocol

TCP. Handshake: the initiator sends `magic | version | X25519 eph | ML-KEM-768 ek`, the
responder answers `magic | version | X25519 eph | ML-KEM-768 ct`; keys are derived from both
secrets and the transcript. Frames: `u32 length || ChaCha20-Poly1305(counter nonce)`, at most
8 MiB. Messages: Hello, Ping/Pong, GetHashes/Hashes (locator-based sync), GetBlocks/Block,
Inv, GetTxs/Tx, GetAddr/Addr. Limits: 8 outbound, 64 inbound, 4 per IP; malformed data
bans the peer's IP for 24 hours. Your own transactions go first to one random peer and are
announced to everyone after 20 seconds (stem/fluff).
