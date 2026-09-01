#!/usr/bin/env bash
# Every source file is reachable from its crate root.
#
# A .rs file nobody declares is not a compile error, not a warning, and not run -- it simply is
# not part of the crate. Two of them were found at once: `prompt/says.rs` and `keys/accept.rs`,
# each holding tests that had silently not run since the commit that moved them there. Both were
# a `mod` line an edit failed to add, and nothing anywhere would have said so.
set -euo pipefail
fail=0
while IFS= read -r file; do
  name=$(basename "$file" .rs)
  crate=$(echo "$file" | cut -d/ -f1-2)
  # `mod name;` anywhere in the crate, or a `#[path]` naming the file -- both are how a module
  # gets declared here, and the second is what the `_tests` files use.
  if grep -rqE "^ *(pub |pub\(super\) |pub\(crate\) )?mod +${name} *;" "$crate/src" \
     || grep -rqF "$(basename "$file")\"" "$crate/src"; then
    continue
  fi
  echo "$file is not declared anywhere: nothing compiles it and its tests do not run"
  fail=1
done < <(find crates -name '*.rs' -not -path '*/target/*' \
           -not -name lib.rs -not -name main.rs -not -name mod.rs \
           -not -path '*/tests/*' -not -path '*/examples/*' -not -name build.rs)
exit $fail
