# Release gate

This gate is not passed. Status is recorded against exact commits in the
development journal. Every item needs executable evidence, not an assertion.
The mainnet has been live since 10 October 2026; this checklist governs every
further release, and an unchecked item means "no exact-commit evidence yet".

The full functional Windows/Linux matrix passed for `1911bd6` in
[run 37703358653](https://github.com/deepdrogo/GENORYN/actions/runs/37703358653).
Later source changes (including releases 0.4.0–0.5.0 and the network-privacy
work) require another exact-commit pass. Local results for the network-privacy
work are in [network-privacy evidence](network-privacy-evidence.md); they are
Windows-only and do not replace the CI matrix.

## Functional and consensus

- [ ] Linux daemon/miner/CLI/native wallet build and run
- [ ] Windows daemon/miner/CLI/native wallet build and run
- [ ] Three independently validating TCP peers synchronize and relay blocks
- [ ] Real RandomX mining with explicit destination
- [ ] Alice-to-Bob private transfer; unrelated Carol cannot scan it
- [ ] Bob spends received value; duplicate/nullifier conflicts are rejected
- [ ] Invalid signature/proof/amount/network/PoW/block are rejected
- [ ] Seed recovery reconstructs the same balance and receiving identity
- [ ] Username payment omits the username from consensus transaction bytes
- [ ] Voluntary per-output payment proof verifies; tampering is rejected
- [ ] Heavier-fork reorganization restores/reapplies wallet and nullifier state
- [ ] Abrupt crash/restart and offline peer catch-up preserve validity
- [ ] Exact 21M cap, 100K bootstrap, 210K treasury, vesting and halving tests
- [ ] Stable/spike/drop/oscillation/time-manipulation retarget tests
- [ ] Windows/Linux consensus vectors agree byte for byte
- [ ] Malformed/oversized P2P and RPC input tests
- [ ] No seed/password/private witness in node or miner logs
- [ ] Admin RPC cannot be publicly bound; public RPC cannot mutate state
- [ ] Native wallet interrupted/repeated/lock/close flows and real screenshots
- [ ] Formatting, strict clippy, unit/integration/property suite pass
- [ ] Dependency audit, license inventory and release checksums

## Network privacy (`cargo test -p genoryn --test network_privacy`)

- [ ] Tor-only mode makes no direct P2P connection, with or without Tor running
- [ ] SOCKS refusals and proxy failures never fall back to clearnet
- [ ] Onion and tor-only host names reach the proxy as names (no local DNS)
- [ ] No connection is attempted before Tor finishes bootstrapping
- [ ] Proxy restart drops and restores Tor-routed peers; control-port restart
      re-registers the same onion service
- [ ] Hybrid mode routes each destination by policy; direct mode refuses onion peers
- [ ] Unupgraded v1 peers never receive an undecodable message
- [ ] Hostile peer messages cannot steer a tor-only node onto clearnet
- [ ] Credentials, onion keys, onion hostnames and peer addresses never reach logs
- [ ] Auxiliary HTTPS (seed list, wallet update check) goes through Tor by name
      in tor-only mode and fails closed
- [ ] A live test against a real Tor daemon by an operator (not automated)

An external audit remains distinct from local test completion. Production
signing keys are intentionally absent; the mainnet treasury is burned.
