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

# ---- one frame cap, checked on read and on write (FAMILY.md, Frames) -----------------------------
# Only the code a binary runs: a file under tests/, a `tests.rs`, a file some `#[cfg(test)]` names
# as its module, and everything below a file's own `#[cfg(test)]` are left out.
TEST_MODS=$(find "$ROOT" -name '*.rs' -not -path '*/target/*' -exec awk '
  FNR == 1 { attrs = "" }
  /^[ \t]*#\[/ { attrs = attrs $0; next }
  attrs ~ /cfg\(test\)/ && match($0, /mod +[a-z_0-9]+ *;/) {
    named = substr($0, RSTART, RLENGTH); sub(/mod +/, "", named); sub(/ *;/, "", named)
    print named
  }
  { attrs = "" }
' {} + 2>/dev/null | sort -u)

test_only() {
  case "$1" in */tests/*|*/tests.rs) return 0 ;; esac
  printf '%s\n' "$TEST_MODS" | grep -qx "$(basename "$1" .rs)"
}

runs() {
  find "$ROOT" -name '*.rs' -path '*/src/*' -not -path '*/target/*' 2>/dev/null | sort |
    while IFS= read -r file; do
      test_only "$file" || printf '%s\n' "$file"
    done
}

# The code of `$1` above its own `#[cfg(test)]`, numbered.
shipped() {
  awk '/^#\[cfg\(test\)\]/ { exit } { printf "%d:%s\n", NR, $0 }' "$1"
}

CODEC="$ROOT/magi-ipc/src/codec.rs"
broken=""
if ! grep -q 'pub const MAX_FRAME_BYTES: usize = 16 \* 1024 \* 1024;' "$CODEC" 2>/dev/null; then
  broken="${broken}$CODEC: MAX_FRAME_BYTES is not 16 MiB
"
fi
if ! grep -q '"cap": 16777216' tests/wire-corpus/cases.json 2>/dev/null; then
  broken="${broken}tests/wire-corpus/cases.json: the corpus names another cap
"
fi
# A length compared against the cap, by any of the names it goes by.
CAP_HELD='[<>]=? *([a-z_]+::)*(MAX_FRAME_BYTES|FRAME_CAP|CAP)([^A-Z_]|$)'
capped=$(for file in $(runs); do
  # A cap is spelled once, in the codec; every other one names it.
  [ "$file" = "$CODEC" ] ||
    shipped "$file" | grep -E 'const [A-Z_]*(FRAME|CAP|RECV)[A-Z_]*: *usize *= *[0-9]' |
      sed "s|^|$file:|; s|$| (a cap spelled again; name MAX_FRAME_BYTES)|"
  # A declared length is held to the cap within four lines of being read, before any allocation.
  shipped "$file" | awk -F: -v file="$file" -v cap="$CAP_HELD" '
    /from_be_bytes\(/ { at = $1; open = 1; next }
    open && $0 ~ cap { open = 0 }
    open && $1 - at >= 4 { print file ":" at ": a length read and never held to the cap"; open = 0 }
    END { if (open) print file ":" at ": a length read and never held to the cap" }
  '
  # A length written was held to the cap in the ten lines before it.
  shipped "$file" | awk -F: -v file="$file" -v cap="$CAP_HELD" '
    { seen[$1] = $0 }
    /to_be_bytes\(/ {
      held = 0
      for (back = $1 - 10; back < $1; back++)
        if (seen[back] ~ cap) held = 1
      if (!held) print file ":" $1 ": a length written and never held to the cap"
    }
  '
done)
[ -z "$capped" ] || broken="${broken}${capped}
"

# The Lua client frames nothing by hand: `magi.stream`'s `call` does, in Rust, against the cap.
LIB="$ROOT/magi-lua/lua/magi.lua"
if grep -nE ':recv\(|:send\(|be32|16777216' "$LIB" >/dev/null 2>&1; then
  broken="${broken}$LIB: frames by hand rather than through handle:call
"
fi
if ! grep -q 'FRAME_CAP' "$ROOT/magi-lua/src/stream/call.rs" 2>/dev/null; then
  broken="${broken}$ROOT/magi-lua/src/stream/call.rs: handle:call is not held to the cap
"
fi

# ---- the runtime directory (FAMILY.md, Where sockets live) -----------------------------------------
RUNTIME="$ROOT/magi-ipc/src/runtime.rs"
for held in 'filter(|v| !v.is_empty())' 'is_absolute()' 'mode(0o700)' 'symlink_metadata' \
            'mode() & 0o077 == 0' 'uid() == uid()'; do
  grep -qF "$held" "$RUNTIME" 2>/dev/null ||
    broken="${broken}$RUNTIME: the rule no longer says \`$held\`
"
done

# Where a program may make a directory, and why each place may. Everything else asks the rule.
made_here() {
  case "$1" in
    # the rule itself: 0700, then lstat
    */magi-ipc/src/runtime.rs) return 0 ;;
    # a file tool's write makes its parents; the jail's /tmp lives in the user's cache
    */magi-tools/src/ops.rs) return 0 ;;
    # the acknowledged-packages manifest, in the configuration directory
    */magi-lua/src/acknowledged.rs) return 0 ;;
    # chosen.json, in the configuration directory
    */magi-host/src/remember.rs) return 0 ;;
    # the prompt history, in the state directory, and a log file the person named
    */magi-cli/src/history.rs|*/magi-cli/src/logging.rs) return 0 ;;
    # a test's scratch, made 0700
    */magi-model/src/scratch.rs|*/magi-testkit/src/*) return 0 ;;
    *) return 1 ;;
  esac
}

for file in $(runs); do
  if shipped "$file" | grep -q 'std::env::var[_os]*("XDG_RUNTIME_DIR")'; then
    [ "$file" = "$RUNTIME" ] ||
      broken="${broken}$file: reads XDG_RUNTIME_DIR itself; ask magi_ipc::runtime_base or trusted_xdg
"
  fi
  if shipped "$file" | grep -qE 'create_dir_all\(|create_dir\(|DirBuilder::new\(\)'; then
    made_here "$file" ||
      broken="${broken}$file: makes a directory outside the runtime rule; use private_dir(s)
"
  fi
done

if [ -n "$broken" ]; then
  echo "gate-wire: the frame cap or the runtime-directory rule is broken:" >&2
  printf '%s\n' "$broken" | sed '/^$/d; s/^/  /' >&2
  echo "gate-wire: failed" >&2
  exit 1
fi
echo "gate-wire: ok"
