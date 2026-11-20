#!/usr/bin/env bash
# Runs tests/test.sh in Ubuntu 20.04 (the oldest base Hive OS uses), with jq and shellcheck.
set -euo pipefail
abs() { (cd "$1" && { pwd -W 2>/dev/null || pwd; }); }
export MSYS_NO_PATHCONV=1
SRC=$(abs "$(dirname "${BASH_SOURCE[0]}")/..")
docker run --rm -v "$SRC:/src/integrations/hiveos:ro" ubuntu:20.04 bash -c '
	export DEBIAN_FRONTEND=noninteractive
	apt-get update -qq && apt-get install -y -qq jq shellcheck >/dev/null
	bash /src/integrations/hiveos/tests/test.sh
'
