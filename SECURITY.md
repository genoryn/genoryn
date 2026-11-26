# Security policy

## Status

Genoryn Krypton has extensive automated tests (consensus, proofs, PoW, reorganizations,
light-client forgery, parser property tests) but **no independent audit yet**. The internal
reviews and the fixes they led to are in [docs/audit](docs/audit): the latest, a review of the
whole system fixed in 1.0.53, is
[docs/audit/2026-11-26-system-audit-1.0.53.md](docs/audit/2026-11-26-system-audit-1.0.53.md),
and every review is summarised at [genoryn.io/audits](https://genoryn.io/audits).
Highest-value review targets:

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

What to expect (targets, not promises, for a project without a company behind it):

| Step | Target |
|---|---|
| Acknowledgement of your report | within 3 days |
| First assessment (confirmed or not, and how severe) | within 14 days |
| Fix and release | as fast as a safe, tested fix allows; you are told the plan |
| Public disclosure | after a fixed release is available, agreed with you; credit if you want it |

In scope: the node, the miner, the wallets, the swap engine and offer board, the light client,
the release signatures, the Hive OS package and genoryn.io. Reports about the infrastructure
behind genoryn.io go through the same private channel; this policy does not describe it.

## Threat model in short

* Quantum adversary: spends rely on hashes only; note encryption and peer links use hybrid
  ML-KEM; releases carry hybrid ML-DSA + SLH-DSA signatures.
* Chain observer: sees fees, proof sizes and timing, not amounts, senders or recipients.
* Malicious light-client server: can withhold data, cannot forge chain data (every
  response is verified). Use several sources; they are cross-checked.
* Compromised computer: out of scope; protect your recovery phrase offline.
