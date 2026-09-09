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
PLUGINS="$ROOT/magi-lua/src/plugins.rs"
DISCOVERED="$ROOT/magi-cli/src/config/discovered.rs"
TRUST="$ROOT/magi-cli/src/config/mod.rs"

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

# ---- one VM, in one place ---------------------------------------------------------------------
built=$(sites 'Lua::full()\|Lua::new()\|Lua::new_with\|unsafe_new')
count=$(printf '%s' "$built" | grep -c . || true)
if [ "$count" -ne 1 ]; then
  echo "gate-sandbox: a Lua VM is built in $count places; there is one sandbox and it is applied once:" >&2
  printf '%s\n' "$built" | sed 's/^/  /' >&2
  fail=1
elif ! printf '%s' "$built" | grep -q "^$ENGINE:"; then
  echo "gate-sandbox: the VM is no longer built in $ENGINE:" >&2
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

# ---- the removals are still the removals --------------------------------------------------------
# Field by field, because losing one is losing a specific thing: `execute` and `popen` are the
# spawn, `remove`/`rename`/`tmpname` are writes that go round the `Ops` seam where path checking
# lives, `exit` ends the daemon from inside a config file, and `io` goes wholesale because every
# remaining member of it opens a file.
for gone in execute exit remove rename tmpname setlocale; do
  holds "$SANDBOX" "\"$gone\"" "os.$gone is no longer removed from the VM" || fail=1
done
for gone in io package dofile loadfile require; do
  holds "$SANDBOX" "\"$gone\"" "the global \`$gone\` is no longer removed from the VM" || fail=1
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
# project's own file may *assign*. `confine` is the wall, `allow` is what happens without asking,
# and `trusted` decides which files these rules apply to at all — a `.magi.lua` that could set the
# last one would exempt itself from the other two.
#
# The list and the comparison are both checked. `PRIVILEGED_SETTINGS` with nothing consulting it
# is a constant, and `altered()` over an empty list refuses nothing.
#
# Read out of the declaration rather than looked for in the file. `"confine"` also appears in
# `boolean("confine")` further down, so a whole-file grep would have found the reader of the
# setting and reported the guard on it as present.
listed=$(code "$TRUST" | sed -n '/^const PRIVILEGED_SETTINGS/,/];/p')
if [ -z "$listed" ]; then
  echo "gate-sandbox: nothing names the settings a project file may not assign" >&2
  fail=1
fi
for setting in confine allow trusted; do
  printf '%s\n' "$listed" | grep -q "\"$setting\"" || {
    echo "gate-sandbox: \`magi.$setting\` is no longer privileged; a project file could set it" >&2
    fail=1
  }
done
holds "$TRUST" 'machine.altered(' \
  "nothing compares the privileged settings against what they were before .magi.lua ran" || fail=1
# Fatal rather than printed: by the time this is noticed the value the project file wanted is the
# value the config holds, and there is nothing sensible to carry on with.
code "$TRUST" | grep -A6 'machine.altered(' | grep -q 'return Err' || {
  echo "gate-sandbox: a project file altering a privileged setting no longer ends the run" >&2
  fail=1
}

[ "$fail" -eq 0 ] || { echo "gate-sandbox: failed" >&2; exit 1; }
echo "gate-sandbox: ok"
