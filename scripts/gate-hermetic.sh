#!/bin/sh
# The suite leaves nothing behind in the temporary directory.
#
# It used to leave a great deal: every test tidied up on its last line, and `assert!` unwinds
# straight past a trailing `remove_dir_all`. So a *failing* test always leaked, and the
# delete-then-create helpers only ever revisited their own name under their own pid, which never
# repeats. Thousands of directories accumulated across two renames of this project and nothing
# said so, because nothing looked.
#
# Run under a `TMPDIR` of its own, so the answer is about this run and not about whatever else
# the machine has in `/tmp`. That also means it can be trusted on a developer's laptop, which the
# equivalent check against the shared directory could not.
set -eu

root=$(mktemp -d "${TMPDIR:-/tmp}/gate-hermetic-XXXXXX")
trap 'rm -rf "$root"' EXIT HUP INT TERM

TMPDIR="$root" cargo test --all-targets --quiet >/dev/null

# What a *product* is entitled to leave. `magi-output-<uid>` is where a tool result too large for
# the transcript is spilled; the code that writes it expires its contents after a day, and tests
# that exercise the spill path legitimately create it. Anything else is a test that did not clean
# up after itself.
left=$(ls -A "$root" | grep -v '^magi-output-[0-9][0-9]*$' || true)

if [ -n "$left" ]; then
  echo "gate-hermetic: the suite left these behind:" >&2
  printf '  %s\n' $left >&2
  echo "gate-hermetic: failed" >&2
  exit 1
fi
echo "gate-hermetic: ok"
