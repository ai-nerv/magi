#!/bin/sh
# The suite leaves nothing behind — not in the temporary directory, and not in the family's.
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

# **Short, and rooted at `/tmp` rather than under whatever `$TMPDIR` already is.** A unix socket
# path may not exceed `SUN_LEN` — 108 bytes — and several tests here bind one inside a scratch
# directory inside this root. On a developer's machine `$TMPDIR` is `/tmp` and nesting is free; on
# the runner it is `/home/runner/work/_temp`, and the same test failed with "path must be shorter
# than SUN_LEN" in the one place the gate was supposed to be proving something.
#
# Isolation comes from the directory being ours, not from where it hangs.
base=/tmp
[ -d "$base" ] && [ -w "$base" ] || base="${TMPDIR:-.}"
root=$(mktemp -d "$base/gh-XXXXXX")

# Kept rather than discarded. When the suite fails it is a test failing, not a leak, and the name
# of the test is the whole answer — a gate that printed only "exit 101" sent the reader back to
# `cargo test` to find out what it already knew.
out=$(mktemp "$base/gh-log-XXXXXX")
before=$(mktemp "$base/gh-rt-XXXXXX")
after=$(mktemp "$base/gh-rt-XXXXXX")
trap 'rm -rf "$root" "$out" "$before" "$after"' EXIT HUP INT TERM

# **The `TMPDIR` above cannot see the other place a test leaks into, and that is the place that
# did the damage.** magi's siblings put their sockets and their per-project state under
# `$XDG_RUNTIME_DIR`, which they read from their own environment — so a test that spawns one and
# then `SIGKILL`s it leaves a socket nothing will ever unlink, and a test that starts a melchior
# session leaves the project directory melchior made for it. Neither is under `TMPDIR` and neither
# was ever counted. On this machine `$XDG_RUNTIME_DIR/balthasar` had collected hundreds of
# `api@magi-*.sock` corpses and `$XDG_RUNTIME_DIR/melchior` fifteen stale projects, and the tests
# that walk those directories looking for a sibling that answers had to dial every one of them.
# That cost most of a day, misread three times as a bug in `resume_live`.
#
# Deliberately *not* solved by pointing the run at a private `XDG_RUNTIME_DIR`. That would make
# this check unfailable — no test could dirty a directory nothing else uses — and it would make
# every `*_live` test skip, because what they look for is a sibling that is really there.
runtime="${XDG_RUNTIME_DIR:-}"

# What the family's runtime directories hold, one `<program>/<entry>` per line.
#
# **`LC_ALL=C` on the sort, and this is not a tidiness point.** `comm` compares bytes; `sort` in
# a UTF-8 locale compares by collation, which ignores punctuation — so `api@mc-…` and `balthasar/…`
# come out in an order `comm` calls unsorted. It then exits non-zero, `set -e` takes the script
# with it, and the gate stops *before* the process check and before it prints a verdict. What the
# reader gets is three lines about sort order and no answer, on the run where something actually
# leaked. Found by breaking three checks at once and watching only the first one report.
runtime_listing() {
  [ -n "$runtime" ] || return 0
  for program in melchior balthasar magi; do
    if [ -d "$runtime/$program" ]; then
      ls -A "$runtime/$program" | sed "s|^|$program/|"
    fi
  done | LC_ALL=C sort
}

runtime_listing >"$before"

# The status is kept rather than acted on, because **the failing run is the one that leaks** and a
# gate that gave up at the first `exit 1` would only ever check the passing case. A suite that
# fails and cleans up is a different report from a suite that fails and does not.
#
# **`--no-fail-fast`, and without it the paragraph above was a wish.** cargo stops at the first
# test *binary* that fails, and there are forty of them; the one that failed was measured and the
# thirty-nine after it never ran at all. Six tests were made to fail on purpose to check this, and
# the run took two seconds and reached one binary. So the gate was strictest about exactly the
# case it exists for — and looked green doing it, because a leak nothing executed cannot appear.
status=0
TMPDIR="$root" cargo test --all-targets --no-fail-fast --quiet >"$out" 2>&1 || status=$?

runtime_listing >"$after"

failed=0

if [ "$status" -ne 0 ]; then
  cat "$out" >&2
  echo "gate-hermetic: the suite failed (exit $status)" >&2
  failed=1
fi

# What a *product* is entitled to leave. `magi-output-<uid>` is where a tool result too large for
# the transcript is spilled; the code that writes it expires its contents after a day, and tests
# that exercise the spill path legitimately create it. Anything else is a test that did not clean
# up after itself.
left=$(ls -A "$root" | grep -v '^magi-output-[0-9][0-9]*$' || true)

if [ -n "$left" ]; then
  echo "gate-hermetic: the suite left these in its temporary directory:" >&2
  printf '  %s\n' $left >&2
  failed=1
fi

# `comm -13` is the lines only in the second file: what the run added. Removals are not a leak —
# a sweep clearing somebody else's corpse is the behaviour under test in two places.
#
# The status is caught rather than left to `set -e`. A gate that dies in the middle of its own
# checks reports whichever ones it had reached and nothing about the rest, which is the failure
# it exists to prevent, one level up.
new=$(comm -13 "$before" "$after") || {
  echo "gate-hermetic: the runtime listings could not be compared; treating that as a failure" >&2
  new="(comm failed)"
}

if [ -n "$new" ]; then
  echo "gate-hermetic: the suite left these under $runtime:" >&2
  printf '  %s\n' $new >&2
  echo "gate-hermetic: a session started by hand during the run also lands here" >&2
  failed=1
fi

# **A leak is not only a file.** Both checks above count what is on disk, and the thing that
# escaped this suite for months was a process: `lifecycle`'s stand-in balthasar wrote down the
# shell's pid and then *forked* a ten-minute `sleep`, so the guard killed the pid it was given,
# the pid went, and the sleep one level below it ran on with init for a parent. Three per run,
# passing or failing, and nothing looked because everything anybody looked at was the process
# that was named.
#
# Asked by working directory rather than by name. Every process the suite starts inherits a cwd
# inside `$root`, which `mktemp -d` made moments ago and nothing else on the machine has ever
# been in — so this cannot mistake a developer's own editor or session for a leak, and it needs
# no list of program names to keep up to date. A scratch directory already removed still answers
# `/tmp/gh-…/mf-… (deleted)`, which begins with `$root` and is still a leak.
survivors=$(
  for entry in /proc/[0-9]*; do
    at=$(readlink "$entry/cwd" 2>/dev/null) || continue
    case "$at" in
      "$root"/*|"$root")
        # `tr` because a cmdline is NUL-separated, and an unreadable one is still a pid worth
        # naming.
        echo "${entry#/proc/} $(tr '\0' ' ' <"$entry/cmdline" 2>/dev/null)"
        ;;
    esac
  done
)

if [ -n "$survivors" ]; then
  echo "gate-hermetic: the suite left these processes running in $root:" >&2
  echo "$survivors" | sed 's|^|  |' >&2
  failed=1
fi

if [ "$failed" -ne 0 ]; then
  echo "gate-hermetic: failed" >&2
  exit 1
fi
echo "gate-hermetic: ok"
