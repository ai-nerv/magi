#!/bin/sh
# Gate 1 from PLAN.md: nothing merges unless the shipping path reaches it.
# Pi fails this on ~20,000 LOC; Tau on 1,202. Both shipped the dead code anyway.
set -eu
reachable=$(cargo tree -p magi-cli --prefix none --no-dedupe 2>/dev/null \
  | awk '{print $1}' | grep '^magi-' | sort -u)
members=$(cargo metadata --no-deps --format-version 1 2>/dev/null \
  | grep -o '"name":"magi-[a-z-]*"' | cut -d'"' -f4 | sort -u)
fail=0
for member in $members; do
  # The testkit is the one exemption: it exists to be depended on by tests.
  [ "$member" = "magi-testkit" ] && continue
  if ! echo "$reachable" | grep -qx "$member"; then
    echo "$member is not reachable from the magi binary"
    fail=1
  fi
done
exit $fail
