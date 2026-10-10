#!/usr/bin/env bash
# Build web + server and restart the genoryn.io service.
set -euo pipefail
cd "$(dirname "$0")/.."
npm run test
npm run build
systemctl restart genoryn-web
sleep 3
systemctl is-active genoryn-web
curl -fsS -o /dev/null -w 'local: %{http_code}\n' http://127.0.0.1:3000/
curl -fsS http://127.0.0.1:3000/api/v1/health
echo
