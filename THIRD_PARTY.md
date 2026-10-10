# Third-party software

GENORYN's own source is dual MIT/Apache-2.0. Dependencies retain their original
licenses and authorship. No upstream audit is represented as a GENORYN audit.
Cargo.lock pins exact source checksums and dependency versions.

| Component | Use | Source / license |
|---|---|---|
| Orchard 0.16 | Corrected shielded-note protocol and Halo 2 circuits | [zcash/orchard](https://github.com/zcash/orchard), MIT/Apache-2.0 |
| randomx-rs / RandomX | CPU proof of work / native binding | [tari-project/randomx-rs](https://github.com/tari-project/randomx-rs), binding and bundled upstream licenses apply |
| RustCrypto | Argon2id, XChaCha20-Poly1305, SHA-256, HKDF | [RustCrypto](https://github.com/RustCrypto), MIT/Apache-2.0 |
| ed25519-dalek | Receiver-bundle ownership signatures | [dalek-cryptography](https://github.com/dalek-cryptography/curve25519-dalek), BSD-3-Clause |
| bip39 | Standard mnemonic parsing and checksums | [rust-bip39](https://github.com/rust-bitcoin/rust-bip39), CC0-1.0 |
| rusqlite / SQLite | Crash-consistent local database | MIT / public domain |
| Tokio / Axum / Reqwest | Async network and HTTP | MIT and respective dependency licenses |
| eframe / egui | Native monochrome desktop interface | [emilk/egui](https://github.com/emilk/egui), MIT/Apache-2.0 |
| Hickory | Native DNS discovery | [hickory-dns](https://github.com/hickory-dns/hickory-dns), MIT/Apache-2.0 |
| Arti 0.47 | Built-in Tor client and onion service in `genorynd` | [Tor Project Arti](https://gitlab.torproject.org/tpo/core/arti), MIT/Apache-2.0 |
| Inter, Unbounded, JetBrains Mono | Wallet typography, compiled into `genoryn-wallet` (Latin subsets via Fontsource) | [rsms/inter](https://github.com/rsms/inter), [googlefonts/unbounded](https://github.com/googlefonts/unbounded), [JetBrains/JetBrainsMono](https://github.com/JetBrains/JetBrainsMono); SIL Open Font License 1.1, texts in `apps/genoryn-wallet/assets/fonts` and `third-party/fonts` of every package |

Use the release packager to collect dependency license files and exact metadata
with each distributable. This overview does not replace those license texts.
The collector preserves nested paths, including the bundled RandomX upstream
license and egui's embedded font notices (Hack, Ubuntu, Noto and emoji fonts).
Version-bound, checksummed upstream supplements cover notices omitted from
some crate archives. The packager fails closed if a dependency has no collected
notice text. [The native inventory and provenance](docs/security/dependency-licenses.md)
distinguish exact upstream notices from explicitly identified standard CC0 text,
and exclude inactive platform graphs. Mechanical completeness is not legal clearance.
Bundled tests use only valueless local development keys and encrypted outputs.

The `randomx-rs 1.6.0` binding is vendored with a narrow Rust FFI safety patch.
[ADR 0012](docs/adr/0012-randomx-binding-safety.md) and
[its source manifest](third-party/randomx-source.json) identify the original crate
checksum/revision and per-file hashes. A second wrapper addition lets miner threads
share one read-only dataset ([ADR 0016](docs/adr/0016-fast-mode-mining.md),
[patch](third-party/randomx-shared-dataset.patch)). Native RandomX source is unchanged.
The Rust build script isolates upstream generated assembly under Cargo's output
directory and rejects external source overrides; this build-only delta is
recorded separately from the FFI patch.
Packages retain both binding and native licenses and include
`VENDORED-SOURCE-PROVENANCE.json`; this is not a new upstream release or audit.

Arti's `saturating-time 0.5.0` is vendored with a termination fix: its search
for the platform's `SystemTime` limits never ended on Windows (100 ns clock),
which stalled the built-in Tor client while parsing every consensus.
[ADR 0015](docs/adr/0015-built-in-tor-client.md),
[its source manifest](third-party/saturating-time-source.json) and
[the patch](third-party/saturating-time-windows-limits.patch) record the
original checksum, upstream revision and per-file hashes. The packager refuses
any crates.io patch without such a record.
