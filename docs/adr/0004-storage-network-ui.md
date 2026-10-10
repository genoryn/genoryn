# ADR 0004: local-first storage, networking and wallet

Use SQLite via rusqlite with WAL, FULL synchronous transactions, integrity
checks and explicit schema/network/genesis metadata. Canonical block bytes are
the recoverable source of truth; best-chain changes are committed atomically.
Rebuild derived shielded state by replay when loading or selecting a heavier
branch. This prioritizes correctness over production-scale replay speed.

Use bounded, versioned TCP frames over Tokio for the initial peer protocol:
network magic, chain ID, genesis and protocol version in every handshake; peer
cache and peer exchange; bounded requests, response ranges, concurrency and
timeouts. Independent peers select cumulative validated work. Discovery peers
have no authority. Public internet transport hardening, Tor/I2P adapters and
production anti-eclipse resilience require later review.

Wallet uses the same Rust core as the CLI. Native GUI will use egui/eframe to
avoid a web-origin IPC bridge and platform webview dependencies. Strictly
monochrome views expose actual node/wallet results and honest pending/error
states. No websites are part of the local release. Seeds use BIP39, file
encryption uses Argon2id and XChaCha20-Poly1305, with secret zeroization.
