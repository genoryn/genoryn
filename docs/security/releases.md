# Verify a release before running it

Development artifacts include an exact source commit, compiler version,
dependency inventory, third-party license texts, SHA256SUMS and a JSON release
manifest. They are labeled **unsigned development builds**. A checksum establishes
byte equality with its source, not the publisher's identity.

## Check a development archive

First consult the [dependency-notice evidence](dependency-licenses.md). Do not
interpret these commands as approving an older incomplete distributable.
After extracting the matching successful Actions wrapper ZIP, open its `dist`
directory. Linux:

```sh
sha256sum --check genoryn-*.zip.sha256
# Continue only if every listed archive reports OK.
unzip genoryn-*-mainnet-linux-x86_64.zip
cd genoryn-*-mainnet-linux-x86_64
sha256sum --check SHA256SUMS
```

Windows PowerShell, from the wrapper's `dist` directory:

```powershell
$archives = @(Get-ChildItem -File 'genoryn-*.zip')
if ($archives.Count -ne 1) { throw 'Use a folder containing exactly one platform archive' }
$archive = $archives[0]
$expected = ((Get-Content -LiteralPath ($archive.FullName + '.sha256') -Raw).Trim() -split '\s+')[0]
if ($expected -notmatch '^[0-9a-fA-F]{64}$') { throw 'Malformed checksum file' }
$actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $archive.FullName).Hash
if ($actual -ne $expected) { throw 'Archive checksum mismatch; do not run it' }
Expand-Archive -LiteralPath $archive.FullName -DestinationPath '.\verified-package'
```

Also inspect `release-manifest.json`: the `commit` must equal the exact Actions
run you selected, `working_tree_dirty` must be false, and the platform must match
your computer. Archive checksums from the same download detect corruption; they
do not protect against a compromised publisher or replace production signatures.

## GitHub Releases and update notices

Each `vX.Y.Z` tag runs [the release workflow](https://github.com/genoryn/genoryn/blob/main/.github/workflows/release.yml),
which builds the Windows and Linux packages from that exact commit, builds the
Windows installer from the verified Windows package
([`packaging/windows/genoryn.iss`](https://github.com/genoryn/genoryn/blob/main/packaging/windows/genoryn.iss)),
and attaches them, with their `.sha256` files, to the matching
[GitHub Release](https://github.com/genoryn/genoryn/releases). The single
version source is `[workspace.package] version` in the root `Cargo.toml`.

The wallet reads the public latest-release record from `api.github.com` when it
opens (Settings → Updates can turn this off or check on demand). When a newer
version is published it shows a notice; on Windows its Download update button
fetches that release's `GENORYN-Setup-<version>.exe` in the browser (only from this
repository's release downloads, for exactly that version), elsewhere it opens the
release page.
Nothing about the wallet is sent, nothing is downloaded in the background and
nothing is installed or executed automatically. Verify every download as above.

## Production signature boundary

The wallet core includes Ed25519 verification of release metadata against an
explicit independently trusted public key, followed by streaming file checksum
and path validation. The signed bytes are the domain
`GENORYN/release-manifest/v1` followed by a NUL and the exact manifest bytes.
Whitespace changes invalidate the signature. The public key must not be taken
from the same unauthenticated download as the signature.

No official production signing identity exists in this development release, and
no key is generated or saved by the build. Production signing is a separate
maintainer-controlled ceremony; future multi-maintainer authorization requires
its own reviewed policy. No background updater downloads and runs unsigned code.
The verifier never executes a file or installs an update.

```sh
genoryn-cli verify-release --manifest release-manifest.json \
  --signature release-manifest.sig --public-key TRUSTED_32_BYTE_HEX_KEY \
  --directory ./extracted-package
```

The signature file contains the 64-byte Ed25519 signature as hexadecimal. An
unsigned development package cannot pass this identity check; do not substitute
a key from an unverified channel or a publicly known test fixture.

Package already-verified binaries with:

```sh
python3 scripts/release/package.py --bin-dir target/debug --output dist
```

These packages are development builds. Reproducible-build work includes pinned
compiler/dependencies, canonical vectors and provenance, but bit-for-bit native
binary reproducibility across independent builders has not been established.
