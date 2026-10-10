# Start with GENORYN

GENORYN runs on Mainnet 1.0 (`genoryn-mainnet-1.0`), launched with release 1.0.0. The earlier
network (`genoryn-mainnet-v1`) is retired; its coins do not exist on 1.0.
The software has not been independently audited: mine or hold only what you can
afford to lose. No VPS, exchange, domain account or cloud key is needed; your
computer runs its own full node.

## Install on Windows

There are two installers. **GENORYN** (`GENORYN-Setup-<version>.exe`) is the wallet,
its own full node and the CPU miner. **GENORYN Lite**
(`GENORYN-Lite-Setup-<version>.exe`) is the wallet and full node without mining, for
people who only send and receive. Both share the wallets in `%USERPROFILE%\.genoryn`.

Download the installer you want from
[the latest release](https://github.com/genoryn/genoryn/releases/latest) and run it.
It installs for your user account without administrator rights, adds GENORYN to the
Start menu (and the desktop if you choose), and appears under Settings → Apps for
uninstalling.

GENORYN is not code-signed yet. Windows 11's Smart App Control blocks unsigned
programs and has no per-app exception, so with it on, the installer stops before
installing anything and explains this (otherwise Windows would report "An
Application Control policy has blocked this file", code 4551). To use GENORYN,
turn it off under Windows Security → App & browser control → Smart App Control
settings, then run the installer again. On some Windows versions Smart App
Control cannot be turned back on without reinstalling Windows; decide accordingly.
SmartScreen's "Windows protected your PC" prompt is separate: choose More info →
Run anyway.

If the Microsoft Visual C++ runtime is missing, the installer offers
to download it from Microsoft. To update, run the newer installer: it closes the
running wallet, replaces the program and keeps your settings. Wallets and chain data
live in `%USERPROFILE%\.genoryn` and are never touched by installing, updating or
uninstalling. Check the `.sha256` file next to the installer before running it.

## Prebuilt packages

The newest Windows and Linux packages are attached to
[GitHub Releases](https://github.com/genoryn/genoryn/releases/latest), built by
the release workflow from the tagged commit. The wallet tells you when a newer
release is published (Settings → Updates); on Windows, Download update fetches the
new installer.

Use a development package only after its exact commit's workflow and final
archive verification pass. The [native notice inventory](security/dependency-licenses.md)
now has complete mechanical coverage; older archives produced before that
correction remain historical test artifacts. Build from source while the final
candidate's verification is in progress.

After the exact commit's Windows/Linux verification workflow succeeds, download
its matching `genoryn-<OS>-<commit>` artifact from the repository's Actions
page. Choose only a successful run for the commit referenced in the release
evidence. Compare the archive SHA-256 with the published checksum. Development
builds are explicitly unsigned; they are not an authenticated production release.

The Actions download is a wrapper ZIP containing evidence and a `dist` folder.
Extract it, then extract the platform ZIP inside `dist` to a normal user-owned
directory. Keep all executables together:
- Windows: double-click `Start-Wallet.cmd`.
- Linux desktop: execute `./start-wallet`.

From the extracted Actions wrapper, verify the inner archive before opening it.
On Linux:

```sh
cd dist
sha256sum -c genoryn-*.zip.sha256
```

On Windows, in PowerShell:

```powershell
Set-Location .\dist
$archive = Get-ChildItem genoryn-*.zip | Select-Object -First 1
$expected = ((Get-Content ($archive.FullName + ".sha256") -Raw).Trim() -split "\s+")[0]
$actual = (Get-FileHash $archive.FullName -Algorithm SHA256).Hash
if ($actual -ne $expected) { throw "Archive checksum mismatch" }
```

A matching checksum detects changed bytes; it does not replace an authenticated
publisher signature or the exact-source verification evidence.

The wrapper and direct native executable use the same default wallet under
your home directory's `.genoryn/mainnet-1.0` folder (`wallet.gnr`), unless you explicitly
choose another path. They never silently create separate profiles by launch method.

The package must keep all seven executable files together. The wallet starts
no mining automatically. These are large, unoptimized development packages:
allow about 2 GB for the extracted application, plus download and chain storage.

### Platform requirements

The Linux CI package targets **Ubuntu 24.04 x86-64**. Its actual GUI binary uses
GLIBC_2.39; it is not compatible with Ubuntu 22.04 or Debian 12's older glibc.
Other distributions are not certified by the filename `linux-x86_64`. Build from
source on your system instead of replacing its system C library. A graphical
X11/Wayland session and OpenGL support are required. On Ubuntu 24.04, install
the runtime libraries if absent:

```sh
sudo apt-get install libstdc++6 libgcc-s1 libx11-6 libxi6 libxrandr2 libxcursor1 \
  libxinerama1 libgl1 libegl1 libwayland-client0 libxkbcommon0 libxkbcommon-x11-0
```

Software OpenGL rendering is possible for testing. Windows CI targets x64 on
Windows Server 2022; Windows uses Direct3D 12, including a compatible software
adapter. Other Windows versions require their own validation. The wallet and
daemon require the **Microsoft Visual C++ v14 x64 runtime** (`MSVCP140.dll`,
`VCRUNTIME140.dll` and `VCRUNTIME140_1.dll`). If absent, install the latest
[official x64 runtime](https://aka.ms/vc14/vc_redist.x64.exe), then launch again.
Microsoft documents its [supported runtime versions and platforms](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist?view=msvc-170).
`Start-Wallet.cmd` checks these runtime files and displays that link if missing;
it does not download, install or bundle Microsoft's DLLs. The runtime must be
at least as recent as the MSVC build tools; use Microsoft's latest supported v14
x64 package rather than assuming an old installed v14 version is sufficient.

## First wallet

1. Choose a new wallet path and a strong, unique password.
2. Create the wallet. Write the 24-word recovery phrase down offline. Confirm
   that you saved it before leaving the recovery screen. Do not screenshot it.
3. The wallet starts its local node by itself when none is running, and the node
   joins the GENORYN mainnet. An already-running independent node is displayed as
   external and will not be killed by the wallet. Windows may ask once whether
   `genorynd` may accept network connections; outbound connections work either way.
4. In Receive, copy your `gnr1…` receiving address or its versioned QR/URI.
5. To mine, open Mining, choose CPU threads and start. The miner uses one 2 GiB
   RandomX dataset for all threads; it is ready about a minute after the start, and
   until then the miner hashes in the slower light mode. If the miner stops, the Mining page shows the reason in
   plain words and, when it helps, restarts it in portable RandomX mode or with
   fewer threads. Mining never needs administrator rights; if security software
   quarantines `genoryn-miner.exe`, allow it there.
6. Newly mined coins mature after 60 blocks (about an hour). The balance
   refreshes by itself as blocks arrive; Scan chain rescans immediately.
7. To pay someone, enter their `gnr1…` address or `@username`, the exact amount
   and an optional encrypted memo. Review the preview and confirm. A mempool
   acknowledgment is not finality.
8. Lock before stepping away. Closing the application stops and reaps only the
   node/miner processes it started. Wallet files remain encrypted on disk.

For two wallets, use separate wallet files and passwords; the wallet can keep up
to eight open at once. Usernames are registered on the chain, first come first
served and permanent; the registration price is burned. See
[usernames](protocol/usernames.md). The light/dark switch is the sun or moon
button in the top bar; dark is the default.

## Hide your IP address from peers with Tor

A new installation starts in **Tor only** with the built-in Tor; an earlier
installation keeps its mode (Direct, where every peer sees your computer's
public IP address). Your payments stay shielded either way; Tor only changes who
can see your IP. To switch an earlier installation to Tor:

1. In the wallet, open **Network → Network privacy** and choose **Tor only**.
   **Built-in Tor** is preselected: nothing to install or keep open. Optionally
   tick **Accept peers through a Tor onion service**.
2. To use a Tor you already run instead, choose **My own Tor** and enter its
   SOCKS proxy: `127.0.0.1:9050` for the Tor daemon or `127.0.0.1:9150` for Tor
   Browser, plus optionally its control port (`9051` / `9151`) for bootstrap
   progress and an onion service. GENORYN never installs or starts that Tor.
3. Click **Apply and restart node** (or restart the wallet). The Network page then
   shows Tor status, bootstrap progress and connection types. The first start
   with built-in Tor downloads Tor's directory information and takes a little
   longer.

If Tor loses its connection, a Tor-only node pauses networking instead of
connecting directly, and payments you send wait and go out once Tor works
again; **Retry Tor now** checks immediately. Read
[network privacy and Tor](network-privacy.md) for what Tor does and does not
protect.

## Build from source

Install Rust 1.92.0, Git, CMake and a C/C++ compiler. Windows requires the Visual
Studio C++ desktop build workload. On Ubuntu:

```sh
sudo apt-get install build-essential cmake libx11-dev libxi-dev libxrandr-dev \
  libxcursor-dev libxinerama-dev libgl1-mesa-dev libwayland-dev libxkbcommon-dev \
  libxkbcommon-x11-dev
cargo build --locked --workspace
./target/debug/genoryn-wallet --bin-dir ./target/debug
```

On Windows run the same Cargo command from a Developer PowerShell, then
`.\target\debug\genoryn-wallet.exe --bin-dir .\target\debug`.

## Recovery

Create a new wallet file using Restore and the same 24 words; do not overwrite
the original. Set a new local file password. Synchronize the same network and
scan the full chain. Spend/view keys, account-zero receiver and username signing
identity are derived from the seed. Local owner pins, labels and off-chain
registry metadata need their own backups. No maintainer can recover a lost seed.

### Back up and restore local metadata

For the default profile, the encrypted wallet is
`$HOME/.genoryn/mainnet-1.0/wallet.gnr` on Linux or
`%USERPROFILE%\.genoryn\mainnet-1.0\wallet.gnr` on Windows. Its contact-owner pins
are beside it as `wallet.username-pins.json`. With a custom wallet filename,
replace its final extension with `.username-pins.json` (for example,
`alice.wallet` uses `alice.username-pins.json`). Pins are trust decisions;
losing them means first-use contact ownership must be verified again.

1. Stop mining, stop your owned node and close the wallet. If a node was started
   separately, stop that node separately before copying its database.
2. Copy the encrypted wallet and its pin file, if present, to a private backup.
   Keep the seed offline and the password separate. Pins reveal contact metadata.
3. To preserve local username registrations, back up the stopped node's entire
   data directory. The registry is `usernames.sqlite`; legacy `usernames.json`,
   when present, is a preserved migration backup. Never copy only an actively
   written SQLite main file while omitting its WAL. The directory also contains
   the private `admin.token`, so protect this backup and do not publish it.
4. Restore copies into a separate directory with the applications stopped.
   Select the restored wallet and data paths explicitly, unlock and scan the
   same network. Keep the original backup until balances and pins are checked.

A seed restores financial keys and receiving identity; it does not restore
off-chain labels, pins or a resolver's database. Chain history can be obtained
again from independently validating peers, when those peers are available.

See [CLI and development-lab instructions](testing/devnet.md), [GUI controls](wallet/gui.md)
and [security limitations](security/threat-model.md).
