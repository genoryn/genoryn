#!/usr/bin/env bash
# Installs a Genoryn Krypton node (+ optional CPU miner capped at 50 %) on Ubuntu 22.04/24.04.
# Usage: sudo ./setup-server.sh <network> [--miner <payout-address-file>]
set -euo pipefail
NET="${1:-testnet}"
MINER_ADDR=""
if [[ "${2:-}" == "--miner" ]]; then MINER_ADDR="$3"; fi
case "$NET" in mainnet) RPC=30334; P2P=30333 ;; testnet) RPC=31334; P2P=31333 ;; *) echo "network?"; exit 1 ;; esac
HERE="$(cd "$(dirname "$0")" && pwd)"

id krypton >/dev/null 2>&1 || useradd --system --home /var/lib/krypton --shell /usr/sbin/nologin krypton
install -d -o krypton -g krypton -m 750 /var/lib/krypton
install -d -m 755 /opt/krypton/bin /etc/krypton
install -m 755 "$HERE"/../bin/* /opt/krypton/bin/ 2>/dev/null || install -m 755 "$HERE"/bin/* /opt/krypton/bin/

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

if command -v ufw >/dev/null; then
  ufw allow OpenSSH >/dev/null; ufw allow 80/tcp >/dev/null; ufw allow 443/tcp >/dev/null; ufw allow "$P2P"/tcp >/dev/null
  ufw --force enable >/dev/null
fi
systemctl daemon-reload
systemctl enable --now kryptond
[[ -n "$MINER_ADDR" ]] && systemctl enable --now krypton-miner
echo "kryptond ($NET) running; API http://127.0.0.1:$RPC/v1/info"
