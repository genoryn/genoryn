#!/usr/bin/env bash
# Hive OS runs this (in its screen session) to start the miner. It starts only a miner
# whose bytes match the checksum shipped in this package, only one at a time, and only
# with the settings h-config.sh checked.

DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
cd "$DIR" || exit 1
# shellcheck source=/dev/null
. "$DIR/h-manifest.conf"
LOG="$CUSTOM_LOG_BASENAME.log"
mkdir -p "$(dirname "$LOG")"

say() { echo "genoryn: $*" | tee -a "$LOG" >&2; }

# A start that cannot work waits a minute before it ends, so Hive OS's restarts stay slow.
fail() {
	say "$*"
	sleep "${GENORYN_FAIL_WAIT:-60}"
	exit 1
}

[[ -r $CUSTOM_CONFIG_FILENAME ]] || fail "no checked config: fix the Flight Sheet (see the lines above, or run h-config.sh)"
NODE=""
ADDRESS=""
ARGS=()
while IFS= read -r line; do
	case $line in
	NODE=*) NODE=${line#NODE=} ;;
	ADDRESS=*) ADDRESS=${line#ADDRESS=} ;;
	ARG=*) ARGS+=("${line#ARG=}") ;;
	esac
done <"$CUSTOM_CONFIG_FILENAME"
[[ -n $NODE && -n $ADDRESS ]] || fail "the config is incomplete: run h-config.sh"

# The miner as released: its SHA-256 is in krypton-miner.sha256, and that file is covered
# by the release's signed SHA256SUMS.
[[ -x ./krypton-miner && -r ./krypton-miner.sha256 ]] || fail "krypton-miner or its checksum is missing: reinstall the package"
want=$(cut -d' ' -f1 krypton-miner.sha256)
have=$(sha256sum krypton-miner | cut -d' ' -f1)
[[ ${#want} -eq 64 && $want == "$have" ]] || fail "krypton-miner does not match its checksum: refusing to run it (reinstall the package from the official release)"
ver=$(./krypton-miner --version 2>/dev/null | awk '{print $2}')
[[ $ver == "$CUSTOM_VERSION" ]] || fail "krypton-miner is version '$ver', the package is $CUSTOM_VERSION: reinstall the package"

# One miner at a time on this rig.
exec 9>"${GENORYN_LOCK:-/run/genoryn-miner.lock}" || fail "cannot open the lock file"
flock -n 9 || fail "another Genoryn miner is already running on this rig"

say "starting krypton-miner $ver, node $NODE, address ${ADDRESS:0:12}...${ADDRESS: -6}"
export KRYPTON_MINER_ADDRESS=$ADDRESS
./krypton-miner --node "$NODE" --json "${ARGS[@]}" 2>&1 | tee -a "$LOG"
