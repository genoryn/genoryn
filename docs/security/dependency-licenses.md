# Dependency notices and native package inventory

The mechanical notice inventory is complete for the supported native Linux and
Windows target graphs. This is packaging evidence, not legal clearance, a
cryptographic audit, or permission to launch mainnet. Exact-commit CI and final
archive inspection remain separate release gates. The clean `95d1a5a` baseline
has now passed both ([archive evidence](../testing/evidence/native-artifacts-95d1a5a.json));
subsequent source changes still require their own exact verification.

## What the package contains

The packager asks Cargo for the compiler's native target graph and selects its
resolved package IDs. It records the target triple and conservatively includes
build/test dependencies too. Inactive Apple, Android and UEFI dependencies from
the broader lockfile are not represented as shipped Linux/Windows components.
A future platform release requires its own inventory and runtime verification.

Every selected third-party package, including vendored path dependencies, must
have collected license/notice text. The
collector preserves nested paths, including bundled RandomX and embedded-font
attribution. Empty marker files, checker scripts and archived symlink-placeholder
text do not count as notice coverage. Unknown gaps fail packaging. Upstream supplements omitted from some
crate archives are bound to exact package versions, source revisions, URLs and
SHA-256 values in the repository's `third-party/license-supplements/manifest.json`.
The package contains their provenance as `DEPENDENCY-LICENSE-SOURCES.json`.
The reviewed RandomX binding patch additionally records exact original and
patched source hashes in `VENDORED-SOURCE-PROVENANCE.json`; packaging rejects
missing, changed or extra vendored files. Native RandomX sources remain unchanged.

Two exact-version archives, `bitcoin_hashes 0.14.101` and `hexf-parse 0.2.1`,
explicitly declare **CC0-1.0** but omit a license file. They receive the verbatim
standard CC0 legal text from the already Cargo.lock-pinned `bip39 2.2.2` source
archive. The manifest labels this `canonical-license-text`, records its original
source/checksum and the two declared-license bases, and retains authors and
repositories in `DEPENDENCIES.json`. It does **not** claim that a package-specific
notice was downloaded for either crate or synthesize a copyright attribution.
The helper rejects a canonical text whose SPDX identifier differs from the
package declaration; this is not a generic fallback for unknown licenses.

## Recorded validation

The [native graph check](../testing/evidence/native-license-inventory.json) found
436 registry packages and 805 notice files for Linux, and 425 packages and 791
notice files for Windows, with no missing entries. These are conservative Cargo
graph inventories, not exact linker symbol counts.

With the minimal RandomX binding safety patch, these same totals consist of
435 registry packages plus one vendored dependency on Linux and 424 plus one on
Windows. Both binding/native RandomX notices remain present. The fuzz workspace
also explicitly selects the same patched binding.

An actual local Linux package passed all 876 manifest hash/size checks, with no
missing notices or project-document links. The full wgpu-hal MIT text, parent
egui-code attribution plus all four font notices, and both RandomX notices were
explicitly checked. Pointer-only files and checker scripts are excluded. This
was a dirty-worktree validation archive, so it is not the final distributable;
its [limited report](../testing/evidence/native-package-notices.json) says so.
An independent review reproduced both native inventories and closed the two
substantive-notice findings. Final CI packages must identify a clean exact commit
and pass the same checks.

The [historical unfiltered gap snapshot](license-inventory-pending.json) is kept
for traceability. It describes the earlier collector, not current native package
coverage. Presence and checksum checks cannot determine every legal obligation;
new dependency versions and platforms still need review.
