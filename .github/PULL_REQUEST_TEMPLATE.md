## What this changes

<!-- One change per pull request. What does it do, and why? -->

## How it was tested

<!-- Tests added or updated, and what you ran. -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Site or wallet texts changed: catalogs regenerated (`website/i18n.py extract`, `website/i18n_wallet.py extract` and `build`) and `website/i18n_check.py --dist` and `website/i18n_check.py --wallet` pass
- [ ] Hive OS package changed: `integrations/hiveos/tests/run.sh` passes
- [ ] Consensus rules are unchanged, or the change has a specification update and an activation height
- [ ] No vulnerability details are disclosed here (those go through a private advisory)
