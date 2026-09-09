#!/bin/sh
# The workspace's denials apply to every crate in it, and nothing takes them back.
#
# magi has no `unsafe` anywhere and no `pre_exec`: the one place that needed `PR_SET_PDEATHSIG`
# has balthasar make the call on itself through `rustix` instead. `unsafe_code = "deny"` is what
# keeps that true, and it is one line in the root manifest — but a workspace lint reaches a crate
# only if that crate asks for it. A twelfth crate added without
#
#     [lints]
#     workspace = true
#
# compiles with no denial at all: `unsafe`, `unwrap()`, `dbg!` and `todo!()` are all allowed in
# it, and nothing anywhere says so. Three lines are easy to forget and invisible when they are.
#
# The denials are named individually rather than counted, for the reason the other gates give:
# each is here because something shipped the thing it forbids. `unwrap_used` is the one that
# earns its keep daily; `unsafe_code` is the one that would be quietly re-allowed by whoever next
# wants a `pre_exec`.
#
# POSIX for the same reason the others are: /bin/sh on the runner is dash.
set -eu
ROOT="${GATE_ROOT:-crates}"
MANIFEST="${GATE_MANIFEST:-Cargo.toml}"

fail=0

# What a file says with its comments taken off, so a denial named in prose is not mistaken for
# one in force. `#` for TOML, `//` for Rust; neither appears in the other's syntax here.
code() {
  case "$1" in
    *.toml) sed 's/#.*$//' "$1" ;;
    *) awk '{ sub(/\/\/.*$/, ""); print }' "$1" ;;
  esac
}

# ---- the root still denies ----------------------------------------------------------------------
for denied in unsafe_code unused; do
  code "$MANIFEST" | grep -q "^$denied *= *\"deny\"\|^$denied *= *{ *level *= *\"deny\"" || {
    echo "gate-lints: the workspace no longer denies \`$denied\`" >&2
    fail=1
  }
done
for denied in unwrap_used dbg_macro todo; do
  code "$MANIFEST" | grep -q "^$denied *= *\"deny\"" || {
    echo "gate-lints: the workspace no longer denies \`clippy::$denied\`" >&2
    fail=1
  }
done

# ---- and every member asks for them ---------------------------------------------------------------
# The members are read out of the manifest rather than globbed: a crate directory that is not a
# member is not built, and a member whose directory is missing is a different failure.
members=$(code "$MANIFEST" | sed -n '/^members *= *\[/,/^]/p' | sed -n 's/.*"\([^"]*\)".*/\1/p')
if [ -z "$members" ]; then
  echo "gate-lints: no workspace members could be read out of $MANIFEST" >&2
  fail=1
fi
for member in $members; do
  at="$member/Cargo.toml"
  if [ ! -f "$at" ]; then
    echo "gate-lints: $member is a workspace member with no manifest" >&2
    fail=1
    continue
  fi
  # The table and its one line, together. `workspace = true` also appears under `[package]`,
  # where it means the version and the edition and says nothing about lints.
  code "$at" | sed -n '/^\[lints\]/,/^\[/p' | grep -q '^workspace *= *true' || {
    echo "gate-lints: $at does not take the workspace lints; nothing is denied in that crate" >&2
    fail=1
  }
done

# ---- and no source takes one back -----------------------------------------------------------------
# An `#[allow(unsafe_code)]` is how the denial ends: one attribute, one function, and the property
# the family's `pre_exec`-free spawn surface rests on is gone with no manifest change to notice.
# There is no island here — magi has no `unsafe` at all — so the honest rule is "nowhere".
allowed=$(
  # shellcheck disable=SC2086
  find $ROOT -name '*.rs' -not -path '*/target/*' | sort | while IFS= read -r file; do
    code "$file" \
      | grep -n 'allow(unsafe_code\|expect(unsafe_code\|allow(clippy::unwrap_used\|allow(clippy::dbg_macro\|allow(clippy::todo' \
      | sed "s|^|$file:|" || true
  done
)
if [ -n "$allowed" ]; then
  echo "gate-lints: a denial is allowed back in:" >&2
  printf '%s\n' "$allowed" | sed 's/^/  /' >&2
  fail=1
fi

[ "$fail" -eq 0 ] || { echo "gate-lints: failed" >&2; exit 1; }
echo "gate-lints: ok"
