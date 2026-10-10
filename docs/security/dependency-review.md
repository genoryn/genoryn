# Dependency review

Checked 2026-10-07 with cargo-audit 0.22.2 against the RustSec advisory database.

The automated audit job for `39f6a09b449c3db5b29df22a1ad5794ec9daacd0`
passed on 2026-10-08 with **zero known vulnerabilities**, using RustSec database
commit `b8a1a33e246a0a9a3b5f377248c41a503defec74` (1,294 advisories).
The [downloaded machine-readable report](../testing/evidence/dependency-audit-39f6.json)
and [exact workflow](https://github.com/deepdrogo/GENORYN/actions/runs/37705572395)
retain the evidence. Other jobs in that workflow have separately recorded
packaging failures; the audit pass is not a whole-release pass.

The first scan identified RUSTSEC-2026-0119 in the initial DNS resolver dependency,
an encoding CPU-exhaustion issue in hickory-proto before 0.26.1. The resolver was
upgraded to the maintained 0.26.1+ family and its current builder API. The final
scan must show no known vulnerable packages before local release sign-off.

Informational maintenance warnings remain distinct from vulnerabilities:

- bincode 1.3.3 is frozen/unmaintained (RUSTSEC-2025-0141). Its deterministic
  fixed-width format is pinned, inputs are length-capped, trailing/noncanonical
  data is rejected, and parser tests cover hostile lengths. Replacing it changes
  consensus serialization and requires a versioned migration plus vectors.
- paste and ttf-parser are transitive GUI dependencies with maintenance warnings.
  The wallet uses embedded default fonts and does not load peer-supplied fonts.
  They need upstream tracking before a production release.

These are documented devnet risks, not an audit waiver. Cargo-audit finds known
advisories only; it cannot prove code safe or cryptographic composition correct.
The explicit corrected Orchard circuit selection is separately reviewed in
[ADR 0001](../adr/0001-shielded-protocol.md).

## Wallet additions in 0.2.0

- `winresource` (build-time, Windows hosts only) compiles the icon and version
  resource with the Windows SDK `rc.exe`; it adds no runtime code.
- `rfd` (Windows target only) opens the system file and folder dialogs used to
  choose wallet files. It runs no network or background code.
- `reqwest`, already used by `genoryn-connect`, is now also a direct wallet
  dependency for the optional update check, which reads one public release record
  over HTTPS with a 10-second timeout and a 1 MB response bound.

The lockfile gained only `winresource`, `rfd`, `block2` (macOS-only, not built for
Windows or Linux) and a second `toml` version used by `winresource`. These need the
same advisory scan as every other dependency before sign-off.

## Network-privacy additions (2026-10-10)

- `tokio-socks 0.5.3` (MIT): SOCKS5 client for Tor connections, also enabled
  through `reqwest`'s `socks` feature for `socks5h` auxiliary HTTPS. Domain
  targets are sent to the proxy (ATYP 3); the proxy is always given as a socket
  address, so the crate performs no DNS lookups in GENORYN's use. Its Tor
  extended reply codes surface as a generic error, which GENORYN reports as an
  unavailable onion service.
- `sha3 0.10.9` and `keccak 0.1.6` (MIT/Apache-2.0, RustCrypto): SHA3-256 for
  the v3 onion address checksum only.
- `hmac`, `data-encoding` and `ed25519-dalek` were already in the lockfile and
  are now direct dependencies of the node (SAFECOOKIE HMAC-SHA256, base32 onion
  names, onion key point validation).

### Built-in Tor (Arti), same branch

`arti-client` 0.47 with `tor-rtcompat`, `tor-hsservice`, `tor-cell`,
`tor-proto` and `safelog` (MIT/Apache-2.0, the Tor Project), default features
off; rustls with ring, static SQLite sharing the node's `rusqlite` 0.37,
onion-service client and service, vanguards. About 200 crates enter the graph
(823 lockfile packages in total). `saturating-time` 0.5.0 is vendored with a
termination fix ([ADR 0015](../adr/0015-built-in-tor-client.md)).

`cargo audit` (0.22.2, run 2026-10-10 on this lockfile) reports one
vulnerability and four already-known unmaintained warnings:

- **RUSTSEC-2023-0071, `rsa` 0.9.10** (via `tor-llcrypto`, `tor-key-forge`,
  `ssh-key-fork-arti`): the Marvin timing attack recovers an RSA *private* key
  from decryption/signing timing. No fixed release exists. A Tor client and a
  v3 onion service only verify RSA signatures (directory certificates, legacy
  relay identities); GENORYN creates no RSA key and onion-service keys are
  ed25519/x25519. Assessed as not reachable; the maintainer approved an
  explicit ignore with this justification on 2026-10-10
  (`.cargo/audit.toml`). Revisit when Arti moves off `rsa` 0.9.
- Unmaintained: `bincode` 1.3.3 and 2.0.1, `paste` 1.0.15, `ttf-parser` 0.25.1
  (unchanged from earlier reviews except that `bincode` 2 arrives with Arti).

55 crates ship no license text in their archives (47 Arti crates, plus
`amplify_num`, `amplify_syn`, `asn1-rs-impl`, `cookie-factory`, `defmt-parser`,
`derive-deftly-macros`, `priority-queue` and `void`). Their upstream license
files, fetched at the exact commit each crate was published from, are pinned
with SHA-256 in `third-party/license-supplements` (`void` 1.0.2 shipped without
any; its record uses the author's MIT text merged right after that release).
`priority-queue` is `LGPL-3.0-or-later OR MPL-2.0`; GENORYN uses it under
MPL-2.0, both texts are included, and its unmodified source is on crates.io.

## RandomX binding source review (2026-10-08)

A public report and independent inspection found unchecked nullable VM creation
and immutable-derived native output pointers in the pinned `randomx-rs 1.6.0`
source. A clean automated advisory scan did not establish the absence of these
defects. [ADR 0012](../adr/0012-randomx-binding-safety.md) records the minimal
vendored Rust wrapper corrections, exact source provenance and remaining native
runtime limits. Native RandomX code and consensus inputs are unchanged. Both
the main and fuzz workspaces use the reviewed path patch; future advisory scans
must be accompanied by continued manual review of this downstream dependency.
