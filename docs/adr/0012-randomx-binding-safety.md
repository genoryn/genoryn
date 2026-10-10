# ADR 0012: minimal vendored RandomX binding safety correction

Independent inspection of the exact `randomx-rs 1.6.0` registry archive confirmed
two Rust FFI contract defects: the nullable native VM constructor was wrapped in
`Ok` without a null check, and native hash output was written through a pointer
derived from an immutable Rust array. A [public October 4 advisory](https://zecnero.org/security/2026-10-04-randomx)
describes equivalent defects in that upstream version. The official upstream
file history inspected on October 8 still had no later fix for these paths.

Vendor the exact locked archive and make minimal wrapper safety corrections: reject
a null constructor result before creating an owning VM, guard its destructor,
use mutable output arrays/`as_mut_ptr`, and pass input/key pointers as const.
Keep every native RandomX source input unchanged. No algorithm parameters,
proof system, hash bytes, difficulty or consensus rule changes. Record the
original archive SHA-256, upstream revision, per-file hashes and precise patch.
The separate Rust build-orchestration correction below isolates generated output;
the native C/C++/CMake inputs remain unchanged. Preserve upstream licenses.
Third-party audit PDFs are linked upstream rather
than copied or represented as an audit of GENORYN.

The binding remains a third-party native dependency containing unsafe code.
This is an explicit downstream safety patch, not a new cryptographic primitive,
new upstream release or claim of inherited audit. A pure null-pointer guard test
must not invoke native failure paths; valid-input differential/golden hashes and
complete native integration remain mandatory on Linux and Windows.

The correction permits fallback for construction errors actually returned by the
binding. The native library also has void initialization calls and unchecked
page-protection return values; it cannot promise graceful recovery from every
OS JIT restriction. Restricted hosts can set `GENORYN_RANDOMX_PORTABLE=1` to force portable
execution. The pure flag-selector test covers this override.
No OS security policy should be relaxed to make a test or miner run.

## Native build output isolation

The first Windows build passed the Rust/PoW tests but failed the strict source
hash gate: upstream CMake regenerates `src/asm/configuration.asm` inside its
native source directory for MSVC. The build script now copies the exact native
tree, in sorted order, into a fresh Cargo `OUT_DIR` child before invoking the
unchanged native build. Generated assembly stays in that build copy. Only the
Rust build integration changes; native algorithms, parameters and generators do
not. Packaging continues to reject a changed pinned source tree.

The upstream `RANDOMX_DIR` source override is rejected, so an environment variable
cannot silently replace the pinned consensus implementation. Set no such variable
when building GENORYN. The exact build-script patch is separately recorded in
the vendored provenance. Both platform jobs must verify source hashes after the
build; generated files are not excused from the original source check.
