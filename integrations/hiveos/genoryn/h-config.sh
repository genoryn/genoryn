#!/usr/bin/env bash
# Hive OS calls this before every start of the miner, with the Flight Sheet in scope:
#   CUSTOM_TEMPLATE     wallet and worker template: your GNR address (%WAL%)
#   CUSTOM_URL          pool URL: the node to mine through (empty: https://genoryn.io)
#   CUSTOM_USER_CONFIG  extra config arguments: --backend, --devices, --threads, --max-cpu, --batch
# It checks them and writes the miner's config. Seed phrases and keys are never asked for:
# mining needs only a public address.

DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=/dev/null
. "$DIR/h-manifest.conf"
# shellcheck source=genoryn-check.sh
. "$DIR/genoryn-check.sh"

genoryn_config() {
	# A Flight Sheet that does not pass leaves no config behind: the miner then does not
	# start with the settings of an earlier one.
	rm -f "$CUSTOM_CONFIG_FILENAME"
	# The address is the first word of the template (a worker suffix such as
	# "%WAL%.%WORKER_NAME%" is dropped: the network knows miners by address).
	local address node args word
	address=${CUSTOM_TEMPLATE%%[[:space:]]*}
	address=${address%%.*}
	address=${address%%/*}
	if ! genoryn_valid_address "$address"; then
		echo "genoryn: the wallet in the Flight Sheet is not a GNR address (use your gnrs1... short address or the full gnr1... address; never a recovery phrase)" >&2
		return 1
	fi
	node=${CUSTOM_URL%%[[:space:]]*}
	node=${node%/}
	[[ -n $node ]] || node=$CUSTOM_DEFAULT_NODE
	if ! genoryn_valid_node "$node"; then
		echo "genoryn: the pool URL must be https://<node> or http:// to a node on this computer or the local network (got: ${node:0:80})" >&2
		return 1
	fi
	if ! args=$(genoryn_extra_args "${CUSTOM_USER_CONFIG-}"); then
		echo "genoryn: extra config arguments may only be --backend, --devices, --threads, --max-cpu and --batch, each with a value" >&2
		return 1
	fi
	local conf=$CUSTOM_CONFIG_FILENAME tmp
	mkdir -p "$(dirname "$conf")" || return 1
	tmp=$(mktemp "$conf.XXXXXX") || return 1
	chmod 600 "$tmp"
	{
		printf 'NODE=%s\n' "$node"
		printf 'ADDRESS=%s\n' "$address"
		while IFS= read -r word; do
			if [[ -n $word ]]; then printf 'ARG=%s\n' "$word"; fi
		done <<<"$args"
	} >"$tmp" && mv -f "$tmp" "$conf"
}

genoryn_config
