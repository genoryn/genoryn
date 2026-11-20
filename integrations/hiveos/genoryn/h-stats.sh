#!/usr/bin/env bash
# shellcheck disable=SC2034  # khs and stats are read by the agent that includes this
# Hive OS's agent includes this to read the miner's stats. It must set $khs (total
# hashrate in kH/s) and $stats (JSON). Everything comes from the miner's own JSON status
# line (printed every 10 s): nothing is estimated or made up. What the miner does not
# report (per-device hashrate, a worker name) is left out rather than guessed.

gnr_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=/dev/null
. "$gnr_dir/h-manifest.conf"
gnr_log="$CUSTOM_LOG_BASENAME.log"
khs=0
stats=""
gnr_line=""
# A status line at most a minute old (the miner writes one every 10 s).
if [[ -r $gnr_log ]] && (($(date +%s) - $(stat -c %Y "$gnr_log") < 60)); then
	gnr_line=$(tail -n 400 "$gnr_log" | grep -a '^{"' | grep -a '"event":"status"' | tail -n 1)
fi
if [[ -n $gnr_line ]] && jq -e . >/dev/null 2>&1 <<<"$gnr_line"; then
	khs=$(jq -r '((.hashrate_hs // 0) / 1000)' <<<"$gnr_line")
	stats=$(jq -c --arg ver "$CUSTOM_VERSION" '{
		hs: [(.hashrate_hs // 0)],
		hs_units: "hs",
		uptime: (.uptime_secs // 0),
		ver: (.version // $ver),
		ar: [(.accepted_shares // 0), (.rejected // 0)],
		algo: "kryptohash",
		height: (.height // 0),
		last_share_at: (.last_share_at // 0)
	}' <<<"$gnr_line")
fi
