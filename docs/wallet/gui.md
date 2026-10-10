# Native wallet

`genoryn-wallet` is a native Rust/eframe desktop executable. It uses the shared
Rust wallet through a one-request CLI subprocess rather than duplicating
consensus or cryptographic code. There is no embedded website or remote UI.
Only black, white and gray are used, including QR codes and status/error states.
The GUI never fabricates balances, transaction history, hashrate, peers, prices,
sync percentages or supply. A missing response is shown as unavailable.

## Build and run

```sh
cargo build -p genoryn-wallet -p genoryn
./target/debug/genoryn-wallet --data-dir ./data/alice --rpc-url http://127.0.0.1:29411
```

On Windows, run the corresponding `.exe`. Place `genoryn-wallet-cli`, `genorynd`
and `genoryn-miner` beside the GUI executable, or specify `--bin-dir PATH`.
A development build requires a supported desktop display. Linux uses OpenGL
with X11 or Wayland; Windows uses Direct3D 12, including a compatible software
adapter. See the [platform requirements](../quickstart.md) before downloading
a binary. Actual native-window checks run separately on both CI platforms;
source compatibility alone is not evidence that a desktop runtime passed.

Optional arguments: `--wallet PATH`, `--data-dir PATH`, `--rpc-url URL`,
`--bin-dir PATH`, `--node-config PATH`, `--auto-node on|off`. `GENORYN_DATA_DIR` changes the default data directory. Without
it the default is `$HOME/.genoryn/mainnet` (or `%USERPROFILE%/.genoryn/mainnet`). The GUI
always uses the mainnet; development networks are reached with the CLI programs.
Settings are session-only, except the update-check preference, the dark/light
appearance and the list of remembered wallet files, which are stored as
`wallet-preferences.json` in the data directory.
An explicit `--wallet` path takes precedence over the data-directory-derived
default regardless of argument order. Missing/unknown options fail before a
window opens; passwords and seeds are never command-line options.

## Real workflows

- **Create:** choose a wallet path and password of at least ten characters.
  The paired CLI writes the encrypted wallet. The GUI displays the new recovery
  phrase and requires an explicit offline-backup acknowledgement before use.
  Do not put real funds in this unaudited software or reuse production seeds.
- **Restore:** provide a seed and new encryption password using masked inputs.
  Reconstruction scans the actual available chain. Username-registry metadata
  and local contact-owner pins need a [separate backup](../quickstart.md#back-up-and-restore-local-metadata); the seed recovers keys.
- **Wallet files:** Unlock lists the wallet files found in the folder and, on
  Windows, opens the system file dialog (Browse…). Create and Restore take a wallet
  name and a folder (Choose folder… on Windows) and show the resulting path. Names
  are limited to letters, digits, spaces, dots, dashes and underscores; reserved
  Windows device names are refused and an existing file is never overwritten.
- **Several wallets:** up to eight wallets can be unlocked at once. The wallet menu
  in the top bar switches between them, opens another wallet or closes the active
  one; Overview and the top bar show the combined balance of all open wallets.
  Switching clears payment drafts and ignores late results of the previous wallet.
  Inactive wallets are rescanned in the background, one at a time, with the same
  automatic-scan pacing. Lock and the inactivity lock close every open wallet.
- **Lock screen:** until a wallet is unlocked, only the unlock screen is reachable;
  the sidebar is disabled, including Node, Mining, Network, Settings and the proof
  verifier. A running miner keeps mining while the wallet is locked and the lock
  screen says so: mining pays a receiving address and needs no keys.
- **Unlock / lock:** unlock invokes the CLI to authenticate the encrypted file.
  Lock erases held password and recovery-phrase buffers, clears local wallet
  views, cancels in-flight CLI work and invalidates late responses. Lock is
  automatic after five minutes without input. A canceled payment may already
  have reached a node; rescan before attempting to send it again.
- **Overview / Transactions:** scan the node chain locally, showing only the
  user's reconstructed private notes and history. Values use exact 12-decimal
  atomic-unit conversion, never binary floating point. A scan's height and age
  are visible. History displays the backend's actual confirmation count.
  Unscanned is distinct from zero. While unlocked, the wallet rescans by itself
  when the node reports a block it has not scanned. Scans run in their own
  background lane, shown at the bottom right, and never disable other actions.
  With the wallet's own loopback node, a scan reads only blocks added since the
  last one from an encrypted cache beside the wallet (`wallet.scan`, keyed from the
  seed); a fork is rolled back to the shared block, and Rescan history rebuilds
  the cache from the first block. A remote node, or `full_validation`, uses the
  full validating scan instead. Payments always build from a full validated scan.
- **Transactions:** Transfers (received and sent, shown first) and Mined (block
  rewards and treasury issuance) are separate categories. Search matches a
  transaction ID, memo, amount prefix or block number; filters select direction
  and whether the display confirmation target is reached. Lists show 50 entries
  at a time.
- **Payment proof export:** select a transaction in Transactions, choose its
  zero-based output index and prepare the output locally. A transaction can
  contain recipient and change outputs; the wallet can disclose only an output
  it sent or can decrypt. Review the returned recipient, exact amount, network,
  transaction and output index before explicitly acknowledging disclosure.
  The JSON also exposes the output commitment and note-opening data (`rho` and
  `rseed`), so anyone receiving it can link that recipient and amount to the
  output. It contains no spending key, memo or other output opening. Only then
  may you copy it or save to `DATA_DIR/payment-proofs/TXID-INDEX.json`.
  A save creates a new file, never overwrites an existing one, and uses mode
  `0600` on Unix. Other platforms use the local directory's access controls.
  No proof is automatically shared or uploaded. Clipboard readers and history
  can retain a copied proof.
- **Payment proof verification:** expand the verifier in Transactions, including
  while the wallet is locked. Paste JSON or load a regular local UTF-8 file;
  files, text and reads are bounded to 16 KiB. URLs, network shares, direct
  symlinks and special files are refused by the file loader. Parsed input uses
  the shared `NoteProof` format and mainnet recipient parser. The paired CLI
  validates the selected chain and verifies the proof against its transaction;
  no wallet password, wallet path or admin token is sent for verification.
  The result identifies the exact recipient, amount, transaction and output.
  Verification establishes the disclosed output at that time, not sender
  identity, unspent status or finality. The configured node sees ordinary chain
  requests, not the proof's recipient or note-opening data. Nothing in the
  imported JSON is followed as a URL.
- **Send:** enter an address or `@username`, exact GNR amount, fee and optional
  encrypted memo. A separate confirmation window shows the fixed payment
  intent before proof construction, signing and submission. A submission is
  pending until a block confirms it. Username verification uses the paired CLI.
- **Receive:** raw receiving identity and monochrome QR. The QR encodes that
  address by default. An explicit request action creates the experimental v1
  payment URI described below, optionally with an amount. Copying an address
  or request displays a clipboard warning. The wallet never automatically copies seeds.
- **Username:** Receive → Username registers an [on-chain username](../protocol/usernames.md).
  Availability, pending registrations and the price appear as you type; a
  confirmation lists the burned amount and what becomes public before anything is
  sent. Your names show on Receive and Overview, and registrations appear in
  Transactions as Username entries. Send resolves `@name` through your own node and
  shows the receiving address it points to.
- **Mining recovery:** before each start the wallet writes a marker line into
  `genoryn-miner.log`. When the miner exits on its own, the wallet reads that run's
  last lines: the miner reports `genoryn-miner stopped [kind]: reason` for a refused
  admin token, an unreachable node, a different network or a RandomX failure; a run
  without that line crashed. The Mining page explains the cause and, at most three
  times, restarts with portable RandomX (`GENORYN_RANDOMX_PORTABLE=1`, for CPUs or
  security software that refuse the JIT) or half the threads (for allocation
  failures). Executables blocked by security software or missing are named with
  the fix. Administrator rights are never requested.
- **Appearance:** dark by default; the sun/moon button in the top bar or Settings
  switches to light, and the choice is remembered. Text uses the bundled Inter,
  Unbounded (headings and balances) and JetBrains Mono (labels) fonts.
- **GENORYN Lite:** when `genoryn-miner` is not installed next to the wallet, the
  window is titled GENORYN Lite, the Mining page is absent and update checks offer only
  `GENORYN-Lite-Setup-<version>.exe`. The Lite installer and packages contain the
  wallet, `genoryn-wallet-cli`, `genorynd` and `genoryn-cli`.
- **Balance checks:** Review payment and Register username stay disabled while the
  matured balance cannot pay the amount or price plus the fee; the card states both
  numbers. Refused operations are shown in red in their card.
- **Node:** when no node answers at launch, the wallet starts its own mainnet node
  once (`--auto-node off` disables this); it can also be started explicitly. Its
  admin RPC stays on loopback and its P2P port joins the mainnet as
  described in [networking](../protocol/networking.md). Check its identity,
  inspect live height/peers/mempool/difficulty and stop/restart only the process owned by
  this GUI. Logs are in the selected data directory. An independently started
  node is detected but is never killed by the GUI, and a node stopped later is
  never silently restarted. Open recent logs shows the last 64 KiB of
  `genorynd.log` as readable lines in a separate window, with Refresh, Open log
  file and Open folder; a node started outside the wallet has no log there, and
  the window says so. The log viewer reads at most
  the last 64 KiB of the selected node log inside the app. Advanced configuration
  selects an optional strict TOML file before startup; public RPC and peer choices
  in that file remain explicit operator settings.
- **Mining:** explicitly start and stop a CPU miner, with a visible unlocked
  wallet destination and thread count. No mining starts on wallet launch,
  unlock or node startup. Numeric metrics are taken from actual miner JSON
  events and are otherwise marked unavailable. Stop mining before stopping the
  owned node. The CPU selector offers one worker per logical CPU; all workers share
  one 2 GiB RandomX dataset ([ADR 0016](../adr/0016-fast-mode-mining.md)). The miner
  abandons a template within a second once another block takes its height, and
  retries a busy or restarting node instead of exiting. Closing the GUI
  stops/reaps its own miner and node.
- **Remembered wallets:** the wallets that were open are remembered (paths only,
  in `wallet-preferences.json`; never passwords). At launch the last active one
  is preselected and listed under Your wallets; unlocking it can also unlock the
  other remembered wallets that share its password. Wallets with another password
  wait under Locked in the wallet menu. Close this wallet stops remembering it.
- **Network privacy:** the Network page chooses the mode of the node this wallet
  starts: Direct, Hybrid or Tor only (new installations start in Tor only;
  existing ones keep their mode), the Tor engine (built-in, preselected, or the
  user's own Tor with its SOCKS proxy and optional control port) and an optional
  onion service (persisted in `wallet-preferences.json`; invalid settings stop
  the node start with a reason).
  Direct and Hybrid show a privacy warning because peers see this computer's IP.
  From the local node's admin RPC it shows the running mode, Tor proxy and
  control-port state, bootstrap progress, readiness, onion service state and
  address (with Copy), and clearnet/Tor/onion connection counts and warnings, such
  as Tor being unavailable. **Apply and restart node** restarts the owned node with
  the new mode; **Retry Tor now** asks the node to re-check Tor and retry peers.
  In Tor-only mode the wallet refuses remote RPC endpoints before sending anything
  and sends update checks only through Tor. No "anonymous" indicator is shown; the
  page explains that network privacy (IP address) and on-chain privacy (shielded
  notes) are separate. See [network privacy](../network-privacy.md).
- **Network:** show pinned chain ID/genesis and actual public supply RPC values of
  the mainnet. No market price is fabricated. Optional Settings input
  shows a user-entered USD/GNR Reference Value using checked integer arithmetic,
  rounded down to cents. It is separate from payments, fees and consensus.
- **Updates:** at launch (unless disabled in Settings → Updates) the wallet reads
  the public latest-release record of this repository from `api.github.com`.
  A strictly newer published version shows a notice with its release page;
  Settings → Updates shows the installed version, checks on demand and lists the
  release notes. Only this repository's release pages are opened, in the browser.
  Nothing is downloaded in the background or installed automatically.

Repeated wallet actions and lifecycle clicks are disabled while work is
pending. Native subprocess and RPC work runs off the UI thread. A startup
result that arrives after the application closes owns a cleanup guard, so it
cannot silently leave a newly started miner running. Failed CLI calls display
safe error text rather than claiming completion.
Proof-file work is also asynchronous. Clear/Cancel or lock erases held proof
data and invalidates late results; changing the RPC endpoint clears prior
verification. A local save that already began may have completed, and clearing
the view cannot erase clipboard copies or previously saved files. Repeated
actions are disabled until the canceled worker finishes.

## Local JSON-stdio contract

Launch `genoryn-wallet-cli --json-stdio`; send one JSON request through stdin,
close stdin, and receive exactly one JSON response on stdout:

```json
{"ok":true,"result":{"address":"gnrd…"}}
```

or `{ "ok": false, "error": "safe error message" }`. No log lines belong on
stdout. Passwords and phrases must never be echoed, included in errors or put
on argv. The GUI discards CLI stderr and bounds stdout to 16 MiB. Operations
have a ten-minute timeout so proof generation can run without freezing the UI.

Common request fields: `op`, `wallet_path`, `password`, `rpc_url`; a loopback
administrative operation may also include `token_file`, derived from the node
data directory. Do not transmit admin credentials to a non-loopback endpoint.

| Operation | Additional input | Result |
| --- | --- | --- |
| `create` | optional `account` (default 0) | `address`, `seed_phrase`, `account` |
| `restore` | `seed_phrase`, optional `account` | `address`, `account` |
| `open` | none | `address`, `account` |
| `address` | optional `address_index` | `address`, `account`, `address_index` |
| `derive_account` | `account`, `output` (new encrypted file path) | `address`, `account` |
| `estimate_fee` | `priority` (`economy`, `standard`, `priority`) | exact fee quote, policy basis, no inclusion guarantee |
| `sync` | none | `address`, `balance_atomic`, `spendable_atomic`, `height`, `history` |
| `send` | `to`, `amount_atomic`, `fee_atomic`, `memo` | `txid` |
| `register_username` | `username` | `username`, `registered` |
| `resolve_username` | `username` | `username`, `address`, `verified` |
| `export_proof` | `txid`, `action` (zero-based output) | shared `NoteProof` JSON |
| `verify_proof` | `proof` (shared `NoteProof` JSON) | `recipient`, `amount_atomic`, empty `memo` |

Atomic amounts are decimal strings. History entries have `txid`, `direction`,
`amount_atomic`, `height`, `status` and `confirmations`. Username registration is off-chain in
v1, so it does not return a fictional registration transaction ID.

## Security boundaries and remaining work

Sensitive text editors discard undo history each frame, so clearing or locking
cannot restore dismissed inputs with Undo. This includes passwords, recovery
phrases, proof data and payment drafts.

Rust zeroization reduces residual secret buffers, but it cannot guarantee
removal of OS swap, GPU/text-renderer copies, other renderer copies, crash dumps,
clipboard managers or malware. Use a trusted computer. OS credential-store
integration and a dedicated sandboxed wallet service need separate review.
Telemetry is absent; the optional update check sends only an ordinary HTTPS
request for the public release record (through Tor in Tor-only mode). The
clipboard is never cleared automatically: copied addresses, onion addresses and
payment proofs stay readable by other applications and clipboard history until
replaced. Rich memos/invoices, other durable GUI settings,
and full production light-wallet protocols remain later work, not simulated
controls. The UI's localhost health check is not itself consensus validation;
run your own full node to validate the chain.

## Verification

`cargo test -p genoryn-wallet` includes network-privacy argument and policy
tests (remote RPC refused in Tor-only mode, update checks sent to a local SOCKS
stand-in by host name and failing closed when it is down), exact amount validation, recursive
secret-buffer wiping, local binary discovery, owned-process cleanup, late
response rejection after lock/cancel, malformed and oversized proof inputs,
overwrite refusal, locked proof verification, and headless rendering of every screen in both
appearances with a strict grayscale-vertex assertion. These are focused tests,
not substitutes for actual desktop interaction, end-to-end payment testing,
Windows QA or external security review.

## Experimental payment URI v1

The GUI can generate and import `genoryn:<receiving_identity>?v=1&network=mainnet`
and an optional `&amount=1.000000000001`. This is an experimental application
format, not a consensus identifier or frozen production standard. Imports only
populate a draft; they never connect to an arbitrary URL, unlock a wallet,
start mining, sign, or broadcast. The separate review/confirmation remains
mandatory. The OS is not automatically registered as a URI handler.

The address is verified with the shared wallet-core Bech32m and receiver parser.
The expected network must match both the address and explicit network field.
Version and network are required. Only canonical exact decimal amounts up to
the shared transaction limit are admitted: no floating point, exponent notation,
signs, zero, leading zeroes, redundant trailing fractional zeroes, duplicate
parameters, unknown fields, percent escapes, fragments or embedded whitespace.
Amounts are optional but must be positive when present. Plaintext memos and
invoice descriptions are deliberately not accepted in the URI. Share a payment
request only with intended recipients; its address and requested amount are
visible to anyone holding the URI.

Unit tests exercise valid roundtrips, version/network separation, malformed
receivers, duplicate parameters, canonical decimal enforcement and ambiguous
encodings. None of the parsing functions has network or sending side effects.
