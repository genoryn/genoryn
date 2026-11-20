# Genoryn Krypton (GNR) on Hive OS

A Hive OS custom miner package for `krypton-miner`, the official Genoryn Krypton miner
(KryptoHash v1 on NVIDIA through CUDA, AMD and Intel through OpenCL, or the CPU).

**No external pools. No pool fees. Every miner is rewarded directly by the blockchain in
proportion to their valid shares.** You mine through a Genoryn node: your own, or the public
one at `https://genoryn.io`. Rewards are paid to your address every 360 blocks (about six
hours) by the chain itself. See [docs/mining](../../docs/mining/for-miners.md).

## What you need

- A GNR address from the Genoryn Krypton wallet: the **short address** (`gnrs1...`, about 57 characters) from the wallet's Receive panel, or the full `gnr1...` address. **Never** a recovery phrase, a password or a key: mining needs none of them, and nobody will ever ask for them.
- A GPU with at least 4 GB of memory (the dataset is 2 GiB and grows by 4 MiB every 16,384 blocks), or a CPU.
- A Hive OS image based on Ubuntu 20.04 or later.

## Flight Sheet

| Field | Value |
|---|---|
| Coin | `GNR` (add it under Wallets if Hive OS does not list it) |
| Wallet | your GNR address |
| Pool | Configure in miner |
| Miner | Custom |
| Installation URL | `https://github.com/genoryn/genoryn/releases/download/v<version>/genoryn-<version>.tar.gz` |
| Hash algorithm | `kryptohash` |
| Wallet and worker template | `%WAL%` |
| Pool URL | `https://genoryn.io` (or your own node, see below) |
| Pass | (empty) |
| Extra config arguments | optional, e.g. `--backend cuda --devices 0,1` |

**Extra config arguments** may only contain these options; anything else is refused before
the miner starts:

| Option | Values |
|---|---|
| `--backend` | `auto` (default: CUDA, else OpenCL, else CPU), `cpu`, `cuda`, `opencl` |
| `--devices` | GPU indices, e.g. `0,1` (default: all) |
| `--threads` | CPU threads when mining on the CPU |
| `--max-cpu` | upper bound on CPU use in percent, 1 to 100 |
| `--batch` | nonces per GPU launch (auto-tuned when omitted) |

To update, change the version in the Installation URL. To go back, put the earlier version
back; Hive OS installs exactly the package that URL names.

## Your own node (recommended for farms)

Every rig asks its node for work about every two seconds. The public node limits requests
per IP address, so a farm behind one IP should run one node on its network and point every
rig at it, for example `http://192.168.1.10:30334`:

```sh
# On any Linux machine of the farm (Ubuntu 22.04 or later):
wget https://github.com/genoryn/genoryn/releases/latest/download/genoryn-krypton-<version>-linux-x64.tar.gz
tar -xzf genoryn-krypton-*-linux-x64.tar.gz && cd genoryn-krypton-*/
./kryptond --data-dir ~/.genoryn --rpc 0.0.0.0:30334   # keep port 30334 inside the farm
```

Plain `http://` is accepted only to this computer and to private network addresses
(`10.x`, `172.16-31.x`, `192.168.x`). Anything else must be `https://`.

## What the dashboard shows

From the miner's own status line (every 10 seconds):

- **Hashrate**: the miner's total, in H/s. The miner does not measure each GPU separately, so the package reports the total only. Temperatures and fans come from Hive OS itself.
- **Accepted / rejected**: shares the node accepted (a found block counts as a share too) and solutions it did not accept.
- **Uptime** and **version**.

Not shown, because the miner does not have them: a worker name (the network knows miners by
address only) and a separate count of invalid shares. If the log has been silent for a
minute, the package reports zero rather than old numbers.

Your unpaid reward and the next payout are on the node:
`https://genoryn.io/v1/miner/<your address>`, in the wallet's Mining tab, and on
genoryn.io/mining.

## Safety

- `h-run.sh` starts the miner only if its SHA-256 matches `krypton-miner.sha256` in the package; that package is listed in the release's `SHA256SUMS`, signed with the project's post-quantum release key (`docs/release-key.pub`).
- Only one miner runs per rig (a lock file).
- The Flight Sheet's values are checked with fixed patterns and never run as shell code. The config file is readable by root only.
- There is no developer fee and no hidden address: the miner refuses any work from a node that does not pay your own address.

Verify a download yourself:

```sh
sha256sum -c SHA256SUMS --ignore-missing     # genoryn-<version>.tar.gz
krypton-release verify release-key.pub SHA256SUMS SHA256SUMS.sig
```

## Troubleshooting

| The log says | Do this |
|---|---|
| `the wallet in the Flight Sheet is not a GNR address` | Put your `gnrs1...` or `gnr1...` address in the wallet; nothing else |
| `the node does not know the short address` | Open the wallet once while it is online (it publishes the address), or use the full address |
| `the pool URL must be https://...` | Use `https://genoryn.io` or `http://<farm node IP>:30334` |
| `waiting for node` | The node cannot be reached: check the URL, the network, the node |
| `does not match its checksum` | The package is damaged: reinstall it from the official release |
| `another Genoryn miner is already running` | Stop the other miner first |
| `no usable NVIDIA GPU found` | Check the driver (`nvidia-smi`), or use `--backend opencl` / `cpu` |

Package sources and tests: [`integrations/hiveos`](.) in
[genoryn/genoryn](https://github.com/genoryn/genoryn). Run the tests with
`integrations/hiveos/tests/run.sh` (needs Docker).
