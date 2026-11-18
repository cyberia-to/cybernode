#!/usr/bin/env bash
# The peers probe of cybernode-check: one case per rule the node applies to
# <chain>.peers.toml, on both paths — the tomllib program and the awk fallback
# (forced by a python3 that cannot import tomllib). The probe runs before the
# --pre early exit, so a wrong verdict here refuses to start the node.
set -uo pipefail
dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="$dir/cybernode-check"
real_py="$(command -v python3 || true)"
[[ -n "$real_py" ]] || { echo "python3 is needed to test the peers probe"; exit 1; }
"$real_py" -c 'import tomllib' 2>/dev/null || { echo "python3 without tomllib: the primary path cannot be tested"; exit 1; }

tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/etc" "$tmp/bin"
cat > "$tmp/etc/pussy.env" <<'E'
SOFT3_CHAIN=pussy
SOFT3_BIND=127.0.0.1:17792
SOFT3_RADIO=0.0.0.0:17793
E
# a python3 that refuses `import tomllib` and is the real one for everything
# else (the curve-point check of the fallback still runs)
cat > "$tmp/bin/python3" <<E
#!/bin/sh
for a in "\$@"; do case "\$a" in *tomllib*) exit 1 ;; esac; done
exec "$real_py" "\$@"
E
chmod +x "$tmp/bin/python3"

peers="$tmp/etc/pussy.peers.toml"
POINT=5866666666666666666666666666666666666666666666666666666666666666 # the ed25519 basepoint
NOTPOINT=0200000000000000000000000000000000000000000000000000000000000000 # 64 hex, y = 2, not on the curve
fail=0

probe() { # py|awk -> "<LEVEL> <message>" of the peers line
  local out
  if [[ "$1" == awk ]]; then
    out=$(SOFT3_ETC="$tmp/etc" SOFT3_STATE="$tmp/state" PATH="$tmp/bin:$PATH" bash "$script" --pre pussy 2>/dev/null)
  else
    out=$(SOFT3_ETC="$tmp/etc" SOFT3_STATE="$tmp/state" bash "$script" --pre pussy 2>/dev/null)
  fi
  sed -n 's/^\([A-Z]*\) peers: /\1 /p' <<<"$out"
}

want() { # label path level substring
  local got; got="$(probe "$2")"
  if [[ "$got" != "$3 "* || "$got" != *"$4"* ]]; then
    echo "$1 [$2]: want '$3 … $4', got '$got'"
    fail=1
  fi
}

both() { # label py-level awk-level substring
  want "$1" py "$2" "$4"
  want "$1" awk "$3" "$4"
}

cat > "$peers" <<E
version = 1
chain = "pussy"

[[peer]]
name = "agent-a"
id = "$POINT"
addrs = ["194.107.163.6:7793"]

[[peer]]
name = "six"
id = "0x$POINT"
addrs = ["[2a01:4f9:c010:1234::1]:7793"]
E
# the awk fallback never promises more than DEGRADED, even on a good file
both "a good file" OK DEGRADED "2 bootstrap peers in $peers"

cat > "$peers" <<E
version = 1
chain = "bostrom"

[[peer]]
name = "agent-a"
id = "$POINT"
addrs = ["194.107.163.6:7793"]
E
both "another chain" DOWN DOWN "chain is 'bostrom', this instance runs 'pussy'"

cat > "$peers" <<E
version = 2
chain = "pussy"

[[peer]]
name = "agent-a"
id = "$POINT"
addrs = ["194.107.163.6:7793"]
E
both "version 2" DOWN DOWN "version is 2, not 1"

cat > "$peers" <<E
version = 1
chain = "pussy"

[[peer]]
name = "agent-a"
addrs = ["194.107.163.6:7793"]
E
both "a peer with no id" DOWN DOWN "peer 'agent-a': missing field id"

cat > "$peers" <<E
version = 1
chain = "pussy"

[[peer]]
name = "agent-a"
id = ""
addrs = ["194.107.163.6:7793"]
E
both "an empty id" DEGRADED DEGRADED "skipped (empty id): agent-a"

cat > "$peers" <<E
version = 1
chain = "pussy"

[[peer]]
name = "agent-a"
id = "$POINT"
addrs = ["agent-a.cyberia.to:7793"]
E
both "no parsable address" DOWN DOWN "peer 'agent-a': no parsable address"

cat > "$peers" <<E
version = 1
chain = "pussy"

[[peer]]
name = "agent-a"
id = "$POINT"
addrs = ["194.107.163.6:7793", "194.107.163.6:99999"]
E
both "an unparsable address beside a good one" DEGRADED DEGRADED "dropped (unparsable address): agent-a"

cat > "$peers" <<E
version = 1
chain = "pussy"

[[peer]]
name = "agent-a"
id = "$NOTPOINT"
addrs = ["194.107.163.6:7793"]
E
both "64 hex that is not a curve point" DOWN DOWN "peer 'agent-a': id is not an ed25519 point"

cat > "$peers" <<E
version = 1
chain = "pussy"

[[peer]]
name = "agent-a"
id = "abc"
addrs = ["194.107.163.6:7793"]
E
both "an id that is not 64 hex" DOWN DOWN "peer 'agent-a': id is not 64 hex chars"

# only the TOML parser sees this one; the awk fallback reads lines
cat > "$peers" <<E
version = 1
chain = "pussy"
[[peer]
name = "agent-a"
E
want "not valid TOML" py DOWN "not valid TOML"

rm -f "$peers"
both "no file at all" DOWN DOWN "missing or unreadable"

exit "$fail"
