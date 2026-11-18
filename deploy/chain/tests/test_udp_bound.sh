#!/usr/bin/env bash
# udp_bound must work with ss output with and without a Netid column.
set -euo pipefail
script="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/cybernode-check"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin"
awk '/^udp_bound\(\)/{p=1} p{print} p&&/^}/{exit}' "$script" > "$tmp/fn.sh"
[[ -s "$tmp/fn.sh" ]] || { echo "udp_bound not found in script"; exit 1; }
# shellcheck disable=SC1091
source "$tmp/fn.sh"
export PATH="$tmp/bin:$PATH"
fail=0
check() { # label
  if udp_bound 7793; then :; else echo "$1: 7793 should be bound"; fail=1; fi
  if udp_bound 7794; then echo "$1: 7794 should not be bound"; fail=1; fi
}
cat > "$tmp/bin/ss" <<'E'
#!/bin/sh
cat <<'O'
State  Recv-Q Send-Q  Local Address:Port  Peer Address:Port Process
UNCONN 0      0             0.0.0.0:7793       0.0.0.0:*
UNCONN 0      0                [::]:7793          [::]:*
O
E
chmod +x "$tmp/bin/ss"
check "no-netid"
cat > "$tmp/bin/ss" <<'E'
#!/bin/sh
cat <<'O'
Netid State  Recv-Q Send-Q  Local Address:Port  Peer Address:Port Process
udp   UNCONN 0      0             0.0.0.0:7793       0.0.0.0:*
udp   UNCONN 0      0                [::]:7793          [::]:*
O
E
check "netid"
exit "$fail"
