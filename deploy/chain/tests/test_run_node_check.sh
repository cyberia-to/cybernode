#!/usr/bin/env bash
# run-node.sh must find cybernode-check installed next to it (README: bin/ holds both).
set -euo pipefail
src="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/agent-a/run-node.sh"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/etc"
cat > "$tmp/etc/testchain.env" <<'E'
SOFT3_CHAIN=testchain
SOFT3_BIND=127.0.0.1:7792
SOFT3_RADIO=0.0.0.0:7793
SOFT3_MONIKER=test
E
printf '#!/bin/sh\nexit 0\n' > "$tmp/bin/cybernode"; chmod +x "$tmp/bin/cybernode"
cp "$src" "$tmp/bin/run-node.sh"
printf '#!/usr/bin/env bash\ntouch "%s/called"\nexit 1\n' "$tmp" > "$tmp/bin/cybernode-check"
env -u SOFT3_CHECK CHAIN_ROOT="$tmp" PATH="/usr/bin:/bin" \
  timeout 20 bash "$tmp/bin/run-node.sh" testchain >/dev/null 2>&1 || true
[[ -e "$tmp/called" ]] || { echo "cybernode-check next to run-node.sh was not used"; exit 1; }
