#!/usr/bin/env bash
# Updates a Genoryn Krypton node on Debian or Ubuntu to the latest release. Download the
# script of the release you run, read it, then run it:
#
#   curl -fsSLO https://raw.githubusercontent.com/genoryn/genoryn/v<version>/deploy/update.sh
#   sudo bash update.sh
#
# It downloads the release's .deb, SHA256SUMS and SHA256SUMS.sig from GitHub. Installs from
# 1.0.52 on carry the release key and the krypton-release checker, and the signature (the
# hybrid post-quantum key in docs/release-key.pub) is then checked against the installed
# key before anything is installed. The package is checked against the checksums, installed,
# and kryptond restarted if it runs as a service. It never installs an older or the same
# version. Your chain data, wallets and settings are not touched.
set -euo pipefail

# Everything runs from here, called on the last line: a download cut short runs nothing.
main() {
  local api=https://api.github.com/repos/genoryn/genoryn/releases/latest
  local tag have
  tag=$(curl -fsSL "$api" | grep -oE '"tag_name": *"v[0-9]+\.[0-9]+\.[0-9]+"' | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1)
  [ -n "$tag" ] || { echo "could not read the latest release"; exit 1; }
  have=$(kryptond --version 2>/dev/null | grep -oE '[0-9]+\.[0-9]+\.[0-9]+' | head -1 || true)
  if [ -n "$have" ] && [ "$(printf '%s\n%s\n' "$have" "$tag" | sort -V | tail -1)" = "$have" ]; then
    echo "kryptond $have is up to date (latest release: $tag)"
    exit 0
  fi
  echo "updating kryptond ${have:-(not installed)} -> $tag"
  local base=https://github.com/genoryn/genoryn/releases/download/v$tag
  local deb=genoryn-krypton_${tag}_amd64.deb
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  curl -fsSL -o "$tmp/$deb" "$base/$deb"
  curl -fsSL -o "$tmp/SHA256SUMS" "$base/SHA256SUMS"
  curl -fsSL -o "$tmp/SHA256SUMS.sig" "$base/SHA256SUMS.sig"
  local key=/usr/share/genoryn-krypton/release-key.pub
  if command -v krypton-release >/dev/null 2>&1 && [ -f "$key" ]; then
    krypton-release verify "$key" "$tmp/SHA256SUMS" "$tmp/SHA256SUMS.sig" \
      || { echo "the release signature does not match the installed release key: nothing installed"; exit 1; }
  else
    echo "note: this install has no release checker yet (installs from 1.0.52 do); checking the checksums only"
  fi
  awk -v f="$deb" '$2 == f && $1 ~ /^[0-9a-f]{64}$/ { ok = 1 } END { exit !ok }' "$tmp/SHA256SUMS" \
    || { echo "SHA256SUMS does not list $deb"; exit 1; }
  (cd "$tmp" && sha256sum -c SHA256SUMS --ignore-missing)
  local was_active=no
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
}

main "$@"
