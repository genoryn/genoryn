# Contributing to Genoryn Krypton

Genoryn Krypton (GNR) is an open-source, quantum-resistant, private-by-default proof-of-work
cryptocurrency. Nobody owns the protocol, and contributions are judged on their merits, in
public. Thank you for helping.

## Ways to help

- **Review the code.** The most valuable targets are listed in [SECURITY.md](SECURITY.md):
  the spend circuit, consensus, the light client, wallet key handling and the P2P handshake.
- **Report bugs** with the [bug report form](https://github.com/genoryn/genoryn/issues/new/choose).
- **Improve the documentation**: the [README](README.md), the [specifications](docs/spec) and
  the website in [`website/`](website).
- **Run a node**, mine, and tell us what was unclear.

## Security problems

Do **not** open a public issue for a vulnerability. Report it privately through
[GitHub's private vulnerability reporting](https://github.com/genoryn/genoryn/security/advisories/new).
See [SECURITY.md](SECURITY.md).

## Building and testing

You need Rust 1.88 or newer. No other toolchain is required.

```sh
cargo build --release                                   # node, miner, wallets
cargo test --workspace                                  # all tests
cargo fmt --all -- --check                              # formatting
cargo clippy --workspace --all-targets -- -D warnings   # lints
python website/build.py                                 # the static site, into website/dist
```

Run your changes on a private network before proposing them:

```sh
kryptond --network devnet --no-seeds
```

Never test against other people's funds or nodes.

## Pull requests

- Keep a pull request to one change, and explain what it does and why.
- Add or update tests for behaviour you change. Consensus, proof and wallet code needs tests.
- Run the four commands above; CI runs them on Linux, Windows and macOS.
- A change to consensus rules needs a specification update in [`docs/spec`](docs/spec) and
  an activation height, never an immediate switch.
- Match the style around you: plain words in comments and documents, no unsafe code in
  networking, no new dependencies without a reason.

## Where things are

| Path | What |
|---|---|
| `crates/krypton-core` | Consensus, transactions, emission, parameters |
| `crates/krypton-zk` | The zk-STARK spend circuit, prover and verifier |
| `crates/krypton-crypto` | Hashes, ML-KEM, signatures, key derivation |
| `crates/krypton-pow` | KryptoHash proof of work |
| `crates/krypton-p2p`, `crates/krypton-store`, `crates/krypton-light` | Network, storage, light client |
| `crates/krypton-wallet-core`, `crates/krypton-swap` | Wallet logic, atomic swaps and the offer board |
| `apps/kryptond`, `apps/krypton-miner`, `apps/krypton-wallet`, `apps/krypton-wallet-cli` | Node, miner, desktop wallet, command-line wallet |
| `docs/spec` | Specifications |
| `website` | genoryn.io |

## License

By contributing you agree that your contribution is licensed under
[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at the user's option, like the rest of
the project.
