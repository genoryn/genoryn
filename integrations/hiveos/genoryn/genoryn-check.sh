#!/usr/bin/env bash
# Checks for the Genoryn Krypton Hive OS package, shared by h-config.sh and the tests.
# Everything a Flight Sheet passes in is checked here before the miner is started; nothing
# from it is ever evaluated as shell code.

# A payout address: the short form (gnrs1..., tgnrs1..., dgnrs1...) or the full one.
# The miner checks the address's checksum (and a short address against its hash) itself.
genoryn_valid_address() {
	local a=$1
	[[ ${#a} -le 4096 ]] || return 1
	[[ $a =~ ^(gnr|tgnr|dgnr)s1[a-z2-7]{50,56}$ ]] && return 0
	[[ $a =~ ^(gnr|tgnr|dgnr)1[a-z2-7]{1000,4000}$ ]] && return 0
	return 1
}

# The node: https to any host name or address, or plain http to this computer or a private
# network address only (a node on the farm's own network).
genoryn_valid_node() {
	local u=${1%/}
	[[ ${#u} -le 200 ]] || return 1
	if [[ $u =~ ^https://[A-Za-z0-9]([A-Za-z0-9.-]{0,251}[A-Za-z0-9])?(:[0-9]{1,5})?$ ]]; then
		return 0
	fi
	if [[ $u =~ ^http://(localhost|127\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}|10\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}|192\.168\.[0-9]{1,3}\.[0-9]{1,3}|172\.(1[6-9]|2[0-9]|3[01])\.[0-9]{1,3}\.[0-9]{1,3})(:[0-9]{1,5})?$ ]]; then
		return 0
	fi
	return 1
}

# Extra arguments from the Flight Sheet: only these miner options, each with a value of the
# right form. Prints the accepted words one per line; fails on anything else.
genoryn_extra_args() {
	local -a words
	read -r -a words <<<"$1"
	local i=0 flag value
	while ((i < ${#words[@]})); do
		flag=${words[i]}
		value=${words[i + 1]-}
		case $flag in
		--backend) [[ $value =~ ^(auto|cpu|cuda|opencl)$ ]] || return 1 ;;
		--devices) [[ $value =~ ^[0-9]{1,2}(,[0-9]{1,2}){0,31}$ ]] || return 1 ;;
		--threads) [[ $value =~ ^[0-9]{1,3}$ ]] && ((value >= 1)) || return 1 ;;
		--max-cpu) [[ $value =~ ^[0-9]{1,3}$ ]] && ((value >= 1 && value <= 100)) || return 1 ;;
		--batch) [[ $value =~ ^[0-9]{1,10}$ ]] && ((value >= 1)) || return 1 ;;
		*) return 1 ;;
		esac
		printf '%s\n%s\n' "$flag" "$value"
		i=$((i + 2))
	done
}
