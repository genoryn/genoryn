# My Full Node, faster sync, and the Privacy & Security Center (GENORYN 1.1)

## My Full Node

Network → **My Full Node** turns the existing owned-node controls into one page. **Start My
Full Node** runs safe checks first, then starts `genorynd` as a child of the wallet:

| Check | What is checked | Blocks the start when |
|---|---|---|
| Platform | OS and architecture | never (warns outside Windows/Linux x86-64) |
| Disk space | free space where the chain is stored | under 200 MB (warns under 4 GB) |
| Memory | installed memory (the node needs about 0.5 GB) | never |
| Node program | `genorynd` next to the wallet runs | it is missing or does not run |
| Data folder | can be created and written; is the Mainnet 1.0 folder | it cannot be written |
| RPC port | the loopback RPC address is free | another program holds it without answering as a node |
| Existing node | a node already answers at the RPC address | — (it is used; a second one is never started on the same data) |
| Network mode | Tor only / Hybrid / Direct, as chosen on the Network page | — |

Starting never opens an inbound port, changes the firewall, enables inbound listening or
installs a service. The data folder holds an exclusive database lock, so two nodes can never
share it. Only a node this wallet started can be stopped from it; a node started elsewhere is
used as it is.

The status card shows: running / starting / not answering / connected to an external node /
stopped; the verified height; the network identity compared with the built-in Mainnet 1.0
genesis and chain id; uptime; peers in and out; storage; Tor and onion status; and recent
warnings and errors from the opened log. Synchronization is described from the node's own
progress only ("synchronizing", "following its peers", "no peers yet"): a percentage would need
an independently verified target height, which does not exist, and peer counts are this node's
own connections, not the size of the network.

**Optional, off by default, only with your consent:**
- *Keep my node running after I close the wallet*: the node keeps validating; its process id is
  written to the node folder (`genorynd.background.pid`) so the wallet can stop exactly that
  process later (it checks that the id still belongs to `genorynd`).
- *Start my node when I sign in*: a per-user entry in your own sign-in start folder (Windows
  Startup folder / `~/.config/autostart`). No administrator rights, no system service, no port.

A guide on the same page explains how to run a publicly reachable node, a Tor onion node and a
public read-only RPC endpoint (a Quick Wallet source) — the admin RPC and its token are never
exposed by any of them — and why peer discovery never decides validity.

## Faster wallet synchronization

Measured on the real Mainnet 1.0 chain (1,212–1,238 blocks, a local node synchronized over
Tor; Intel Core Ultra 9 285K, 24 logical CPUs, Windows 11, release builds). Before and after
use the same node, chain and benchmark (`scripts/test/sync_bench.py`,
`crates/genoryn-core/examples/replay_bench.rs`).

| Operation | 1.0.5 | 1.1 | How |
|---|---|---|---|
| Any send / proof operation in Full mode, again | 11.29 s | **0.40 s** | the wallet keeps its sealed verified chain and a sealed state snapshot; only new blocks are fetched and applied |
| The same, first time after upgrading | 11.36 s | 6.29 s | 32-block pages, one commitment-tree root per block |
| Full validation of the whole chain (Quick mode first sync) | 49.0 s | **18.0 s** | RandomX work and Halo 2 proofs of long downloads are checked on 16 threads first; every block is then applied in order with every rule |
| Replay of already-validated blocks | 9.5 s | 5.9 s | the commitment-tree root is computed once per block instead of twice |
| First Full-mode scan / incremental scan | 1.24 s / 0.25 s | 1.22 s / 0.24 s | unchanged (already loopback-fast) |

What did **not** change: no cryptographic check is skipped or disabled. A remote node's chain is
still downloaded whole and validated in full; only the user's own node's blocks (which it
validated) and blocks this computer validated before are reused. Every replay produces the same
final state, checked against the sequential replay (tip, cumulative work, both state roots).

Crash safety and growth:
- The scan cache is saved every 2,000 blocks during a long first scan, so a crash or power loss
  resumes from the last checkpoint instead of from genesis.
- The state snapshot (`wallet.chainstate`) is sealed with a wallet-derived key, bound to the
  program version that made it (a new version replays once), and used only if its tip block is
  still in the kept chain; a reorganization, a damaged or foreign file only costs a replay.
- Existing `.scan`, `.chain`, `.outbox` and wallet files are read unchanged; the new files are
  additions that older versions ignore.
- Nodes serve up to 32 blocks per `v1/blocks` request (bounded to 4 MB); clients accept the
  single-block pages of 1.0 nodes.

## Privacy & Security Center

Wallet → **Privacy & Security** lists what this computer can actually observe. Each check is
**Verified**, **Warning**, **Failed** or **Not tested**, and says what it is based on. Two
informational kinds are kept apart and are never shown as verification: **Your setting** (what
you chose) and **Your statement** (what you confirmed, such as a backup).

| Area | Checks |
|---|---|
| Network | configured mode (setting); the mode the node actually runs; Tor bootstrap (reported by your node); peers through Tor; direct clearnet connections and direct attempts since start; proxy failure fails closed; names resolved by Tor (socks5h only); Tor-only keeps wallet RPC local |
| Synchronization | the source (own node, external node, or Quick-mode onion sources) and what remote sources can and cannot do |
| Wallet | lock state; 5-minute inactivity lock; wallet file is an encrypted envelope without readable words; GENORYN's own folders contain no plain-text recovery phrase; backup (your statement, with a checklist) |
| Payments | shielded Orchard transfers (consensus) |
| Software | version and update status (GitHub release record, through Tor in Tor-only mode); release authenticity (checksums only — not an independent publisher signature) |

The controlled tests run only on this computer: a request through a deliberately unreachable
proxy must fail instead of going out directly; a proxy that would resolve names locally is
refused; a remote RPC endpoint is refused in Tor-only mode. Nothing is uploaded and no
privacy-check website is contacted. The storage check reads only GENORYN's data and wallet
folders, reports file names (never contents), and does not claim anything about copies of a
recovery phrase elsewhere. There is no combined anonymity score, and the page never promises
protection against a compromised operating system or a global traffic-correlation adversary.
