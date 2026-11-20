#!/usr/bin/env bash
# Tests of the Hive OS package, in a Linux container laid out like a Hive OS rig
# (/hive/miners/custom/genoryn, /var/log/miner/custom). A stand-in miner prints what the
# real one prints, so every check runs in seconds; tests/live.sh runs the real package
# against a devnet node.
#
#   integrations/hiveos/tests/run.sh      (starts this in Docker)
set -uo pipefail
SRC=${SRC:-/src/integrations/hiveos}
PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); echo "ok   $*"; }
bad() { FAIL=$((FAIL + 1)); echo "FAIL $*"; }
check() { if eval "$1"; then ok "$2"; else bad "$2"; fi; }
# A config that must be accepted, then checked.
accepted() { # template url extra condition name
	if conf "$1" "$2" "$3"; then check "$4" "$5"; else bad "$5 (h-config.sh refused it)"; fi
}

M=/hive/miners/custom/genoryn
rm -rf "$M" /var/log/miner/custom && mkdir -p "$M" /var/log/miner/custom
cp "$SRC"/genoryn/* "$M/"
chmod 755 "$M"/*.sh

# Shell lint: every script, as Hive OS runs them.
if command -v shellcheck >/dev/null; then
	if shellcheck -x -P "$M" -s bash "$M"/*.sh "$SRC"/build-package.sh; then ok "shellcheck finds nothing"; else bad "shellcheck"; fi
fi

SHORT=gnrs1$(printf 'a%.0s' {1..52})
FULL=gnr1$(printf 'b%.0s' {1..2600})
SEED="abandon ability able about above absent absorb abstract absurd abuse access accident account accuse achieve acid acoustic acquire across act action actor actress actual"
export CUSTOM_CONFIG_FILENAME=$M/genoryn.conf
conf() { # template url extra -> exit status of h-config
	CUSTOM_TEMPLATE=$1 CUSTOM_URL=$2 CUSTOM_USER_CONFIG=$3 bash "$M/h-config.sh" 2>/dev/null
}

# h-config.sh: what is accepted, and what never is.
accepted "$SHORT" "" "" "grep -qx 'NODE=https://genoryn.io' $M/genoryn.conf && grep -qx 'ADDRESS=$SHORT' $M/genoryn.conf && ! grep -q '^ARG=' $M/genoryn.conf" "a short address and no pool URL: mines through https://genoryn.io"
accepted "$FULL" "https://node.example.org:30334/" "--backend cuda --devices 0,2 --max-cpu 50" "grep -qx 'NODE=https://node.example.org:30334' $M/genoryn.conf && [[ \$(grep -c '^ARG=' $M/genoryn.conf) -eq 6 ]]" "a full address, an https node and allowed options"
check "[[ \$(stat -c %a $M/genoryn.conf) == 600 ]]" "the config is readable by root only"
accepted "$SHORT.rig7" "http://192.168.1.20:30334" "" "grep -qx 'ADDRESS=$SHORT' $M/genoryn.conf" "a worker suffix after the address is dropped; a node on the farm's network over http"
accepted "$SHORT" "http://127.0.0.1:30334" "--threads 8" "grep -qx 'ARG=8' $M/genoryn.conf" "a node on the rig itself"
for t in "$SEED" "" "gnrs1short" "0x$(printf 'f%.0s' {1..40})" "$SHORT;reboot" "\$(reboot)" "$SHORT\`reboot\`" "gnrs1$(printf 'A%.0s' {1..52})"; do
	if conf "$t" "" ""; then bad "accepted the wallet '${t:0:30}'"; else ok "refused the wallet '${t:0:30}'"; fi
done
check "[[ ! -e $M/genoryn.conf ]]" "a refused Flight Sheet leaves no config behind"
for u in "http://node.example.org:30334" "ftp://genoryn.io" "https://genoryn.io/x;reboot" "https://\$(reboot)" "stratum+tcp://pool:3333" "http://8.8.8.8:30334" "https://-bad.example"; do
	if conf "$SHORT" "$u" ""; then bad "accepted the node '$u'"; else ok "refused the node '$u'"; fi
done
accepted "$SHORT" "https://n1.example.org https://n2.example.org" "" "grep -qx 'NODE=https://n1.example.org' $M/genoryn.conf" "of several pool URLs the first is used"
for x in "--node https://evil.example" "--address $FULL" "--backend gpu" "--devices 0;reboot" "--threads" "--max-cpu 0" "--max-cpu 101" "\$(reboot)" "--json" "--batch -1"; do
	if conf "$SHORT" "" "$x"; then bad "accepted the options '$x'"; else ok "refused the options '$x'"; fi
done

# A stand-in miner: the real one's --version and JSON lines, then it stays until stopped.
stub() {
	cat >"$M/krypton-miner" <<-EOF
		#!/usr/bin/env bash
		if [[ \${1-} == --version ]]; then echo "krypton-miner $1"; exit 0; fi
		echo "args: \$* address=\$KRYPTON_MINER_ADDRESS" >&2
		echo '{"event":"start","version":"$1","device":"CPU x2 threads","devices":1}'
		echo '{"event":"status","version":"$1","uptime_secs":75,"last_share_at":1795100000,"hashrate_hs":1520.5,"hashes":115000,"accepted_blocks":1,"accepted_shares":12,"rejected":1,"height":4242}'
		sleep "\${STUB_SLEEP:-0}"
	EOF
	chmod 755 "$M/krypton-miner"
	(cd "$M" && sha256sum krypton-miner >krypton-miner.sha256)
}
export GENORYN_FAIL_WAIT=0 GENORYN_LOCK=/tmp/genoryn-test.lock
LOG=/var/log/miner/custom/genoryn.log
VER=$(sed -n 's/^CUSTOM_VERSION=//p' "$M/h-manifest.conf")

conf "$SHORT" "" "--backend cpu --threads 2"
stub "$VER"
: >"$LOG"
bash "$M/h-run.sh" >/dev/null 2>&1
check "grep -q 'args: --node https://genoryn.io --json --backend cpu --threads 2 address=$SHORT' $LOG" "h-run starts the miner with exactly the checked settings"
check "grep -q '\"event\":\"status\"' $LOG" "the miner's output goes to the Hive OS log"

# h-stats.sh: the numbers come from the miner's last status line.
khs="" stats=""
# shellcheck source=/dev/null
. "$M/h-stats.sh"
check "[[ \$khs == 1.5205 ]]" "khs is the miner's hashrate in kH/s ($khs)"
check "[[ \$(jq -c .ar <<<'$stats') == '[12,1]' && \$(jq -r .uptime <<<'$stats') == 75 && \$(jq -r .algo <<<'$stats') == kryptohash && \$(jq -r .ver <<<'$stats') == $VER ]]" "stats: accepted and rejected shares, uptime, algorithm, version"
check "jq -e '.hs == [1520.5] and .hs_units == \"hs\"' <<<'$stats' >/dev/null" "the hashrate is the miner's total, not split up by guess"
echo 'garbage {"event":"status" not json' >>"$LOG"
. "$M/h-stats.sh"
check "[[ \$khs == 1.5205 ]]" "a broken line in the log is ignored"
touch -d '5 minutes ago' "$LOG"
. "$M/h-stats.sh"
check "[[ \$khs == 0 && -z \$stats ]]" "a log that has gone quiet reports nothing rather than old numbers"

# h-run.sh refuses what it must refuse.
printf 'x' >>"$M/krypton-miner"
: >"$LOG"
bash "$M/h-run.sh" >/dev/null 2>&1
check "grep -q 'does not match its checksum' $LOG && ! grep -q 'args:' $LOG" "a changed miner binary is never started"
stub "9.9.9"
: >"$LOG"
bash "$M/h-run.sh" >/dev/null 2>&1
check "grep -q \"is version '9.9.9'\" $LOG" "a miner of another version than the package is not started"
rm -f "$M/krypton-miner"
bash "$M/h-run.sh" >/dev/null 2>&1
check "grep -q 'is missing' $LOG" "a missing miner is reported"
stub "$VER"
rm -f "$CUSTOM_CONFIG_FILENAME"
bash "$M/h-run.sh" >/dev/null 2>&1
check "grep -q 'no checked config' $LOG" "no checked config, no start"
conf "$SHORT" "" ""
STUB_SLEEP=5 bash "$M/h-run.sh" >/dev/null 2>&1 &
sleep 2
: >"$LOG"
bash "$M/h-run.sh" >/dev/null 2>&1
if grep -q 'already running' "$LOG"; then ok "a second miner on the same rig is refused"; else bad "a second miner on the same rig is refused"; cat "$LOG"; jobs -l; fi
wait

echo
echo "passed $PASS, failed $FAIL"
((FAIL == 0))
