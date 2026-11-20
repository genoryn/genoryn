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
- **Check a translation** of the site or the wallet (see below).
- **Test a mining integration** such as the Hive OS package on a real rig (see below).

## Translations

The site and the wallet are written in English and machine-translated into nine languages.
People who speak them are the best reviewers: open a
[Translation issue](https://github.com/genoryn/genoryn/issues/new/choose) or change the catalog.

- Guide: [docs/localization/README.md](docs/localization/README.md); terms that must stay the
  same everywhere: [docs/localization/glossary.md](docs/localization/glossary.md).
- Catalogs: `website/locales/<lang>/*.json` (site) and `apps/krypton-wallet/locales/<lang>/*.json`
  (wallet). Never edit `locales/en`: it is generated from the pages and the code.
- A catalog you checked yourself can say `"_review": "human"`.
- Never translate recovery-phrase words, addresses, swap codes, commands or anything else the
  glossary lists, and translate warnings about money and keys exactly.

## Mining integrations

Genoryn Krypton has no pools: every miner is paid by shares in consensus
([docs/mining](docs/mining/native-rewards.md)). An integration must keep it that way:

- It never asks for a recovery phrase, password or key; the miner needs an address only.
- It adds no fee and never changes the payout address the user gave.
- It reports only numbers the miner really has (see `krypton-miner --json`).
- It is tested before it is proposed. The Hive OS package has its own tests:
  `integrations/hiveos/tests/run.sh` (scripts) and `integrations/hiveos/tests/live-run.sh`
  (the real package against a devnet node, in Docker).

## Security problems

Do **not** open a public issue for a vulnerability. Report it privately through
[GitHub's private vulnerability reporting](https://github.com/genoryn/genoryn/security/advisories/new).
See [SECURITY.md](SECURITY.md).

## Building and testing

You need Rust 1.89 or newer. No other toolchain is required.

```sh
cargo build --release                                   # node, miner, wallets
cargo test --workspace                                  # all tests
cargo fmt --all -- --check                              # formatting
cargo clippy --workspace --all-targets -- -D warnings   # lints
python website/build.py                                 # the static site, into website/dist
python website/i18n_check.py --dist                     # translations and localized pages
python website/i18n_check.py --wallet                   # the wallet's translations
```

After changing a page or a text the wallet shows, regenerate the English catalogs:
`python website/i18n.py extract`, `python website/i18n_wallet.py extract` and
`python website/i18n_wallet.py build` (CI checks that they are current).

Run your changes on a private network before proposing them:

```sh
kryptond --network devnet --no-seeds
```

Never test against other people's funds or nodes.

## Pull requests

- Work on a branch and open a pull request; nothing is pushed to `main` directly.
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
| `docs/mining`, `docs/hiveos`, `docs/localization` | Mining guides, Hive OS, translation guide |
| `integrations` | The Hive OS package and material for mining listing sites |
| `website` | genoryn.io (`website/locales`: its translations) |

## License

By contributing you agree that your contribution is licensed under
[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at the user's option, like the rest of
the project.
