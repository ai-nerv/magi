#!/usr/bin/env bash
# Gate 2 from PLAN.md: no god files.
# Tau's harness.rs is 34,875 lines and its test file is 42,191. Both grew one commit at a time.
set -euo pipefail
LIMIT=800
fail=0
while IFS= read -r file; do
  lines=$(wc -l < "$file")
  if [ "$lines" -gt "$LIMIT" ]; then
    echo "$file: $lines lines exceeds the $LIMIT line limit"
    fail=1
  fi
done < <(find crates -name '*.rs' -not -path '*/target/*')
exit $fail
