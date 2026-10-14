#!/bin/sh
set -eu
binary=$(realpath "${1:?magi binary required}")
memory=$(command -v balthasar)
mind=$(command -v melchior)
proof=$(mktemp -d /tmp/magi-agents-proof-XXXXXXXX)
session="magi-agents-proof-$$"
tmux() { command tmux -L "$session" -f /dev/null "$@"; }
cleanup() {
  if [ -f "$proof/root.pid" ]; then kill "$(cat "$proof/root.pid")" 2>/dev/null || true; fi
  tmux kill-server 2>/dev/null || true
}
trap cleanup EXIT INT TERM
mkdir -p "$proof/home" "$proof/config/magi" "$proof/cache" "$proof/data" "$proof/state" "$proof/runtime" "$proof/work"
chmod 700 "$proof/runtime"
worker_model=fixture/worker-with-a-complete-long-model-name-not-truncated
cat > "$proof/mind" <<EOF
#!/bin/sh
case "\$1" in
  models)
    printf '%s\n' '{"ok":true,"refreshed":true,"failed":[],"result":[{"id":"fixture/main","provider":"fixture","name":"main","api":"openai-completions","context_window":128000,"max_output":1000,"ready":true},{"id":"$worker_model","provider":"fixture","name":"worker","api":"openai-completions","context_window":128000,"max_output":1000,"ready":true}]}'
    ;;
  ask)
    request=\$(cat)
    model=\$(printf '%s' "\$request" | /usr/bin/jq -r '.model')
    prefix=ROOT
    [ "\$model" = '$worker_model' ] && prefix=CHILD
    /usr/bin/jq -cn --arg prefix "\$prefix" '{event:"text",text:([range(0;200)|"\(\$prefix)-HISTORY-\(.)"]|join("\n"))}'
    printf '%s\n' '{"event":"spent","usage":{"input":450,"output":900,"cache_read":0,"cache_write":0,"cost_micros":1000}}' '{"event":"stop","reason":"end_turn"}'
    ;;
  *) exec '$mind' "\$@" ;;
esac
EOF
cat > "$proof/config/magi/init.lua" <<EOF
magi.project = "proof"
magi.model = "fixture/main"
magi.melchior = "$proof/mind"
magi.memory = "$memory"
magi.tools = "$proof/absent-tools"
magi.agents = { reviewer = { model = "$worker_model", description = "offline tree acceptance" } }
EOF
cat > "$proof/environment" <<EOF
#!/bin/sh
exec env -i HOME='$proof/home' PATH='/usr/bin:/bin' TERM=xterm-256color \
  XDG_CONFIG_HOME='$proof/config' XDG_CACHE_HOME='$proof/cache' XDG_DATA_HOME='$proof/data' \
  XDG_STATE_HOME='$proof/state' XDG_RUNTIME_DIR='$proof/runtime' "\$@"
EOF
cat > "$proof/launch" <<EOF
#!/bin/sh
cd '$proof/work'
echo \$\$ > '$proof/root.pid'
exec '$proof/environment' '$binary' --logs '$proof/root.log'
EOF
chmod 700 "$proof/mind" "$proof/environment" "$proof/launch"
tmux new-session -d -s "$session" -x 160 -y 28 "$proof/launch; sleep 60"
wait_for() {
  n=0
  while [ "$n" -lt 200 ]; do
    tmux capture-pane -p -t "$session" > "$proof/current.txt"
    if grep -Fq "$1" "$proof/current.txt"; then return; fi
    n=$((n + 1)); sleep 0.1
  done
  echo "agents acceptance timed out: $1; evidence: $proof" >&2
  exit 1
}
send() { tmux send-keys -t "$session" -l "$1"; tmux send-keys -t "$session" Enter; }
prompt() {
  tmux send-keys -t "$session" Escape
  tmux send-keys -t "$session" -l i
  send "$1"
}
wait_for 'proof/main/'
prompt 'generate root history'
wait_for 'ROOT-HISTORY-199'
root=$(find "$proof/runtime/melchior/proof" -name '*.role' -type f | head -1)
root=$(basename "$root" .role)
cd "$proof/work"
child=$("$proof/environment" env MAGI_SESSION_PID="$(cat "$proof/root.pid")" \
  MAGI_MELCHIOR_PROJECT=proof MAGI_MELCHIOR_ID="$root" \
  "$binary" fork --role reviewer 'generate child history')
printf '%s\n' "$child" > "$proof/child.id"
sleep 3
send ':agents'
wait_for '2 agents'
tmux send-keys -t "$session" Down
wait_for "model: $worker_model"
wait_for 'last input:'
wait_for 'thinking:'
tmux capture-pane -p -t "$session" > "$proof/tree-160x28.txt"
tmux send-keys -t "$session" Enter
wait_for 'CHILD-HISTORY-199'
tmux capture-pane -p -t "$session" > "$proof/tail-160x28.txt"
! grep -Fq 'CHILD-HISTORY-0 ' "$proof/tail-160x28.txt"
tmux send-keys -t "$session" PageUp
sleep 0.5
tmux capture-pane -p -t "$session" > "$proof/scrolled-160x28.txt"
! grep -Fq 'CHILD-HISTORY-199' "$proof/scrolled-160x28.txt"
sleep 2.5
tmux capture-pane -p -t "$session" > "$proof/scrolled-stable-160x28.txt"
! grep -Fq 'CHILD-HISTORY-199' "$proof/scrolled-stable-160x28.txt"
tmux send-keys -t "$session" S-End
wait_for 'CHILD-HISTORY-199'
tmux resize-window -t "$session" -x 80 -y 24
send ':agents'
wait_for 'model: fixture/worker'
wait_for 'thinking:'
tmux capture-pane -p -t "$session" > "$proof/tree-80x24.txt"
tmux send-keys -t "$session" Enter
wait_for 'CHILD-HISTORY-199'
tmux capture-pane -p -t "$session" > "$proof/tail-80x24.txt"
printf '%s\n' '{"tree_details":true,"tail_attach":true,"manual_scroll":true,"widths":[80,160],"paid_model_calls":0}' > "$proof/result.json"
echo "agents terminal acceptance passed: $proof"
