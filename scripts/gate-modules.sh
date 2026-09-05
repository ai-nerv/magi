#!/bin/sh
# Every source file is reachable from its crate root.
#
# A .rs file nobody declares is not a compile error, not a warning, and not run -- it simply is
# not part of the crate. Two of them were found at once: `prompt/says.rs` and `keys/accept.rs`,
# each holding tests that had silently not run since the commit that moved them there. Both were
# a `mod` line an edit failed to add, and nothing anywhere would have said so.
#
# POSIX for the same reason the others are: /bin/sh on the runner is dash.
#
# `GATE_ROOT` for the single-crate repos. When it is `crates`, a file's crate root is the first
# two path components (`crates/<name>`); when the whole repo is one crate there is nothing to
# strip and the root is the search root itself.
set -eu
ROOT="${GATE_ROOT:-crates}"

undeclared=$(
  find "$ROOT" -name '*.rs' -not -path '*/target/*' \
    -not -name lib.rs -not -name main.rs -not -name mod.rs \
    -not -path '*/tests/*' -not -path '*/examples/*' -not -name build.rs \
  | sort | while IFS= read -r file; do
    name=$(basename "$file" .rs)
    if [ "$ROOT" = "crates" ]; then
      crate=$(echo "$file" | cut -d/ -f1-2)/src
    else
      crate="$ROOT"
    fi
    # `mod name;` anywhere in the crate, or a `#[path]` naming the file -- both are how a module
    # gets declared here, and the second is what the `_tests` files use.
    if grep -rqE "^ *(pub |pub\(super\) |pub\(crate\) )?mod +${name} *;" "$crate" \
       || grep -rqF "$(basename "$file")\"" "$crate"; then
      continue
    fi
    printf '%s is not declared anywhere: nothing compiles it and its tests do not run\n' "$file"
  done
)

if [ -n "$undeclared" ]; then
  printf '%s\n' "$undeclared" >&2
  echo "gate-modules: failed" >&2
  exit 1
fi
echo "gate-modules: ok"
