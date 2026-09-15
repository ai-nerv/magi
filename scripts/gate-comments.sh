#!/bin/sh
# Comments describe code. They do not argue with it.
#
# A comment block may not exceed 20% of the lines of the implementation it describes. Measured per
# file, because a block belongs to the file it is in and an average over a crate hides a module
# header ten times the length of its re-exports.
#
# Two allowances, both narrow:
#
#   `SAFETY:` blocks do not count. An `unsafe` block is required to carry one, the requirement is
#   not this gate's to override, and the note has to say enough to be checked against.
#
#   A file may always have FLOOR comment lines, whatever its size. Twenty percent of a four-line
#   re-export module is not a sentence, and a module that cannot say what it is has been made
#   worse rather than tidier.
set -eu

ROOT="${GATE_ROOT:-crates}"
LIMIT="${GATE_COMMENT_PCT:-20}"
FLOOR="${GATE_COMMENT_FLOOR:-3}"

over=$(
  find "$ROOT" -name '*.rs' -type f -not -path '*/target/*' -not -path '*/xtra/*' | sort |
    while IFS= read -r file; do
      awk -v file="$file" -v limit="$LIMIT" -v floor="$FLOOR" '
        # A run of comment lines is one block, held until it ends so a multi-line `SAFETY:` note
        # is exempt whole. Exempt from the `SAFETY:` line onward only: anything above it is an
        # ordinary comment that happens to sit next to one, and counting it is what stops a
        # paragraph being parked above a safety note to escape the budget.
        function settle() {
          comments += (safe ? safe - 1 : held)
          held = 0; safe = 0
        }
        { line = $0; sub(/^[ \t]+/, "", line) }
        line ~ /^\/\// {
          held++
          if (line ~ /SAFETY:/ && !safe) safe = held
          next
        }
        { settle() }
        line == "" { next }
        { code++ }
        END {
          settle()
          allowed = code * limit / 100
          if (allowed < floor) allowed = floor
          if (comments > allowed)
            printf "%s: %d comment lines against %d of code (%d allowed)\n",
                   file, comments, code, allowed
        }
      ' "$file"
    done
)

if [ -n "$over" ]; then
  echo "gate-comments: these say more about the code than the code does:" >&2
  printf '%s\n' "$over" | sed 's/^/  /' >&2
  echo "gate-comments: describe the block; cut the argument, the history and the persuasion" >&2
  exit 1
fi
echo "gate-comments: ok"
