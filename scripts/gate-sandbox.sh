#!/bin/sh
# A Lua VM in magi is a sandboxed one, and there is only one place that builds it.
#
# magi runs declarations it did not write. `~/.config/magi/plugin/` is whatever was dropped in
# there, `site/pack/*/start/*/plugin/` arrived by being fetched, and a project's `.magi.lua`
# comes with a checkout. `Lua::full()` hands a VM the whole standard library — `os.execute`,
# `io.popen`, `dofile` — and a config with those does not need a process tool at all, which
# would make the process transport a stylistic preference rather than the only way out.
#
# **The defence is one line in one constructor, and that is the risk.** `magi-lua/src/sandbox.rs`
# has tests that prove the removals work; what they cannot prove is that every VM went through
# the constructor that applies them. A second `Lua::full()` written somewhere reasonable — a
# helper, a benchmark, a `configure` path that wanted its own VM — is a full standard library
# with no test anywhere going red. So the count is held at one, and its file is named.
#
# Comments are stripped before every match, and a call is matched with its paren. Both are
# lessons from casper's version of this gate, which was green twice on defences that were not
# there: `grep -q 'Lua::full()'` matched the sandbox module's own documentation, and a bare
# symbol name matched an identifier somebody had renamed to `symbol_REMOVED`.
#
# POSIX for the same reason the others are: /bin/sh on the runner is dash.
set -eu
ROOT="${GATE_ROOT:-crates}"

ENGINE="$ROOT/magi-lua/src/engine.rs"
SANDBOX="$ROOT/magi-lua/src/sandbox.rs"
STREAM="$ROOT/magi-lua/src/stream.rs"
FAMILY="$ROOT/magi-tools/src/family.rs"
PLUGINS="$ROOT/magi-lua/src/plugins.rs"
DISCOVERED="$ROOT/magi-cli/src/config/discovered.rs"
TRUST="$ROOT/magi-cli/src/config/mod.rs"
PROJECT="$ROOT/magi-cli/src/config/project.rs"

fail=0

# What a file says with its comments taken off. `//` only: this workspace has no `/* */`.
code() {
  awk '{ sub(/\/\/.*$/, ""); print }' "$1"
}

# Every file matching a pattern in the code rather than in the prose, as `path:line`.
sites() {
  # shellcheck disable=SC2086
  find $ROOT -name '*.rs' -not -path '*/target/*' | sort | while IFS= read -r file; do
    code "$file" | grep -n "$1" | sed "s|^|$file:|" || true
  done
}

# A file exists and its code — not its documentation — contains a pattern.
holds() {
  [ -f "$1" ] || { echo "gate-sandbox: $1 is gone; $3" >&2; return 1; }
  code "$1" | grep -q "$2" || { echo "gate-sandbox: $3" >&2; return 1; }
}

# The code of one item, from the line matching a pattern to the first line that closes at column
# zero.
#
# **A whole-file grep answered the wrong question here, and passed.** `needs_acknowledging()` is
# also called by `installed()`, which lists what `magi acknowledge` would take — so replacing the
# loader's guard with `if false` left the call in the file and left this gate green. Checked by
# breaking it, which is the only way that showed.
body() {
  code "$1" | awk -v pat="$2" '
    !inside && $0 ~ pat { inside = 1 }
    inside { print }
    inside && /^}/ { exit }
  '
}

# One declaration, from the line naming it to the line that ends it, which may be the same line.
decl() {
  code "$1" | awk -v pat="$2" '
    !inside && $0 ~ pat { inside = 1 }
    inside { print }
    inside && /;[[:space:]]*$/ { exit }
  '
}

# A file's code before its tests.
shipped() {
  code "$1" | sed '/#\[cfg(test)\]/,$d'
}

# ---- two VMs, in one place ---------------------------------------------------------------------
# The machine's VM and a project file's data-only VM, both built in the engine and nowhere else.
built=$(sites 'Lua::full()\|Lua::core()\|Lua::empty()\|Lua::new()\|Lua::new_with\|unsafe_new')
count=$(printf '%s' "$built" | grep -c . || true)
if [ "$count" -ne 2 ]; then
  echo "gate-sandbox: a Lua VM is built in $count places; there are two and each is sandboxed once:" >&2
  printf '%s\n' "$built" | sed 's/^/  /' >&2
  fail=1
elif printf '%s\n' "$built" | grep -v "^$ENGINE:" | grep -q .; then
  echo "gate-sandbox: a VM is built outside $ENGINE:" >&2
  printf '%s\n' "$built" | sed 's/^/  /' >&2
  fail=1
fi

# ---- and that place trims it -------------------------------------------------------------------
# In the constructor's own body, not merely somewhere in the file: an `apply` that had moved to a
# test helper or a second `impl` would be a full standard library everywhere it matters.
body "$ENGINE" 'pub fn new[(][)] -> Self' | grep -q 'sandbox::apply(' || {
  echo "gate-sandbox: $ENGINE builds a VM and does not apply the sandbox to it" >&2
  fail=1
}

# ---- a project's VM is data -----------------------------------------------------------------------
# The core library only, the project sandbox applied, a memory ceiling, and no socket or
# filesystem lent.
project_vm=$(body "$ENGINE" 'pub fn project[(]')
for wanted in 'Lua::core()' 'sandbox::apply_project(' 'set_memory_limit(' 'install_project('; do
  printf '%s\n' "$project_vm" | grep -qF "$wanted" || {
    echo "gate-sandbox: the project VM no longer calls $wanted" >&2
    fail=1
  }
done
installed=$(body "$ENGINE" 'fn install_project[(]')
for lent in 'stream::table' 'fs::table' 'json::table' '"stream"' '"fs"'; do
  if printf '%s\n' "$installed" | grep -qF "$lent"; then
    echo "gate-sandbox: the project VM is lent $lent" >&2
    fail=1
  fi
done
for gone in load loadstring dofile loadfile require collectgarbage io os package debug; do
  decl "$SANDBOX" '^const REMOVED_FROM_PROJECTS' | grep -q "\"$gone\"" || {
    echo "gate-sandbox: a project VM keeps \`$gone\`" >&2
    fail=1
  }
done

# ---- every VM runs on a budget ---------------------------------------------------------------------
# One way to run a chunk, and it counts fuel; `execute` and `finish` count none.
if code "$ENGINE" | grep -q '\.execute::<\|\.finish(\|execute_async'; then
  echo "gate-sandbox: $ENGINE runs a chunk without a budget" >&2
  fail=1
fi
body "$ENGINE" 'fn run_within[(]' | grep -q 'Fuel::with(' || {
  echo "gate-sandbox: chunks are no longer run a slice of fuel at a time" >&2
  fail=1
}
body "$ENGINE" 'fn run_within[(]' | grep -q 'left <= 0' || {
  echo "gate-sandbox: nothing stops a chunk that has spent its budget" >&2
  fail=1
}
body "$ENGINE" 'pub fn new[(][)] -> Self' | grep -q 'set_memory_limit(Some(MEMORY))' || {
  echo "gate-sandbox: the config VM is built with no memory ceiling" >&2
  fail=1
}
body "$ENGINE" 'fn run_within[(]' | grep -q 'self.over_memory()' || {
  echo "gate-sandbox: nothing stops a chunk that holds more than its ceiling" >&2
  fail=1
}

# ---- a VM dials the family and nothing else ----------------------------------------------------
# By the family's own directory names, never by the runtime directory alone.
body "$STREAM" '^fn dialable[(]' | grep -q 'family_socket(' || {
  echo "gate-sandbox: magi.stream no longer asks whether a path is one of the family's sockets" >&2
  fail=1
}
if shipped "$STREAM" | grep -q 'temp_dir()\|XDG_RUNTIME_DIR'; then
  echo "gate-sandbox: magi.stream decides what is dialable by directory again" >&2
  fail=1
fi
programs=$(decl "$FAMILY" '^pub const PROGRAMS')
for program in magi casper melchior memory balthasar; do
  printf '%s\n' "$programs" | grep -q "\"$program\"" || {
    echo "gate-sandbox: \`$program\` is no longer one of the family's socket directories" >&2
    fail=1
  }
done
expected=5
listed_programs=$(printf '%s\n' "$programs" | grep -o '"[a-z]*"' | grep -c . || true)
[ "$listed_programs" -eq "$expected" ] || {
  echo "gate-sandbox: the family's socket directories are $listed_programs, not $expected" >&2
  fail=1
}
body "$FAMILY" '^pub fn family_socket_under[(]' | grep -q 'is_socket()' || {
  echo "gate-sandbox: a dialable path is no longer checked to be a socket" >&2
  fail=1
}
body "$FAMILY" '^pub fn family_socket_under[(]' | grep -q 'uid()' || {
  echo "gate-sandbox: a dialable socket is no longer checked to be this uid's" >&2
  fail=1
}

# ---- the removals are still the removals --------------------------------------------------------
# Field by field, because losing one is losing a specific thing: `execute` and `popen` are the
# spawn, `remove`/`rename`/`tmpname` are writes that go round the `Ops` seam where path checking
# lives, `exit` ends the daemon from inside a config file, and `io` goes wholesale because every
# remaining member of it opens a file.
for gone in execute exit remove rename tmpname setlocale; do
  holds "$SANDBOX" "\"$gone\"" "os.$gone is no longer removed from the VM" || fail=1
done
for gone in io package dofile loadfile require debug; do
  holds "$SANDBOX" "\"$gone\"" "the global \`$gone\` is no longer removed from the VM" || fail=1
done

# ---- and nothing is queued for a finalizer -----------------------------------------------------
for vm in '^pub fn apply[(]' '^pub fn apply_project[(]'; do
  body "$SANDBOX" "$vm" | grep -q 'guard(ctx, "setmetatable", finalized)' || {
    echo "gate-sandbox: a VM's setmetatable no longer refuses __gc" >&2
    fail=1
  }
  body "$SANDBOX" "$vm" | grep -q 'hide_userdata_metatables(ctx)' || {
    echo "gate-sandbox: a VM's getmetatable hands out the metatable errors share" >&2
    fail=1
  }
done
if code "$ENGINE" | grep -q 'run_finalizers('; then
  echo "gate-sandbox: $ENGINE runs finalizers, which no budget meters" >&2
  fail=1
fi

# ---- bytecode never enters ---------------------------------------------------------------------
body "$ENGINE" 'fn run_within[(]' | grep -q 'sandbox::precompiled(' || {
  echo "gate-sandbox: a chunk is no longer checked for bytecode before it is loaded" >&2
  fail=1
}
body "$SANDBOX" '^pub fn apply[(]' | grep -q 'guard(ctx, "load", binary)' || {
  echo "gate-sandbox: \`load\` no longer refuses bytecode" >&2
  fail=1
}

# ---- a value leaves the VM as JSON within bounds -------------------------------------------------
CONVERT="$ROOT/magi-lua/src/convert.rs"
for wanted in 'self.open.insert(table)' 'self.nodes > NODES' 'self.bytes > BYTES' \
              'self.open.len() >= DEEPEST'; do
  shipped "$CONVERT" | grep -qF "$wanted" || {
    echo "gate-sandbox: converting a Lua value to JSON no longer checks $wanted" >&2
    fail=1
  }
done

# ---- fetched code still has to be let in --------------------------------------------------------
# The other half of the boundary. A file under `site/pack/` arrived from somewhere else and can
# change between one run and the next, so it runs once somebody has said it may.
#
# Both ends are checked: the rule that says which trust needs it, and the loader that asks.
# Either alone passes with the defence gone — a `needs_acknowledging` nobody consults is a
# function, not a gate.
holds "$PLUGINS" 'Trust::Installed' \
  "nothing marks an installed package as needing acknowledgement" || fail=1
loader=$(body "$DISCOVERED" '^pub fn run[(]')
printf '%s\n' "$loader" | grep -q 'needs_acknowledging()' || {
  echo "gate-sandbox: the loader no longer asks whether a discovered file has been acknowledged" >&2
  echo "gate-sandbox: fetched declarations would then run on sight, which is the whole risk" >&2
  fail=1
}
printf '%s\n' "$loader" | grep -q 'acknowledged::cleared(' || {
  echo "gate-sandbox: the loader asks, and nothing checks the answer against the manifest" >&2
  fail=1
}

# ---- and a checkout cannot govern the session ---------------------------------------------------
# The third door into the same room. The sandbox says what a config *can call*; this says what a
# project's own file may *assign*: an allowlist, held to exactly the settings that choose a model or
# how the screen looks.
#
# Read out of the declaration rather than looked for in the file.
allowed=$(decl "$PROJECT" '^pub const PROJECT_SETTINGS')
if [ -z "$allowed" ]; then
  echo "gate-sandbox: nothing names the settings a project file may assign" >&2
  fail=1
fi
for setting in model thinking ui; do
  printf '%s\n' "$allowed" | grep -q "\"$setting\"" || {
    echo "gate-sandbox: \`magi.$setting\` is missing from what a project file may set" >&2
    fail=1
  }
done
listed_settings=$(printf '%s\n' "$allowed" | grep -o '"[a-z_]*"' | grep -c . || true)
[ "$listed_settings" -eq 3 ] || {
  echo "gate-sandbox: a project file may set $listed_settings settings; the list is three" >&2
  fail=1
}
# The privileged settings, by name: the wall, what runs without asking, which files the rules
# apply to, the environment every spawn gets, which program fills a role, and every program's table.
for setting in confine allow trusted isolation may_spawn mode ask deny env roles agents agent_talk \
               project system helpers tools memory melchior casper balthasar; do
  if printf '%s\n' "$allowed" | grep -q "\"$setting\""; then
    echo "gate-sandbox: \`magi.$setting\` is privileged and a project file may set it" >&2
    fail=1
  fi
done
holds "$PROJECT" 'HELD_WITHIN' \
  "nothing keeps the safety model out of what a project file chooses" || fail=1
# A refusal ends the run.
body "$PROJECT" '^pub fn apply[(]' | grep -q 'return Err' || {
  echo "gate-sandbox: a project file setting what it may not no longer ends the run" >&2
  fail=1
}
code "$TRUST" | grep -q 'project::apply(&mut engine, &local)?' || {
  echo "gate-sandbox: the configuration no longer runs a project file apart, or drops its refusal" >&2
  fail=1
}
holds "$PLUGINS" 'is_absolute()' \
  "an empty or relative \`magi.trusted\` entry would vouch for every directory" || fail=1

[ "$fail" -eq 0 ] || { echo "gate-sandbox: failed" >&2; exit 1; }
echo "gate-sandbox: ok"
