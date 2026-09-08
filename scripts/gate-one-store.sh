#!/bin/sh
# balthasar holds this session's history, and magi keeps no copy of it.
#
# magi used to write a JSONL journal per session under `~/.local/share/magi/sessions/`, beside
# balthasar's store rather than instead of it. Two stores is one store and a copy that goes
# stale: a session resumed from the stale one resumes into something that half-happened, and —
# because the fallback was silent — nobody could tell which of the two they had been using.
#
# The journal is gone. This is what stops it coming back, because it would come back as a
# reasonable-looking change: a `BufWriter` in `magi-journal` reads as a performance fix, and a
# `.jsonl` path reads as a debugging aid. Both are a second store.
#
# What is checked, and why each is checked here rather than left to review:
#
#   1. `magi-journal` touches no file. It is the in-memory window on what balthasar holds, and
#      the type is still called `Journal` — which is exactly the invitation this guards.
#   2. Nothing anywhere builds a `.jsonl` path. That extension is what the old store used, and a
#      new one under a different name would be caught by (1) only if it lived in that crate.
#
# Tests are searched too. A test that writes a transcript to disk is a test asserting the thing
# this forbids, and the ones that did were rewritten rather than exempted — see
# `magi-cli/tests/oneshot.rs`, which now asserts the *absence*.
#
# POSIX for the same reason the others are: /bin/sh on the runner is dash.
set -eu
ROOT="${GATE_ROOT:-crates}"

failed=""

# 1. The transcript crate holds no writer of any kind.
store=$(
  find "$ROOT/magi-journal" -name '*.rs' -not -path '*/target/*' 2>/dev/null \
  | sort | while IFS= read -r file; do
    # `std::fs`, `File`, `OpenOptions`, `BufWriter` and `write!` to anything: the whole vocabulary
    # of putting bytes somewhere that outlives the process.
    if grep -nE 'std::fs|File::|OpenOptions|BufWriter|fs::write|fs::read' "$file"; then
      printf '  ^ in %s\n' "$file"
    fi
  done
)
if [ -n "$store" ]; then
  printf '%s\n' "$store" >&2
  printf 'magi-journal touches the filesystem: it is the window on balthasar, not a store\n' >&2
  failed="yes"
fi

# 2. Nobody builds a path to a transcript file.
#
# The literal extension, anywhere in the tree. `scripts/` is not searched: this file names it in
# the prose above, and a gate that failed on its own explanation would be a gate nobody keeps.
#
# `tests/fixtures/` is not a store and is not searched. What lives there is a recorded stream of
# `HarnessEvent`s that `magi fake-host` replays, which is how the UI is developed without a model
# — an *input*, checked in, that magi reads and never writes. The distinction the rule is drawing
# is where a session's history goes, and a fixture is not anybody's history.
journals=$(grep -rn '\.jsonl' "$ROOT" --include='*.rs' | grep -v 'tests/fixtures/' || true)
if [ -n "$journals" ]; then
  printf '%s\n' "$journals" >&2
  printf 'a .jsonl transcript path is a second store: balthasar holds the history\n' >&2
  failed="yes"
fi

if [ -n "$failed" ]; then
  echo "gate-one-store: failed" >&2
  exit 1
fi
echo "gate-one-store: ok"
