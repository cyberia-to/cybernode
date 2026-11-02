#!/usr/bin/env bash
# Run every deploy/chain test; non-zero exit if any fails.
dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
rc=0
for t in "$dir"/test_*.sh; do
  if out="$(bash "$t" 2>&1)"; then echo "PASS $(basename "$t")"
  else echo "FAIL $(basename "$t")"; [[ -n "$out" ]] && sed 's/^/    /' <<<"$out"; rc=1; fi
done
exit "$rc"
