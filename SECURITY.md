# Security policy

## Status

Genoryn Krypton has extensive automated tests (consensus, proofs, PoW, reorganizations,
light-client forgery, parser property tests) but **no independent audit yet**. Treat the
testnet as experimental. Highest-value review targets:

1. The spend AIR and its zero-knowledge masking (`crates/krypton-zk`, `docs/spec/spend-circuit.md`)
2. Consensus and reorganization code (`crates/krypton-core/src/consensus.rs`, `crates/krypton-store`)
3. Light-client verification (`crates/krypton-light`)
4. Wallet key handling and file encryption (`crates/krypton-wallet-core`)
5. The P2P handshake (`crates/krypton-p2p/src/session.rs`)

## Reporting a vulnerability

Do not open a public issue for vulnerabilities. Use GitHub's private vulnerability reporting
("Security" → "Report a vulnerability") on `github.com/genoryn/genoryn`. Include a
description, impact and reproduction steps. Never test against other people's funds or nodes;
use devnet or your own testnet node.

## Threat model in short

* Quantum adversary: spends rely on hashes only; note encryption and peer links use hybrid
  ML-KEM; releases carry hybrid ML-DSA + SLH-DSA signatures.
* Chain observer: sees fees, proof sizes and timing, not amounts, senders or recipients.
* Malicious light-client server: can withhold data, cannot forge chain data (every
  response is verified). Use several sources; they are cross-checked.
* Compromised computer: out of scope; protect your recovery phrase offline.
