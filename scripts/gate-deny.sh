#!/bin/sh
# The jail's mandatory-deny list is the same in magi and casper.
#
# A command's world is built twice — casper builds it for a tool call, magi for `magi.shell` — and
# the two must mask the same credential stores, or a key readable through one path is a hole whichever
# jail claims to close it. The lists are duplicated on purpose (a shared crate is the dependency
# `FAMILY.md` forbids), so this is the check that notices when one moves. At the root of magi because,
# like `gate-twins`, it needs a sibling checkout; skipped, not failed, when casper is not beside it.
set -eu

root=$(dirname "$0")/..
casper="${CASPER_CHECKOUT:-$root/../casper}"
mine="$root/crates/magi-tools/src/jail.rs"
theirs="$casper/src/jail.rs"

[ -f "$mine" ] || { echo "gate-deny: no such file: $mine" >&2; exit 2; }

if [ ! -f "$theirs" ]; then
  echo "gate-deny: no casper beside this checkout — nothing to compare against"
  exit 0
fi

# The one array literal each jail masks. Named by its first element so a reordering still compares.
list() {
  grep -oE '\[".ssh".*\]' "$1" | head -1
}

ours=$(list "$mine")
yours=$(list "$theirs")

[ -n "$ours" ] || { echo "gate-deny: magi's jail names no deny list" >&2; exit 1; }
[ -n "$yours" ] || { echo "gate-deny: casper's jail names no deny list" >&2; exit 1; }

if [ "$ours" != "$yours" ]; then
  echo "gate-deny: the mandatory-deny lists have drifted." >&2
  echo "  magi:   $ours" >&2
  echo "  casper: $yours" >&2
  echo "gate-deny: a credential store masked in one jail and not the other is a hole. See PLAN-ISOLATION.md." >&2
  exit 1
fi
echo "gate-deny: magi and casper mask the same stores."
