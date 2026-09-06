#!/bin/sh
# No cycles among a crate's top-level modules.
#
# `{asking, directory, identity, policy}` were a knot of three: asking resolved a name through
# directory while directory dialled through asking; identity asked the directory which names were
# taken while the directory was built out of identities; policy read a setting the directory
# owned while the directory asked policy what was allowed. Nothing said so. Every one of them
# compiles, and a cycle is the one shape that makes a module impossible to read on its own,
# impossible to test on its own, and impossible to move.
#
# **This is the gate Pi never built.** It built *reachability* — and a cycle is maximally
# reachable, which is why a reachability gate passes at 240,000 lines with the knot still in it.
# `magi/scripts/gate-reachable.sh` has the identical blind spot.
#
# Comments are stripped first, and this is not a detail: a module that documents its counterpart
# with `[`crate::directory::free_in`]` is describing the relationship, not depending on it. A
# gate that counted rustdoc links would have fired on the fix that removed the last real edge,
# which is the failure mode that gets a gate deleted in a week. `#[cfg(test)]` bodies go the same
# way -- a test may reach for anything, and one that does is not an architectural fact.
#
# POSIX for the same reason the others are: /bin/sh on the runner is dash.
# `GATE_ROOT` because casper and melchior keep their sources in `src`, not `crates`.
set -eu
ROOT="${GATE_ROOT:-crates}"

# Every crate root under the search root, so this reads the same in a workspace and in a repo
# that is one crate.
if [ "$ROOT" = "crates" ]; then
  roots=$(find "$ROOT" -mindepth 2 -maxdepth 2 -type d -name src -not -path '*/target/*' | sort)
else
  roots="$ROOT"
fi

found=""
for src in $roots; do
  report=$(
    # The top-level modules of this crate: `src/<name>.rs` and `src/<name>/`, minus the roots.
    names=$(
      { find "$src" -maxdepth 1 -name '*.rs' -not -name lib.rs -not -name main.rs \
             -exec basename {} .rs \;
        find "$src" -mindepth 1 -maxdepth 1 -type d -exec basename {} \;
      } | sort -u
    )

    # One `from to` edge per line, over code with comments and test bodies removed.
    for name in $names; do
      files=$(find "$src/$name.rs" "$src/$name" -name '*.rs' 2>/dev/null || true)
      [ -n "$files" ] || continue
      # shellcheck disable=SC2086
      cat $files | awk '
        # Drop a `#[cfg(test)]` item entirely, by counting braces from its opening one.
        /^[ \t]*#\[cfg\(test\)\]/ { skipping = 1; depth = 0; started = 0 }
        skipping {
          n = gsub(/\{/, "{"); depth += n
          n = gsub(/\}/, "}"); depth -= n
          if (n > 0 || depth > 0) started = 1
          if (started && depth <= 0) skipping = 0
          next
        }
        { sub(/\/\/.*$/, ""); print }
      ' | grep -o 'crate::[a-z_][a-z0-9_]*' | sed 's/crate:://' | sort -u \
      | while IFS= read -r to; do
          [ "$to" = "$name" ] && continue
          echo "$names" | grep -qx "$to" || continue
          printf '%s %s\n' "$name" "$to"
        done
    done
  )

  # What is left after every leaf is peeled off, from both ends. A node nothing leads into
  # cannot be on a cycle, and neither can one that leads nowhere; strip both until nothing more
  # comes off, and what remains is the knot and only the knot. Peeling one end only was the
  # first version, and it named every module that so much as pointed at the cycle -- thirteen
  # edges for a knot of two, which is a report nobody reads.
  left=$(printf '%s' "$report" | awk '
    { from[NR] = $1; to[NR] = $2; n = NR; node[$1] = 1; node[$2] = 1 }
    END {
      changed = 1
      while (changed) {
        changed = 0
        for (k in node) { out[k] = 0; in_[k] = 0 }
        for (i = 1; i <= n; i++) if (!gone[i]) { out[from[i]]++; in_[to[i]]++ }
        for (i = 1; i <= n; i++) {
          if (gone[i]) continue
          if (out[to[i]] == 0 || in_[from[i]] == 0) { gone[i] = 1; changed = 1 }
        }
      }
      for (i = 1; i <= n; i++) if (!gone[i]) printf "%s -> %s\n", from[i], to[i]
    }
  ')

  if [ -n "$left" ]; then
    printf '%s: these modules depend on each other in a circle:\n' "$src"
    printf '%s\n' "$left" | sed 's/^/  /'
    found=yes
  fi
done

if [ -n "$found" ]; then
  echo "gate-cycles: failed" >&2
  exit 1
fi
echo "gate-cycles: ok"
