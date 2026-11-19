#!/usr/bin/env bash
# Sets a Genoryn Krypton node up as a service (+ optional CPU miner capped at 50 %) on a
# Linux with systemd. The release programs need glibc 2.39 (Ubuntu 24.04, Debian 13).
# Usage: sudo bash setup-server.sh <network> [--miner <payout-address-file>]
# After the .deb package: sudo bash /usr/lib/genoryn-krypton/deploy/setup-server.sh mainnet
set -euo pipefail
NET="${1:-mainnet}"
MINER_ADDR=""
if [[ "${2:-}" == "--miner" ]]; then MINER_ADDR="$3"; fi
case "$NET" in mainnet) RPC=30334; P2P=30333 ;; testnet) RPC=31334; P2P=31333 ;; *) echo "network?"; exit 1 ;; esac
HERE="$(cd "$(dirname "$0")" && pwd)"

id krypton >/dev/null 2>&1 || useradd --system --home /var/lib/krypton --shell /usr/sbin/nologin krypton
install -d -o krypton -g krypton -m 750 /var/lib/krypton
install -d -m 755 /opt/krypton/bin /etc/krypton
# The programs: next to this folder (the release .tar.gz), in a bin folder (a source
# build copied here), or installed by the .deb package.
BIN=""
for d in "$HERE/.." "$HERE/../bin" "$HERE/bin" "$HERE/../target/release" /usr/bin; do
  if [[ -x "$d/kryptond" ]]; then BIN="$d"; break; fi
done
[[ -n "$BIN" ]] || { echo "kryptond was not found next to this script or in /usr/bin"; exit 1; }
for f in kryptond krypton-miner krypton-wallet-cli; do
  if [[ -x "$BIN/$f" ]]; then install -m 755 "$BIN/$f" /opt/krypton/bin/; fi
done

printf 'KRYPTON_NETWORK=%s\nKRYPTON_RPC_PORT=%s\n' "$NET" "$RPC" > /etc/krypton/kryptond.env
install -m 644 "$HERE/systemd/kryptond.service" /etc/systemd/system/kryptond.service

if [[ -n "$MINER_ADDR" ]]; then
  CORES=$(nproc)
  THREADS=$(( CORES / 2 > 0 ? CORES / 2 : 1 ))
  QUOTA=$(( CORES * 50 ))%
  install -m 640 -g krypton "$MINER_ADDR" /etc/krypton/miner-address.txt
  printf 'KRYPTON_RPC_PORT=%s\nKRYPTON_MINER_THREADS=%s\n' "$RPC" "$THREADS" > /etc/krypton/miner.env
  sed "s/__CPU_QUOTA__/$QUOTA/" "$HERE/systemd/krypton-miner.service" > /etc/systemd/system/krypton-miner.service
fi

# A firewall that is already on gets the node's port opened; one that is off is left alone.
if command -v ufw >/dev/null && ufw status 2>/dev/null | grep -q "Status: active"; then
  ufw allow "$P2P"/tcp >/dev/null
  echo "ufw: opened port $P2P/tcp for other nodes"
fi
systemctl daemon-reload
systemctl enable --now kryptond
if [[ -n "$MINER_ADDR" ]]; then systemctl enable --now krypton-miner; fi
echo "kryptond ($NET) running; API http://127.0.0.1:$RPC/v1/info"
