#!/bin/sh
# The family speaks one way, and a sixth wire cannot invent a sixth spelling.
#
# Five wires had grown five ways to say the same thing: `say`/`heard` on melchior's pipe,
# `to`/`from` on the surface pipe, `message` on the tool-peer pipe, `said` on the streamed
# answer, and a call envelope on the sockets. Nothing anywhere said which was meant, and nothing
# could: each was internally consistent, and each was written by somebody who had not read the
# other four.
#
# **The failure is silent.** Two of those wires exist as byte-identical copies in two
# repositories -- the surface frames, and the streamed answer -- so when two spellings drift
# nothing fails and no test goes red. The surface simply stops being answered, and it presents
# much later as "that tool never draws".
#
# Two shapes only. A **call** is answered and carries a reply; an **event** is not. Every enum
# whose variants cross a process boundary is tagged `event`. The rule is written out in this
# repository's wire module.
#
# A **data** discriminator is a different thing: a tagged enum nested *inside* a message, which
# names a shape rather than an event. Those are allowed their own key, and each is listed below
# with why. The list is the point -- adding to it is a deliberate act, and a new wire that
# invents a tag is not on it.
#
# POSIX for the same reason the others are: /bin/sh on the runner is dash.
set -eu
ROOT="${GATE_ROOT:-crates}"

# Data discriminators, and what each names. Nested inside a message; never a frame of its own.
allowed() {
  case "$1" in
    # a transport, a scope, an action, a body, an endpoint: shapes, named where they are used
    kind) return 0 ;;
    # a provider's own content blocks, which are not this family's to name
    type) return 0 ;;
    # how a tool result is drawn, carried inside the result
    shown) return 0 ;;
    # what a surface was told, carried inside an `answer` event
    answer) return 0 ;;
    # the journal on disk. A file is not a wire, and its records outlive every protocol here
    record) return 0 ;;
    # magi's own UI-to-daemon protocol: CBOR in an envelope, two halves of one program that
    # ship together, deliberately not the family shape -- a peer from another build is turned
    # away at the boundary rather than after its fields have been read
    command) return 0 ;;
    *) return 1 ;;
  esac
}

wrong=""
for file in $(find "$ROOT" -name '*.rs' -not -path '*/target/*' 2>/dev/null | sort); do
  # `#[serde(… tag = "x" …)]`, whatever order the attributes are written in.
  for found in $(grep -o 'tag *= *"[a-z_]*"' "$file" 2>/dev/null | sed 's/.*"\(.*\)"/\1/'); do
    [ "$found" = "event" ] && continue
    allowed "$found" && continue
    wrong="$wrong$file: tag = \"$found\"
"
  done
done

if [ -n "$wrong" ]; then
  echo "gate-wire: a tagged type is neither an \"event\" nor a listed data discriminator:" >&2
  printf '%s' "$wrong" | sed 's/^/  /' >&2
  echo "gate-wire: an event crossing a process boundary is tagged \"event\"; see the wire module" >&2
  echo "gate-wire: a shape nested inside a message goes on this gate's list, with its reason" >&2
  echo "gate-wire: failed" >&2
  exit 1
fi

# A reply says which revision it is written in. Without it a version skew arrives as a missing
# field at the point of use, which reads as the peer being broken rather than as a peer from a
# different build.
if grep -rql 'pub struct Reply' "$ROOT" 2>/dev/null; then
  if ! grep -rql 'pub family: u16' "$ROOT" 2>/dev/null; then
    echo "gate-wire: a Reply carries no \`family\`: a skew would arrive as a missing field" >&2
    echo "gate-wire: failed" >&2
    exit 1
  fi
fi
echo "gate-wire: ok"
