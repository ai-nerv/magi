#!/usr/bin/env bash
# Gate 5 from PLAN.md: magi-proto stays under 4,000 lines.
# Tau's tau-proto is 22,750 lines carrying 165 event variants, and that is the direct cause
# of its 34,875-line daemon. Overflow here is a design smell, not a budget request.
set -euo pipefail
LIMIT=4000
lines=$(find crates/magi-proto -name '*.rs' -exec cat {} + | wc -l)
echo "magi-proto: $lines / $LIMIT lines"
[ "$lines" -le "$LIMIT" ]
