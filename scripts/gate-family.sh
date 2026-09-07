#!/bin/sh
# Does this binary implement the family contract? See FAMILY.md.
#
# Takes a path to a program. Reports which of the contract it implements, and exits non-zero if
# it fails the floor or contradicts itself. Copied into each repository's `scripts/`, because a
# shared library between these programs is the dependency the whole arrangement exists to prevent
# — and a gate is the one kind of copy that notices its own drift, since each repo runs it against
# its own binary and the cross-repo job runs it against all four.
#
# The two rules worth having are the ones a reader cannot check: that everything advertised is
# dispatched, and that everything dispatched is advertised. A surface that offers what it refuses
# is worse than one that offers nothing, because the second is honest.
set -eu

prog=${1:-}
[ -n "$prog" ] || { echo "usage: gate-family.sh <program>" >&2; exit 2; }
command -v "$prog" >/dev/null 2>&1 || [ -x "$prog" ] || {
  echo "gate-family: no such program: $prog" >&2; exit 2; }

# Whether the coordinated half applies is **read off what the program advertises**, not passed in.
# A flag would be a second place the contract lives, and the one a caller can get wrong; asking
# the binary makes the rule self-enforcing — you must answer what you advertise, so a program
# claiming `needs` is held to it and a coordinator that claims nothing is not.
coordinated=no

name=$(basename "$prog")
fail=0
say() { printf '  %-34s %s\n' "$1" "$2"; }
bad() { say "$1" "$2"; fail=$((fail + 1)); }

echo "gate-family: $name"

# ---- the floor -----------------------------------------------------------------------------
verbs=$("$prog" verbs 2>/dev/null || true)
case "$verbs" in
  '{"ok":true'*) say "verbs" "answers in the reply shape" ;;
  '') bad "verbs" "MISSING — every family program answers this" ;;
  *) bad "verbs" "NOT the reply shape — a self-description must be parseable" ;;
esac

# `family` is the revision of the contract the reply is written in.
case "$verbs" in
  *'"family":1'*) say "family" "declares revision 1" ;;
  *) bad "family" "the reply carries no contract revision" ;;
esac

# `surface` is the revision of what a *third party* writes against, which moves for different
# reasons: adding a registrar or a field does not touch it, renaming or removing one does. Five
# registrars were published, dead and unversioned for most of this project's life; a number that
# a plugin can check is what closes that window deliberately rather than by accident.
case "$verbs" in
  *'"surface":'*) say "surface" "declares a registrar surface revision" ;;
  *) bad "surface" "nothing says what a third party is writing against" ;;
esac

client=$("$prog" client 2>/dev/null || "$prog" lua-api 2>/dev/null || true)
case "$client" in
  # A program whose surface is not reached from Lua still answers: "there is none, and here is
  # why" is parseable, where silence is not.
  '{"ok":false'*) say "client" "answers: it has none, and says why" ;;
  '') bad "client" "MISSING — a consumer cannot be made to hold a fresh copy" ;;
  *) say "client" "$(printf '%s\n' "$client" | wc -l) lines of client library" ;;
esac

# A listing is the rows, not one row that is a list. `result` is a list of values and a value is
# what the verb reports; `"result":[[` is a program that wrapped its whole listing in one row and
# then reported `n` as 1. Cheap to check and exact: casper shipped every listing it had that way
# while the other three sent theirs flat, and the coordinator that read them row by row found an
# array where a declaration belonged and concluded casper declared nothing.
case "$verbs" in
  *'"result":[['*) bad "result is rows" "the whole listing is wrapped in one row" ;;
  *) say "result is rows" "a row is a value, not the list" ;;
esac

# ---- encodings -----------------------------------------------------------------------------
cbor=$("$prog" verbs --cbor 2>/dev/null | head -c 1 | od -An -tx1 | tr -d ' \n' || true)
case "$cbor" in
  a?|b?|8?|9?) say "--cbor" "verbs answers in cbor" ;;
  '') bad "--cbor" "MISSING — the family answers in either encoding" ;;
  *) bad "--cbor" "answered, but not with a cbor map or array (first byte 0x$cbor)" ;;
esac

# ---- coordinated ---------------------------------------------------------------------------
# Held to it if it claims it. See above.
case "$verbs" in *'"verb":"needs"'*) coordinated=yes ;; esac

if [ "$coordinated" = yes ]; then
  needs=$("$prog" needs 2>/dev/null || true)
  case "$needs" in
    '{"ok":true'*) say "needs" "answers in the reply shape" ;;
    '') bad "needs" "MISSING — a coordinator cannot discover what this takes" ;;
    *) bad "needs" "answered, but not in the reply shape" ;;
  esac
  case "$needs" in
    *'"result":[['*) bad "needs is rows" "the declarations are wrapped in one row" ;;
  esac

  # A setting nobody declared must be refused *by name*, not ignored.
  refused=$(printf '%s.nonesuch_probe = 3\n' "$name" | "$prog" configure 2>/dev/null || true)
  case "$refused" in
    *nonesuch_probe*) say "configure" "refuses an unknown setting by name" ;;
    '{"ok":true'*) bad "configure" "accepted a setting nobody declared" ;;
    '') bad "configure" "MISSING — a coordinator cannot hand this its settings" ;;
    *) bad "configure" "answered, but did not name what it refused" ;;
  esac
else
  say "needs / configure" "not required — this program coordinates"
fi

# ---- advertised equals dispatched -----------------------------------------------------------
# Every verb `verbs` names for **this door** must be answered. "Answered" means anything but the
# program's own no-such-call refusal: a verb that needs arguments may fail for that reason and
# still exist.
#
# **A verb carries the door it is on.** Probing a socket verb against a command line reports it as
# advertised-and-refused when it is neither — it is correctly absent from a door that never
# claimed it. A program that does not say which door a verb is on has every verb probed here, and
# that is the older, cruder reading.
# Split on object boundaries, not on every brace: a description is prose and prose contains
# brackets, which tore one verb's name away from its own door and reported six socket verbs as
# missing from a command line that never claimed them.
listed=$(printf '%s' "$verbs" | sed 's/},{/}\n{/g' | grep -v '"door":"socket"' |
  sed -n 's/.*"verb":"\([a-z-]*\)".*/\1/p')
[ -n "$listed" ] || listed=$(printf '%s' "$verbs" | tr ',{}[]' '\n' |
  sed -n 's/^"\([a-z][a-z-]*\)"$/\1/p')

missing=""
for verb in $listed; do
  case "$verb" in
    # Verbs whose whole job is to hold a stream or take over the terminal cannot be probed by
    # running them; they are covered by the tests in their own repository.
    serve|surface|ask|run|fork) continue ;;
  esac
  out=$("$prog" "$verb" 2>&1 </dev/null || true)
  case "$out" in
    *"no such call"*|*"no such command"*|*"unrecognized subcommand"*)
      missing="$missing $verb" ;;
  esac
done
if [ -n "$missing" ]; then
  bad "advertised = dispatched" "advertised and refused:$missing"
else
  say "advertised = dispatched" "every verb it lists, it answers"
fi

echo
if [ "$fail" -gt 0 ]; then
  echo "gate-family: $name fails the contract in $fail place(s). See FAMILY.md."
  exit 1
fi
echo "gate-family: $name implements the contract."
