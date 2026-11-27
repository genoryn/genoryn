# Hive OS

Mining Genoryn Krypton (GNR) on Hive OS with the official custom miner package.

- **Installation, Flight Sheet, troubleshooting:** [integrations/hiveos/README.md](../../integrations/hiveos/README.md) (the same text ships inside the package).
- **The package:** `genoryn-<version>.tar.gz` on each [release](https://github.com/genoryn/genoryn/releases), listed in the release's signed `SHA256SUMS`.
- **How rewards work:** [docs/mining](../mining/for-miners.md).

## Quick start

1. In the Genoryn wallet, open Receive, choose **Short address**, press **Publish short form**
   if it is not published yet, and copy it (`gnrs1...`).
2. In Hive OS, add a wallet for coin `GNR` with that address.
3. Create a Flight Sheet:
   - Miner: Custom.
   - Installation URL: the release's `genoryn-<version>.tar.gz`.
   - Hash algorithm: `kryptohash`.
   - Wallet and worker template: `%WAL%`.
   - Pool URL: `https://genoryn.io`, or your farm's node.
4. Apply it to the rig.

For a farm, run one `kryptond` on the farm's network and use `http://<its IP>:30334` as the
pool URL. Every rig then asks it for work instead of the public node.

## Building and testing the package

```sh
integrations/hiveos/build-package.sh <version> <out-dir>   # builds in Ubuntu 20.04 (Docker)
integrations/hiveos/tests/run.sh                           # checks of every script
integrations/hiveos/tests/live-run.sh <out-dir>            # the real package on a devnet node
```

The package is built inside Ubuntu 20.04 so it runs on every Hive OS image from 20.04 on.
`h-run.sh` checks the miner's SHA-256 before every start.
