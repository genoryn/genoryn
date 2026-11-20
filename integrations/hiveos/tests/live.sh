#!/usr/bin/env bash
# The real package against a real node, in Ubuntu 20.04 laid out like a Hive OS rig:
# a devnet node and a CPU miner installed from genoryn-<version>.tar.gz. Checks shares,
# the stats Hive OS reads, losing and regaining the node, log rotation, and stop/start.
#
#   integrations/hiveos/tests/live-run.sh <dir with genoryn-<version>.tar.gz>
set -uo pipefail
PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); echo "ok   $*"; }
bad() { FAIL=$((FAIL + 1)); echo "FAIL $*"; }
NODE=http://127.0.0.1:32334
M=/hive/miners/custom/genoryn
LOG=/var/log/miner/custom/genoryn.log
status() { grep -a '"event":"status"' "$LOG" 2>/dev/null | tail -n 1; }
shares() { status | jq -r '.accepted_shares // 0' 2>/dev/null || echo 0; }
wait_for() { # seconds, condition
	local end=$((SECONDS + $1))
	while ((SECONDS < end)); do
		if eval "$2"; then return 0; fi
		sleep 3
	done
	return 1
}

export DEBIAN_FRONTEND=noninteractive
apt-get update -qq && apt-get install -y -qq curl ca-certificates jq build-essential pkg-config procps util-linux >/dev/null
curl -sSf https://sh.rustup.rs | sh -s -- -y -q --profile minimal >/dev/null
. "$HOME/.cargo/env"
mkdir -p /build && cd /build && tar -C /src --exclude=./target --exclude=./website/dist -cf - . | tar -xf -
cargo build --release --locked -p kryptond -p krypton-wallet-cli 2>&1 | tail -n 1
BIN=/build/target/release

start_node() { "$BIN/kryptond" --network devnet --no-seeds --data-dir /tmp/node --rpc 127.0.0.1:32334 >>/tmp/node.log 2>&1 & }
start_node
wait_for 60 "curl -sf $NODE/v1/info >/dev/null" && ok "a devnet node runs" || bad "the devnet node did not start"

export KRYPTON_WALLET_PASSWORD="live test wallet password"
W="$BIN/krypton-wallet-cli --network devnet --node $NODE --wallet /tmp/w.json"
$W new >/dev/null 2>&1
SHORT=$($W address --publish 2>/dev/null | sed -n 's/^short: //p')
[[ $SHORT == dgnrs1* ]] && ok "a devnet wallet published its address ($SHORT)" || bad "no short address"

mkdir -p /hive/miners/custom /var/log/miner/custom
tar -xzf /pkg/genoryn-*.tar.gz -C /hive/miners/custom
VER=$(sed -n 's/^CUSTOM_VERSION=//p' "$M/h-manifest.conf")
CUSTOM_TEMPLATE=$SHORT CUSTOM_URL=$NODE CUSTOM_USER_CONFIG="--backend cpu --threads 2" bash "$M/h-config.sh" &&
	ok "h-config.sh accepts the Flight Sheet" || bad "h-config.sh refused the Flight Sheet"

(cd "$M" && setsid bash h-run.sh >/dev/null 2>&1 &)
wait_for 120 "grep -q '\"event\":\"start\"' $LOG" && ok "the miner started (version $VER)" || bad "the miner did not start: $(tail -n 5 $LOG)"
wait_for 600 "(( \$(shares) >= 3 ))" && ok "the node accepted shares from the rig ($(shares))" || bad "no shares within ten minutes: $(tail -n 3 $LOG)"

khs="" stats=""
# shellcheck source=/dev/null
. "$M/h-stats.sh"
if awk "BEGIN{exit !($khs > 0)}" && [[ $(jq -r '.ar[0]' <<<"$stats") -ge 3 && $(jq -r .ver <<<"$stats") == "$VER" ]]; then
	ok "Hive OS reads $khs kH/s, $(jq -c .ar <<<"$stats") shares, version $VER"
else
	bad "stats: khs=$khs stats=$stats"
fi
FULL=$($W address 2>/dev/null | tail -n 1)
credited=$(curl -sf "$NODE/v1/miner/$FULL" | jq -r '((.shares_recent // 0) + (.shares_pending // 0) + ((.unpaid // 0) | if . > 0 then 1 else 0 end))')
if [[ ${credited:-0} -ge 1 ]]; then ok "the node counts the rig's shares for its address"; else bad "the node shows no shares for the address"; fi

# The node goes away for a while, then comes back.
pkill -f "kryptond --network devnet"
wait_for 60 "grep -q 'waiting for node' $LOG" && ok "without its node the miner waits and says so" || bad "no 'waiting for node'"
sleep 10
pgrep -f "krypton-miner --node" >/dev/null && ok "the miner keeps running (no restart loop)" || bad "the miner exited without its node"
before=$(shares)
start_node
wait_for 300 "(( \$(shares) > before ))" && ok "after the node is back, shares are accepted again ($before -> $(shares))" || bad "no shares after the node came back"

# Hive OS truncates the log to rotate it; the stats come back with the next status line.
: >"$LOG"
wait_for 30 "[[ -n \$(status) ]]" && {
	. "$M/h-stats.sh"
	awk "BEGIN{exit !($khs >= 0)}" && [[ -n $stats ]] && ok "after log rotation the stats are read again"
} || bad "nothing in the log after rotation"

# Stop (as Hive OS does, by ending the session) and start again.
pkill -f "bash h-run.sh"
pkill -f "krypton-miner --node"
wait_for 20 "! pgrep -f 'krypton-miner --node' >/dev/null" && ok "the miner stops" || bad "the miner did not stop"
(cd "$M" && setsid bash h-run.sh >/dev/null 2>&1 &)
wait_for 60 "pgrep -f 'krypton-miner --node' >/dev/null" && ok "and starts again (the lock was released)" || bad "the miner did not start again: $(tail -n 3 $LOG)"
mem=$(ps -o rss= -p "$(pgrep -f 'krypton-miner --node' | head -n 1)" 2>/dev/null)
echo "miner memory: $((${mem:-0} / 1024)) MiB (the 2 GiB dataset is built when work arrives)"
pkill -f "bash h-run.sh"
pkill -f "krypton-miner --node"
pkill -f kryptond

echo
echo "passed $PASS, failed $FAIL"
((FAIL == 0))
