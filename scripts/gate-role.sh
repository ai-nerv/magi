#!/bin/sh
# Does this binary fill a role? See ROLES.md.
#
# Takes a role and a program. Reports which of the role that program implements, and exits non-zero
# if it refuses a core verb. Extensions are reported and do not fail: a memory layer that cannot
# plan a window is a memory layer that does not compact, not a broken one.
#
# One script for every role rather than one per role, because the difference between them is two
# lists and the checking is identical. Copied into each repository's `scripts/` for the reason
# `gate-family.sh` gives: a gate is the one kind of copy that notices its own drift.
#
# The family contract is assumed, not re-checked — `gate-family.sh` is that, and a program failing
# it fails this too for reasons this script would report confusingly.
set -eu

role=${1:-}
prog=${2:-}
[ -n "$role" ] && [ -n "$prog" ] || {
  echo "usage: gate-role.sh <memory|tools|model> <program>" >&2; exit 2; }
command -v "$prog" >/dev/null 2>&1 || [ -x "$prog" ] || {
  echo "gate-role: no such program: $prog" >&2; exit 2; }

# The lists, and the only place they live. `verbs` and `client` are the family floor and belong to
# `gate-family.sh`, so they are not repeated here.
case "$role" in
  memory)
    core="observe replay"
    extra="amend recall remember forget why scroll plan used outcome model resume sessions" ;;
  tools)
    core="tools run"
    extra="surface acknowledge" ;;
  model)
    core="models ask"
    extra="" ;;
  *) echo "gate-role: no such role: $role — see ROLES.md" >&2; exit 2 ;;
esac

name=$(basename "$prog")
fail=0
say() { printf '  %-14s %-10s %s\n' "$1" "$2" "$3"; }

echo "gate-role: $name as $role"

verbs=$("$prog" verbs 2>/dev/null || true)
case "$verbs" in
  '{"ok":true'*) ;;
  *) echo "gate-role: $name does not answer verbs — run gate-family.sh first" >&2; exit 1 ;;
esac

# Advertised, not probed. A role verb takes arguments this script has no business inventing —
# `observe` streams a turn into a scrollback — so running one to see if it answers would either
# write into somebody's memory or fail for the wrong reason. What `verbs` names, the family
# contract already holds the program to answering: `gate-family.sh` checks advertised equals
# dispatched, so an advertised verb is a dispatched one.
advertises() {
  printf '%s' "$verbs" | sed 's/},{/}\n{/g' | grep -q "\"verb\":\"$1\""
}

for verb in $core; do
  if advertises "$verb"; then
    say "$verb" "core" "answered"
  else
    say "$verb" "core" "MISSING — this program cannot fill $role"
    fail=$((fail + 1))
  fi
done

held=0
absent=""
for verb in $extra; do
  if advertises "$verb"; then
    held=$((held + 1))
  else
    absent="$absent $verb"
  fi
done

total=0
for verb in $extra; do total=$((total + 1)); done
if [ -n "$extra" ]; then
  say "extensions" "" "$held of $total"
  [ -z "$absent" ] || say "" "" "not offered:$absent"
fi

echo
if [ "$fail" -gt 0 ]; then
  echo "gate-role: $name cannot fill $role — $fail core verb(s) missing. See ROLES.md."
  exit 1
fi
echo "gate-role: $name fills $role."
