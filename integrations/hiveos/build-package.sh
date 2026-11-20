#!/usr/bin/env bash
# Builds the Hive OS package genoryn-<version>.tar.gz from this repository:
#
#   integrations/hiveos/build-package.sh <version> <out-dir>
#
# Everything happens inside Ubuntu 20.04 (glibc 2.31), so the miner runs on every Hive OS
# image from 20.04 on; release builds made on newer systems would not start there. CUDA is
# loaded from the NVIDIA driver at run time and OpenCL from the system's ICD loader, as in
# the other Linux packages. The package carries the miner's SHA-256, which h-run.sh checks
# before every start; the package itself goes into the release's signed SHA256SUMS.
# Needs only Docker and bash on the machine that runs it.
set -euo pipefail
VER=${1:?version, e.g. 1.0.47}
OUT=${2:?output directory}
[[ $VER =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "bad version: $VER" >&2; exit 1; }
mkdir -p "$OUT"
# Absolute paths Docker understands (on Windows' Git Bash: C:/..., and no path rewriting).
abs() { (cd "$1" && { pwd -W 2>/dev/null || pwd; }); }
export MSYS_NO_PATHCONV=1
OUT=$(abs "$OUT")
REPO=$(abs "$(dirname "${BASH_SOURCE[0]}")/../..")

docker run --rm -e VER="$VER" -v "$REPO:/src:ro" -v "$OUT:/out" ubuntu:20.04 bash -euo pipefail -c '
	export DEBIAN_FRONTEND=noninteractive
	apt-get update -qq && apt-get install -y -qq curl ca-certificates build-essential pkg-config ocl-icd-opencl-dev >/dev/null
	curl -sSf https://sh.rustup.rs | sh -s -- -y -q --profile minimal >/dev/null
	. "$HOME/.cargo/env"
	mkdir -p /build && cd /build
	tar -C /src --exclude=./target --exclude=./website/dist -cf - . | tar -xf -
	cargo build --release --locked -p krypton-miner 2>&1 | tail -n 2
	PKG=/pkg/genoryn
	mkdir -p "$PKG"
	for f in h-manifest.conf h-config.sh h-run.sh h-stats.sh genoryn-check.sh; do cp "integrations/hiveos/genoryn/$f" "$PKG/"; done
	cp integrations/hiveos/README.md LICENSE-MIT LICENSE-APACHE target/release/krypton-miner "$PKG/"
	sed -i "s/^CUSTOM_VERSION=.*/CUSTOM_VERSION=$VER/" "$PKG/h-manifest.conf"
	got=$("$PKG/krypton-miner" --version | awk "{print \$2}")
	[ "$got" = "$VER" ] || { echo "the miner says it is $got, not $VER" >&2; exit 1; }
	(cd "$PKG" && sha256sum krypton-miner >krypton-miner.sha256)
	chmod 755 "$PKG"/*.sh "$PKG/krypton-miner"
	chmod 644 "$PKG"/h-manifest.conf "$PKG"/krypton-miner.sha256 "$PKG"/README.md "$PKG"/LICENSE-*
	ldd "$PKG/krypton-miner" | grep -o "GLIBC_[0-9.]*" | sort -uV | tail -n 1 || true
	objdump -T "$PKG/krypton-miner" | grep -o "GLIBC_[0-9.]*" | sort -uV | tail -n 1
	tar --owner=0 --group=0 --sort=name --mtime="@0" -czf "/out/genoryn-$VER.tar.gz" -C /pkg genoryn
	cd /out && sha256sum "genoryn-$VER.tar.gz"
'
