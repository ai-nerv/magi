#!/bin/sh
# Gate 5 from PLAN.md: magi-proto stays under 4,000 lines.
# POSIX: /bin/sh on the runner is dash, which has no `-o pipefail`.
# Tau's tau-proto is 22,750 lines carrying 165 event variants, and that is the direct cause
# of its 34,875-line daemon. Overflow here is a design smell, not a budget request.
set -eu
LIMIT=4000
lines=$(find crates/magi-proto -name '*.rs' -exec cat {} + | wc -l)
echo "magi-proto: $lines / $LIMIT lines"
[ "$lines" -le "$LIMIT" ]
