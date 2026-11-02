#!/usr/bin/env bash
# ufw-chain.sh present() must match plain `ufw status` (ALLOW, no IN) and still match verbose (ALLOW IN).
set -euo pipefail
script="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/ufw-chain.sh"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin"
grep -E '^present\(\)' "$script" > "$tmp/fn.sh"
[[ -s "$tmp/fn.sh" ]] || { echo "present() not found in script"; exit 1; }
# shellcheck disable=SC1091
source "$tmp/fn.sh"
export PATH="$tmp/bin:$PATH"
fail=0
check() { # label
  port=7793; if present; then :; else echo "$1: 7793/udp should be present"; fail=1; fi
  port=7794; if present; then echo "$1: 7794/udp should not be present"; fail=1; fi
}
cat > "$tmp/bin/ufw" <<'E'
#!/bin/sh
cat <<'O'
Status: active

To                         Action      From
--                         ------      ----
7793/udp                   ALLOW       Anywhere                   # cybernode-pussy-radio
7793/udp (v6)              ALLOW       Anywhere (v6)              # cybernode-pussy-radio
O
E
chmod +x "$tmp/bin/ufw"
check "plain"
cat > "$tmp/bin/ufw" <<'E'
#!/bin/sh
cat <<'O'
Status: active

To                         Action      From
--                         ------      ----
7793/udp                   ALLOW IN    Anywhere                   # cybernode-pussy-radio
O
E
check "verbose"
exit "$fail"
