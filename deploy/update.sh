#!/usr/bin/env bash
# Updates a Genoryn Krypton node on Debian or Ubuntu to the latest release:
#
#   curl -fsSL https://raw.githubusercontent.com/genoryn/genoryn/main/deploy/update.sh | sudo bash
#
# It downloads the release's .deb and its SHA256SUMS from GitHub, checks the package against
# the checksums, installs it and restarts kryptond if it runs as a service. Your chain data,
# wallets and settings are not touched. To check the release signature as well (the hybrid
# post-quantum key in docs/release-key.pub), see "Verify your download" in the release notes.
set -euo pipefail

api=https://api.github.com/repos/genoryn/genoryn/releases/latest
tag=$(curl -fsSL "$api" | grep -oE '"tag_name": *"v[0-9.]+"' | grep -oE '[0-9][0-9.]*')
[ -n "$tag" ] || { echo "could not read the latest release"; exit 1; }
have=$(kryptond --version 2>/dev/null | grep -oE '[0-9][0-9.]*' || true)
if [ "$have" = "$tag" ]; then
  echo "kryptond $have is the latest release"
  exit 0
fi
echo "updating kryptond ${have:-(not installed)} -> $tag"
base=https://github.com/genoryn/genoryn/releases/download/v$tag
deb=genoryn-krypton_${tag}_amd64.deb
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
curl -fsSL -o "$tmp/$deb" "$base/$deb"
curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS"
(cd "$tmp" && sha256sum -c SHA256SUMS --ignore-missing)
was_active=no
systemctl is-active --quiet kryptond 2>/dev/null && was_active=yes
apt-get install -y "$tmp/$deb"
# A node set up as a service (deploy/setup-server.sh) runs from /opt/krypton/bin.
if [ -x /opt/krypton/bin/kryptond ]; then
  for f in kryptond krypton-miner krypton-wallet-cli; do
    if [ -x "/usr/bin/$f" ]; then install -m 755 "/usr/bin/$f" /opt/krypton/bin/; fi
  done
fi
if [ "$was_active" = yes ]; then
  systemctl restart kryptond
  sleep 3
  systemctl is-active kryptond
fi
kryptond --version
