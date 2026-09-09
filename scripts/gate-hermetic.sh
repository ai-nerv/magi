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
#
# **Every level, not the top one.** This listed each program's directory with `ls -A` and stopped
# there, and the entries under `$runtime/magi` are one directory per *project* — so a session that
# left its socket behind in a project directory that already existed added nothing to the listing
# and the diff was empty. That is the leak this check was written for: `sweep()` unlinks a corpse
# socket only in the directory the next magi in *that* project opens, so one left anywhere else
# stays until the machine reboots, and a probe that dials it reads a corpse as a live daemon.
# Seven were sitting under `$XDG_RUNTIME_DIR/magi` on this machine — left by sessions started by
# hand rather than by the suite, which is why the gate had never been red about them, and exactly
# what it would have missed had the suite left them.
runtime_listing() {
  [ -n "$runtime" ] || return 0
  for program in melchior balthasar magi; do
    if [ -d "$runtime/$program" ]; then
      find "$runtime/$program" -mindepth 1 | sed "s|^$runtime/||"
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
#
# **The profile is whichever one the rest of the run already built.** A leak is a leak in either,
# so the only thing the choice decides is whether the workspace is compiled a second time into a
# second target directory. This was pinned to debug: `.make.lua` builds `--release` in every
# recipe and says why, so `make verify` compiled everything twice and `target/debug` had grown to
# 47GB beside a 3.3GB `target/release` for a second copy of the same suite. Release is the
# default for that reason; CI's own verify step is a debug one and sets `GATE_PROFILE=debug`, so
# neither place pays for two.
case "${GATE_PROFILE:-release}" in
  release) profile=--release ;;
  debug) profile= ;;
  *) echo "gate-hermetic: GATE_PROFILE must be release or debug" >&2; exit 1 ;;
esac

status=0
# shellcheck disable=SC2086
TMPDIR="$root" cargo test --all-targets $profile --no-fail-fast --quiet >"$out" 2>&1 || status=$?

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
  echo "gate-hermetic: so does a sibling repository's suite, if one is running at the same time" >&2
  failed=1
fi

# **A leak is not only a file.** Both checks above count what is on disk, and the thing that
# escaped this suite for months was a process: `lifecycle`'s stand-in balthasar wrote down the
# shell's pid and then *forked* a ten-minute `sleep`, so the guard killed the pid it was given,
# the pid went, and the sleep one level below it ran on with init for a parent. Three per run,
# passing or failing, and nothing looked because everything anybody looked at was the process
# that was named.
#
# Asked by `$root` rather than by name. `mktemp -d` made it moments ago and nothing else on the
# machine has ever carried that string, so neither question below can mistake a developer's own
# editor or session for a leak, and neither needs a list of program names to keep up to date.
#
# **Two questions, because the first one alone was blind and looked thorough.** The cwd check was
# written on the premise that every process the suite starts inherits a cwd inside `$root`, and
# that is not what cargo does: a test binary runs with its cwd set to the *package* directory, so
# a child inherits `…/nerv/magi` unless the test explicitly called `current_dir`. Over half the
# files here that spawn something never call it. Measured rather than reasoned about — a `sleep`
# started with this repository as its cwd and `TMPDIR=$root` in its environment was invisible to
# the cwd predicate and named immediately by the environment one. The `sleep 600` this caught when
# it landed was caught because `lifecycle.rs` happens to set `current_dir`; that is a property of
# one fixture, not of the check.
#
# The environment is the predicate that does not depend on the test: `TMPDIR` is set for the whole
# run and every descendant inherits it, and the `XDG_*` directories the tests point at their own
# scratches are all under it too. A scratch directory already removed still answers
# `/tmp/gh-…/mf-… (deleted)` on cwd, which begins with `$root` and is still a leak, so the first
# question is kept as well.
survivors=$(
  {
    for entry in /proc/[0-9]*; do
      at=$(readlink "$entry/cwd" 2>/dev/null) || continue
      case "$at" in
        "$root"/*|"$root") echo "${entry#/proc/}" ;;
      esac
    done
    # One pass over `/proc` rather than a fork per pid. `-s` because a process that ends between
    # the glob and the read is not an error, and `-a` because an environ is NUL-separated and
    # grep would otherwise call it binary and print nothing useful.
    grep -lsa -- "$root" /proc/[0-9]*/environ 2>/dev/null \
      | sed 's|^/proc/||; s|/environ$||'
  } | LC_ALL=C sort -un | while IFS= read -r pid; do
    # `tr` because a cmdline is NUL-separated, and an unreadable one is still a pid worth naming.
    echo "$pid $(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null)"
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
