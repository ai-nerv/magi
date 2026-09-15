#!/bin/sh
# The types magi and melchior both declare still agree.
#
# `magi-proto/src/ask.rs` and `melchior/src/mind/wire.rs` describe the same wire from two sides, and
# neither can depend on the other: `gate-independent.sh` forbids it, and `FAMILY.md` says why — a
# shared library between these programs is the dependency the whole arrangement exists to prevent.
# So they are a deliberate duplication, and this is the check that notices when one moves.
#
# **At the root, because no per-repo gate can see a sibling.** Every other gate runs inside one
# checkout against one binary. This one needs both trees, so it lives beside them and is run by
# hand or by a job that checks out all four. That is the cost of the independence rule, paid here
# rather than by giving it up.
#
# What is compared is the *wire*: field names and the serde spellings that decide what goes on it.
# Rust that differs without changing the wire — a derive, a doc comment, an ordering — is not drift.
set -eu

# Rooted at the checkout, and melchior is looked for beside it. A cross-repo check has nowhere
# else to live: no per-repo gate can see a sibling, and `gate-independent.sh` forbids the
# dependency that would make one unnecessary.
root=$(dirname "$0")/..
melchior="${MELCHIOR_CHECKOUT:-$root/../melchior}"
mine="$root/crates/magi-proto/src/ask.rs"
theirs="$melchior/src/mind/wire.rs"

[ -f "$mine" ] || { echo "gate-twins: no such file: $mine" >&2; exit 2; }

# Skipped, not failed, when melchior is not beside this checkout. CI clones one repository, and a
# gate that failed there would fail every build for a reason that is about the machine. Point
# `$MELCHIOR_CHECKOUT` at one to run it anywhere.
if [ ! -f "$theirs" ]; then
  echo "gate-twins: no melchior beside this checkout — nothing to compare against"
  exit 0
fi

# The types both sides declare. Named rather than discovered: a type only one of them has is not a
# twin, and discovering the overlap would make the gate quietly shrink as they drift apart.
TWINS="Card Wants Ask Said Refusal"

# One type's shape, as the wire sees it: its fields and variants, and every serde rename, sorted so
# that a reordering is not a difference.
shape() {
  awk "/(struct|enum) $2[ {]/,/^}/" "$1" |
    grep -oE '^\s+(pub )?[a-z_]+:|^\s+[A-Z][A-Za-z]+|rename[_a-z]* = "[a-zA-Z_]*"' |
    tr -d ' ' | sed 's/:$//' | LC_ALL=C sort
}

# Files rather than process substitution: `<(…)` is a bashism and dies at parse time under dash,
# which is what `/bin/sh` is on the machines this runs on.
ours=$(mktemp) && yours=$(mktemp)
trap 'rm -f "$ours" "$yours"' EXIT HUP INT TERM

fail=0
for twin in $TWINS; do
  shape "$mine" "$twin" >"$ours"
  shape "$theirs" "$twin" >"$yours"
  if [ ! -s "$ours" ] || [ ! -s "$yours" ]; then
    printf '  %-10s NOT FOUND in one of the two — a twin that lost its pair\n' "$twin" >&2
    fail=$((fail + 1))
    continue
  fi
  if cmp -s "$ours" "$yours"; then
    printf '  %-10s agrees\n' "$twin"
  else
    printf '  %-10s DIFFERS:\n' "$twin" >&2
    diff "$ours" "$yours" | sed 's/^/      /' >&2
    fail=$((fail + 1))
  fi
done

echo
if [ "$fail" -gt 0 ]; then
  echo "gate-twins: $fail type(s) drifted. magi is on the left, melchior on the right." >&2
  echo "gate-twins: one of them changed the wire without the other. See FAMILY.md." >&2
  exit 1
fi
echo "gate-twins: magi and melchior describe the same wire."
