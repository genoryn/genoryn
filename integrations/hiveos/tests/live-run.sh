#!/usr/bin/env bash
# Runs tests/live.sh in Ubuntu 20.04 with this repository and a built package.
#   integrations/hiveos/tests/live-run.sh <dir with genoryn-<version>.tar.gz>
set -euo pipefail
abs() { (cd "$1" && { pwd -W 2>/dev/null || pwd; }); }
export MSYS_NO_PATHCONV=1
REPO=$(abs "$(dirname "${BASH_SOURCE[0]}")/../../..")
PKG=$(abs "${1:?directory with the package}")
docker run --rm --memory 8g -v "$REPO:/src:ro" -v "$PKG:/pkg:ro" ubuntu:20.04 bash /src/integrations/hiveos/tests/live.sh
