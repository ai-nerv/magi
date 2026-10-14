#!/bin/sh
set -eu
binary=$(realpath "${1:?magi binary required}")
memory=$(command -v balthasar)
proof=$(mktemp -d /tmp/magi-env-proof-XXXXXXXX)
session="magi-env-proof-$$"
tmux() { command tmux -L "$session" -f /dev/null "$@"; }
trap 'tmux kill-server 2>/dev/null || true' EXIT INT TERM
mkdir -p "$proof/home" "$proof/config/magi" "$proof/cache" "$proof/data" "$proof/state" "$proof/runtime" "$proof/work"
chmod 700 "$proof/runtime"
printf '%s\n' private-sentinel > "$proof/expected"
cat > "$proof/mind" <<EOF
#!/bin/sh
case "\$1" in
  models)
    ready=false
    [ -n "\$OPENROUTER_API_KEY" ] && ready=true
    printf '%s\n' '{"ok":true,"refreshed":true,"failed":[],"result":[{"id":"fixture/one","provider":"fixture","name":"one","api":"openai-completions","context_window":200000,"max_output":1000,"ready":READY,"needs":"OPENROUTER_API_KEY","vision":false,"reasons":false}]}' | sed "s/READY/\$ready/"
    ;;
  ask)
    cat > /dev/null
    [ "\$OPENROUTER_API_KEY" = "\$(cat '$proof/expected')" ] || exit 2
    [ "\$MAGI_ENV_PATH" = new:old:tail ] || exit 3
    printf '%s\n' '{"event":"text","text":"environment verified"}' '{"event":"stop","reason":"end_turn"}'
    ;;
  *) printf '%s\n' '{"ok":true,"result":[]}' ;;
esac
EOF
chmod 700 "$proof/mind"
cat > "$proof/config/magi/init.lua" <<EOF
magi.model = "fixture/one"
magi.melchior = "$proof/mind"
magi.memory = "$memory"
magi.tools = "$proof/absent-tools"
magi.env = { OPENROUTER_API_KEY = "", MAGI_ENV_PATH = "old:tail" }
EOF
cat > "$proof/home/exports file.sh" <<'EOF'
echo forbidden > "$HOME/execute-marker"
f() { export OPENROUTER_API_KEY=wrong; }
export OPENROUTER_API_KEY=updated:$OPENROUTER_API_KEY
export UNSAFE=$(touch "$HOME/execute-marker")
EOF
cat > "$proof/launch" <<EOF
#!/bin/sh
cd '$proof/work'
exec env -i HOME='$proof/home' PATH='/usr/bin:/bin' TERM=xterm-256color \
  XDG_CONFIG_HOME='$proof/config' XDG_CACHE_HOME='$proof/cache' XDG_DATA_HOME='$proof/data' \
  XDG_STATE_HOME='$proof/state' XDG_RUNTIME_DIR='$proof/runtime' \
  '$binary' --logs '$proof/magi.log'
EOF
chmod 700 "$proof/launch"
tmux new-session -d -s "$session" -x 120 -y 30 "$proof/launch; printf '%s\\n' \$? > '$proof/exit'; sleep 60"
wait_for() {
  n=0
  while [ "$n" -lt 150 ]; do
    tmux capture-pane -p -t "$session" > "$proof/current.txt"
    if grep -Fq "$1" "$proof/current.txt"; then return; fi
    n=$((n + 1))
    sleep 0.1
  done
  echo "environment live acceptance timed out: $1; evidence: $proof" >&2
  exit 1
}
send() {
  tmux send-keys -t "$session" -l "$1"
  tmux send-keys -t "$session" Enter
}
prompt() {
  tmux send-keys -t "$session" Escape
  tmux send-keys -t "$session" -l i
  send "$1"
}
wait_for 'OPENROUTER_API_KEY'
send ':env OPENROUTER_API_KEY=private-sentinel MAGI_ENV_PATH=new:$MAGI_ENV_PATH'
wait_for 'Session variables set'
tmux capture-pane -p -t "$session" > "$proof/env.txt"
prompt 'verify environment'
wait_for 'environment verified'
tmux capture-pane -p -t "$session" > "$proof/first-turn.txt"
printf '%s\n' updated:private-sentinel > "$proof/expected"
send ':source "$HOME/exports file.sh"'
wait_for 'Skipped 1 unsupported'
prompt 'verify sourced environment'
n=0
while [ "$n" -lt 150 ]; do
  tmux capture-pane -p -t "$session" > "$proof/source.txt"
  if [ "$(grep -Fc 'environment verified' "$proof/source.txt")" -ge 2 ]; then break; fi
  n=$((n + 1)); sleep 0.1
done
[ "$n" -lt 150 ] || { echo "source request failed; evidence: $proof" >&2; exit 1; }
[ ! -e "$proof/home/execute-marker" ]
send ':q'
sleep 0.5
if grep -RFq private-sentinel "$proof/data" "$proof/state" "$proof/magi.log" "$proof/env.txt" "$proof/first-turn.txt" "$proof/source.txt"; then
  echo "environment value leaked into history, logs or UI captures; evidence: $proof" >&2
  exit 1
fi
printf '%s\n' '{"env":true,"source":true,"expansion":true,"execution":false,"secret_leak":false}' > "$proof/result.json"
echo "environment live acceptance passed: $proof"
