#!/bin/sh
# Gate 2 from PLAN.md: no god files.
# Tau's harness.rs is 34,875 lines and its test file is 42,191. Both grew one commit at a time.
#
# POSIX, and executed rather than sourced through `sh`: this runs on a CI runner where /bin/sh is
# dash, which has neither process substitution nor `set -o pipefail`.
#
# `GATE_ROOT` because casper and melchior keep their sources in `src`, not `crates`.
set -eu
LIMIT=800
ROOT="${GATE_ROOT:-crates}"

# The offenders are collected as output rather than counted into a flag: a `while` loop on the
# right of a pipe runs in a subshell, so a flag set inside it is lost on the way out.
offenders=$(
  find "$ROOT" -name '*.rs' -type f -not -path '*/target/*' | sort | while IFS= read -r file; do
    lines=$(wc -l < "$file")
    if [ "$lines" -gt "$LIMIT" ]; then
      printf '%s: %s lines (limit %s)\n' "$file" "$lines" "$LIMIT"
    fi
  done
)

if [ -n "$offenders" ]; then
  printf '%s\n' "$offenders" >&2
  echo "gate-file-size: failed" >&2
  exit 1
fi
echo "gate-file-size: ok"
