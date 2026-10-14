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
[ -n "$prog" ] || { echo "usage: gate-family.sh <program> [serve arguments…]" >&2; exit 2; }
command -v "$prog" >/dev/null 2>&1 || [ -x "$prog" ] || {
  echo "gate-family: no such program: $prog" >&2; exit 2; }
# What follows the program is how it serves a socket, for the live probe at the end.
shift

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
# **A verb carries the door it is on.** Probing a socket or tool verb against a command line
# reports it as advertised-and-refused when it is neither — it is correctly absent from a door
# that never claimed it. A program that does not say which door a verb is on has every verb
# probed here, and that is the older, cruder reading.
# Split on object boundaries, not on every brace: a description is prose and prose contains
# brackets, which tore one verb's name away from its own door and reported six socket verbs as
# missing from a command line that never claimed them.
listed=$(printf '%s' "$verbs" | sed 's/},{/}\n{/g' |
  grep -v '"door":"socket"' | grep -v '"door":"tool"' |
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

# ---- a verb it does not have is still a reply ------------------------------------------------
# The first thing any caller does to a sibling it has not met. An argument parser rejects an
# unknown subcommand before the program sees it — usage on stderr, exit 2, nothing on stdout —
# which is indistinguishable from the binary being absent.
unknown=$("$prog" definitely-not-a-verb-xyzzy 2>/dev/null </dev/null || true)
unknown_status=0
"$prog" definitely-not-a-verb-xyzzy >/dev/null 2>&1 </dev/null || unknown_status=$?
case "$unknown" in
  '{"ok":false'*)
    if [ "$unknown_status" -eq 0 ]; then
      say "an unknown verb" "refused in the reply shape, at exit 0"
    else
      bad "an unknown verb" "right shape, wrong exit ($unknown_status) — a refusal exits 0"
    fi ;;
  '') bad "an unknown verb" "nothing on stdout, exit $unknown_status — a caller cannot tell this from a missing binary" ;;
  *) bad "an unknown verb" "not the reply shape — a refusal is a reply" ;;
esac

# ---- both encodings, on every verb ------------------------------------------------------------
# Only the argument parser's own rejection counts. A verb may fail for want of an argument and
# still have taken the flag.
refused_flag=""
for verb in $listed; do
  case "$verb" in
    serve|surface|ask|run|fork) continue ;;
  esac
  for how in --json --cbor; do
    said=$("$prog" "$verb" "$how" 2>&1 </dev/null || true)
    case "$said" in
      *"unexpected argument"*|*"unrecognized option"*|*"unknown flag"*|*"invalid option"*)
        refused_flag="$refused_flag $verb$how" ;;
    esac
  done
done
if [ -n "$refused_flag" ]; then
  bad "--json and --cbor" "the flag was rejected outright:$refused_flag"
else
  say "--json and --cbor" "every verb takes both"
fi

# ---- a door it can open ------------------------------------------------------------------------
# A socket verb on a program with no `serve` is advertised-and-refused wearing a label that hides
# it: the command-line probe skips it as correctly-absent, and nothing can ever connect to check.
if printf '%s' "$verbs" | grep -q '"door":"socket"'; then
  if printf '%s' "$verbs" | sed 's/},{/}\n{/g' | grep -v '"door":"socket"' | grep -q '"verb":"serve"'; then
    say "a door it can open" "it advertises a socket and answers serve"
  else
    bad "a door it can open" "socket verbs, but no serve on the command line to open one"
  fi
else
  say "a door it can open" "no socket verbs, so no socket to account for"
fi

# The same rule for the tool door, which is the one nothing here can probe: it answers for a live
# session and the gate has none. What can be checked is that the way in exists.
if printf '%s' "$verbs" | grep -q '"door":"tool"'; then
  if printf '%s' "$verbs" | sed 's/},{/}\n{/g' | grep '"door":"cli"' | grep -q '"verb":"tool"'; then
    say "a tool door it can open" "it advertises tool verbs and answers tool"
  else
    bad "a tool door it can open" "tool verbs, but no tool on the command line to reach them"
  fi
else
  say "a tool door it can open" "no tool verbs, so no tool door to account for"
fi

# ---- a live socket answers a bad frame with a refusal ---------------------------------------
# Run when the caller says how the program serves: `gate-family.sh <program> serve …`. It is
# started alone, with a clean environment and a runtime directory of its own, which `{run}` in an
# argument names; `$GATE_SERVE_STDIN` is the first line it reads, for a serve handed a key before
# it binds. The first socket that appears there is sent the wire corpus's cap+1 header and
# zero-length frame, and a call from a newer family. Each must come back as a refusal frame, not
# as a closed connection, and the newer family's refusal must name it.
corpus="$(cd "$(dirname "$0")/.." && pwd)/tests/wire-corpus/server"

# The bytes in file `$1`, sent to socket `$2` and half-closed; what came back, on stdout.
ask() {
  if command -v socat >/dev/null 2>&1; then
    socat -t 5 - "UNIX-CONNECT:$2" <"$1" 2>/dev/null || true
  else
    nc -U -N -w 5 "$2" <"$1" 2>/dev/null || true
  fi
}

# One frame holding `$1`, written to file `$2`.
frame() {
  n=$(printf '%s' "$1" | wc -c)
  printf "$(printf '\\%03o\\%03o\\%03o\\%03o' $((n >> 24 & 255)) $((n >> 16 & 255)) \
    $((n >> 8 & 255)) $((n & 255)))" >"$2"
  printf '%s' "$1" >>"$2"
}

live() {
  if ! command -v socat >/dev/null 2>&1 && ! nc -h 2>&1 | grep -q -- '-U'; then
    bad "a live socket" "nothing here speaks to a unix socket: install socat or OpenBSD nc"
    return
  fi
  if [ ! -f "$corpus/oversize.header" ] || [ ! -f "$corpus/zero-length.frame" ]; then
    bad "a live socket" "no wire corpus at $corpus"
    return
  fi
  probe=$(mktemp -d)
  mkdir -m 700 "$probe/run" "$probe/home"
  mkfifo "$probe/in"
  for arg in "$@"; do
    shift
    case "$arg" in "{run}"*) arg="$probe/run${arg#"{run}"}" ;; esac
    set -- "$@" "$arg"
  done
  env -i PATH="$PATH" HOME="$probe/home" TMPDIR="$probe" XDG_RUNTIME_DIR="$probe/run" \
    XDG_CONFIG_HOME="$probe/home/config" XDG_DATA_HOME="$probe/home/data" \
    XDG_CACHE_HOME="$probe/home/cache" XDG_STATE_HOME="$probe/home/state" \
    "$prog" "$@" <"$probe/in" >"$probe/out" 2>&1 &
  served=$!
  exec 3>"$probe/in"
  [ -z "${GATE_SERVE_STDIN:-}" ] || printf '%s\n' "$GATE_SERVE_STDIN" >&3

  sock=""
  tries=0
  while [ -z "$sock" ] && [ "$tries" -lt 50 ] && kill -0 "$served" 2>/dev/null; do
    sleep 0.2 2>/dev/null || sleep 1
    sock=$(find "$probe/run" -type s 2>/dev/null | head -n 1)
    tries=$((tries + 1))
  done

  if [ -z "$sock" ]; then
    bad "a live socket" "\`$name $*\` bound nothing to probe: $(head -c 300 "$probe/out")"
  else
    frame '{"call":"verbs","family":999}' "$probe/newer.frame"
    for sent in "cap+1:$corpus/oversize.header" "zero length:$corpus/zero-length.frame" \
                "a newer family:$probe/newer.frame"; do
      label=${sent%%:*}
      back=$(ask "${sent#*:}" "$sock" | tail -c +5)
      if [ -z "$back" ]; then
        bad "live: $label" "the connection closed with no reply"
      elif ! printf '%s' "$back" | grep -q '"ok":false'; then
        bad "live: $label" "answered, but not with a refusal: $(printf '%s' "$back" | head -c 120)"
      elif [ "$label" = "a newer family" ] && ! printf '%s' "$back" | grep -q 'family 999'; then
        bad "live: $label" "refused, but without naming the family"
      else
        say "live: $label" "answered with a refusal frame"
      fi
    done
  fi

  exec 3>&-
  kill -TERM "$served" 2>/dev/null || true
  wait "$served" 2>/dev/null || true
  rm -rf "$probe"
}

if [ "$#" -gt 0 ]; then
  live "$@"
elif printf '%s' "$verbs" | grep -q '"door":"socket"'; then
  say "a live socket" "not probed: no serve command named"
else
  # A program with no socket verbs serves no family socket. magi's one socket is its host socket,
  # its own CBOR protocol for a front end (FAMILY.md), so a family frame is not its to refuse.
  say "a live socket" "skipped: $name serves no family socket, only its own front-end protocol"
fi

echo
if [ "$fail" -gt 0 ]; then
  echo "gate-family: $name fails the contract in $fail place(s). See FAMILY.md."
  exit 1
fi
echo "gate-family: $name implements the contract."
