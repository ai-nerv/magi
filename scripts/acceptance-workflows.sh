#!/usr/bin/env bash
set -euo pipefail
binary=$(realpath "${1:?candidate Magi binary required}")
mode=${2:-terminal}
[[ $mode == terminal || $mode == legacy ]]
ready=
if [[ $mode == terminal ]]; then
    ready=$(realpath "${3:?read-only readiness fixture required}")
fi
legacy=
if [[ $mode == legacy ]]; then
    legacy=$(realpath "${LEGACY_MAGI_TEST_BINARY:?retained Magi required}")
fi
casper=$(realpath "${CASPER_TEST_BINARY:?selected Casper required}")
memory=$(realpath "${BALTHASAR_TEST_BINARY:?selected Balthasar required}")
mind=$(realpath "${MELCHIOR_TEST_BINARY:?selected Melchior required}")
proof=$(mktemp -d /tmp/mwf.XXXXXXXX)
stage=setup
trap 'printf "workflow acceptance failed at %s, line %s\n" "$stage" "$LINENO" >&2' ERR
printf 'workflow acceptance evidence: %s\n' "$proof"
bash=$(command -v bash)
tmux_binary=$(command -v tmux)
mkdir -p "$proof"/{bin,home,config/magi,cache,data,state,run,tmp,work,requests}
if [[ -n $legacy ]]; then
    mkdir "$proof/legacy"
    cp -- "$legacy" "$proof/legacy/magi"
    chmod 500 "$proof/legacy/magi"
    cmp -- "$legacy" "$proof/legacy/magi"
    legacy="$proof/legacy/magi"
fi
chmod 700 "$proof/run"
for tool in sh bash env cat sleep jq readlink mktemp; do
    ln -s "$(command -v "$tool")" "$proof/bin/$tool"
done
cp -- "$(command -v bwrap)" "$proof/bin/bwrap"
chmod 555 "$proof/bin/bwrap"
[[ $(stat -c %h "$proof/bin/bwrap") == 1 ]]
sha256sum "$binary" "$casper" "$memory" "$mind" "$proof/bin/bwrap" > "$proof/binaries.sha256"
if [[ -n $legacy ]]; then sha256sum "$legacy" >> "$proof/binaries.sha256"; fi
if [[ -n $ready ]]; then sha256sum "$ready" >> "$proof/binaries.sha256"; fi
tmux() {
    env -i HOME="$proof/home" PATH="$proof/bin" SHELL="$bash" LANG=C.UTF-8 \
        "$tmux_binary" -u -S "$proof/tmux.sock" -f /dev/null "$@"
}
cleanup() {
    local code=$?
    trap - EXIT INT TERM
    tmux kill-server 2>/dev/null || true
    printf 'workflow acceptance exit: %s; evidence: %s\n' "$code" "$proof"
    exit "$code"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
cat > "$proof/mind" <<EOF
#!$bash
set -euo pipefail
printf '%s\n' "\$*" >> '$proof/mind-verbs.log'
case "\${1:-}" in
models)
    printf '%s\n' '{"ok":true,"refreshed":true,"failed":[],"result":[{"id":"fixture/main","provider":"fixture","name":"main","api":"openai-completions","context_window":128000,"max_output":8192,"ready":true},{"id":"fixture/helper","provider":"fixture","name":"helper","api":"openai-completions","context_window":128000,"max_output":8192,"ready":true}]}' ;;
needs) printf '%s\n' '{"ok":true,"result":[]}' ;;
ask)
    request=\$(mktemp '$proof/requests/ask.XXXXXXXX.json')
    cat > "\$request"
    last=\$(jq -r '[.context.messages[] | select(.role == "user")][-1].content | map(select(.type == "text") | .text) | join("\n")' "\$request")
    case \$(jq -er .model "\$request") in
        fixture/main) ;;
        fixture/helper) last=MEMORY-FIXTURE ;;
        *) exit 96 ;;
    esac
    case "\${last%%\$'\n'*}" in
        MEMORY-FIXTURE) printf '%s\n' '{"event":"text","text":"{\\"ops\\":[]}"}' ;;
        STALL-REQUEST) printf '%s\n' ready > '$proof/stall.ready'; exec sleep 30 ;;
        LONG-HISTORY) jq -cn '{event:"text",text:([range(0;150)|"HISTORY-\(.)"]|join("\n"))}' ;;
        AFTER-INTERRUPT) printf '%s\n' '{"event":"text","text":"INTERRUPT-RECOVERED"}' ;;
        AFTER-ERROR) printf '%s\n' '{"event":"text","text":"ERROR-RECOVERED"}' ;;
        AFTER-INVALID) printf '%s\n' '{"event":"text","text":"INVALID-RECOVERED"}' ;;
        *) printf '%s\n' '{"event":"text","text":"WORKFLOW-OK"}' ;;
    esac
    printf '%s\n' '{"event":"spent","usage":{"input":100,"output":200,"cache_read":0,"cache_write":0,"cost_micros":0}}' '{"event":"stop","reason":"end_turn"}' ;;
serve|client|verbs) exec '$mind' "\$@" ;;
*) printf 'unexpected mind verb: %s\n' "\$*" >&2; exit 97 ;;
esac
EOF
pin=$(sha256sum "$casper"); pin=${pin%% *}
cat > "$proof/config/magi/init.lua" <<EOF
magi.project = "wf"
magi.model = "fixture/main"
magi.thinking = "off"
magi.helpers = { memory = "fixture/helper" }
magi.melchior = "$proof/mind"
magi.memory = "$memory"
magi.tools = "$casper"
magi.casper_sha256 = "$pin"
magi.workflow_workspace = "selected"
EOF
mkdir -p "$proof/work/selected/.magi/skills/review"
source="$proof/work/selected/.magi/skills/review/SKILL.md"
cat > "$source" <<'EOF'
---
name: review
description: Review 日本語
---
WORKFLOW-BODY literal $SECRET `not-a-command`
EOF
cp "$source" "$proof/source.saved"
digest=$(sha256sum "$source"); digest=${digest%% *}
cat > "$proof/environment" <<EOF
#!$bash
set -euo pipefail
exec '$proof/bin/bwrap' --unshare-user --unshare-pid --unshare-net \
    --ro-bind / / --dev /dev --proc /proc --bind '$proof' '$proof' \
    --chdir '$proof/work' --die-with-parent \
    '$proof/bin/env' -i HOME='$proof/home' PATH='$proof/bin' SHELL='$bash' \
    TERM=xterm-256color LANG=C.UTF-8 TMPDIR='$proof/tmp' \
    XDG_CONFIG_HOME='$proof/config' XDG_CACHE_HOME='$proof/cache' \
    XDG_DATA_HOME='$proof/data' XDG_STATE_HOME='$proof/state' XDG_RUNTIME_DIR='$proof/run' \
    '$bash' -c 'readlink /proc/self/ns/net >> "\$1/network-namespaces"; shift; exec "\$@"' \
    fixture '$proof' "\$@"
EOF
chmod 700 "$proof/mind" "$proof/environment"
readlink /proc/self/ns/net > "$proof/host-network-namespace"
stage=headless-selection
timeout 45 "$proof/environment" "$binary" --logs "$proof/print.log" -p '/skill review literal $(touch forbidden)' \
    > "$proof/print.out" 2> "$proof/print.err"
grep -Fxq WORKFLOW-OK "$proof/print.out"
jq -es '[.[] | select(.model == "fixture/main")] | length == 1 and any(.[0].context.messages[].content[]?; .type == "text" and (.text | contains("WORKFLOW-BODY literal $SECRET `not-a-command`")) and (.text | contains("literal $(touch forbidden)")))' "$proof"/requests/*.json >/dev/null
rm "$source"
stage=headless-resume
timeout 45 "$proof/environment" "$binary" --resume -p 'confirm frozen history' \
    > "$proof/resume.out" 2> "$proof/resume.err"
grep -Fxq WORKFLOW-OK "$proof/resume.out"
jq -es '[.[] | select(.model == "fixture/main")] | length == 2 and all(.[]; any(.context.messages[].content[]?; .type == "text" and (.text | contains("WORKFLOW-BODY literal $SECRET `not-a-command`"))))' "$proof"/requests/*.json >/dev/null
cp "$proof/source.saved" "$source"
before=$(find "$proof/requests" -type f | wc -l)
stage=headless-refusals
for prompt in '/skill' '/skill missing'; do
    code=0
    timeout 45 "$proof/environment" "$binary" -p "$prompt" > "$proof/refused.out" 2> "$proof/refused.err" || code=$?
    [[ $code == 1 ]]
    grep -Eq 'workflow|/skill' "$proof/refused.err"
done
[[ $(find "$proof/requests" -type f | wc -l) == "$before" ]]
[[ ! -e "$proof/work/forbidden" ]]
if [[ $mode == legacy ]]; then
    stage=legacy-reader
    run=$(awk '/ verb: observe .* cursor 1 / { print $5; exit }' "$proof/print.log")
    [[ -n $run ]]
    printf '%s\n' "$run" > "$proof/workflow-run.txt"
    code=0
    timeout 45 "$proof/environment" "$legacy" --resume-run "$run" -p 'must not run after unsupported workflow' \
        > "$proof/legacy.out" 2> "$proof/legacy.err" || code=$?
    [[ $code == 1 ]]
    grep -Eq 'could not read it back|unsupported|unknown variant' "$proof/legacy.err"
    [[ $(find "$proof/requests" -type f | wc -l) == "$before" ]]
    ! grep -Fxq "$(cat "$proof/host-network-namespace")" "$proof/network-namespaces"
    printf '%s\n' '{"headless":true,"frozen_resume":true,"refusal_exit":true,"legacy_explicit_resume_refused":true,"legacy_model_calls":0,"external_network":false}' > "$proof/result.json"
    echo "workflow legacy explicit-resume acceptance passed: $proof"
    exit 0
fi
capture() { tmux capture-pane -p -t wf > "$proof/current.txt"; }
wait_for() {
    local wanted=$1
    for ((attempt=0; attempt<300; attempt++)); do
        capture
        if grep -Fq "$wanted" "$proof/current.txt"; then return; fi
        sleep 0.1
    done
    printf 'terminal timed out waiting for: %s\n' "$wanted" >&2
    return 1
}
type_prompt() {
    wait_ready
    tmux send-keys -t wf Escape
    wait_for '╭─ NOR'
    tmux send-keys -t wf C-c
    tmux send-keys -t wf -l i
    tmux send-keys -t wf -l "$1"
}
prompt() { type_prompt "$1"; tmux send-keys -t wf Enter; }
wait_ready() {
    "$proof/environment" "$ready" "$proof/run/terminal-$size.sock" \
        >> "$proof/readiness-$size.jsonl" 2>> "$proof/readiness-$size.err"
}
for size in 80x24 160x28; do
    stage=terminal-$size
    width=${size%x*}; height=${size#*x}
    tmux new-session -d -s wf -x "$width" -y "$height" \
        "exec '$proof/environment' '$binary' --socket '$proof/run/terminal-$size.sock' --logs '$proof/terminal-$size.log'"
    wait_for 'wf/main/'
    type_prompt '/skill re'
    wait_for 'Review 日本語'
    cp "$proof/current.txt" "$proof/completion-$size.txt"
    tmux send-keys -t wf Tab
    wait_for '/skill review'
    tmux send-keys -t wf -l ' literal $(touch forbidden)'
    tmux send-keys -t wf Enter
    wait_for 'WORKFLOW-OK'
    capture
    cp "$proof/current.txt" "$proof/provenance-$size.txt"
    grep -Fq 'WORKFLOW review' "$proof/current.txt"
    grep -Fq "$digest" "$proof/current.txt"
    grep -Fq "$proof/work/selected/.magi/skills" "$proof/current.txt"
    ! grep -Fq 'WORKFLOW-BODY' "$proof/current.txt"
    prompt LONG-HISTORY
    wait_for HISTORY-149
    tmux send-keys -t wf PageUp
    sleep 0.3
    capture
    ! grep -Fq HISTORY-149 "$proof/current.txt"
    cp "$proof/current.txt" "$proof/scrolled-$size.txt"
    prompt ':skills'
    sleep 1
    capture
    ! grep -Fq HISTORY-149 "$proof/current.txt"
    cp "$proof/current.txt" "$proof/refreshed-scrolled-$size.txt"
    tmux send-keys -t wf S-End
    wait_for HISTORY-149
    prompt '/skill missing'
    wait_for 'workflow is not in the selected catalog'
    cp "$proof/current.txt" "$proof/missing-$size.txt"
    prompt AFTER-ERROR
    wait_for ERROR-RECOVERED
    wait_ready
    before=$(jq -s '[.[] | select(.model == "fixture/main")] | length' "$proof"/requests/*.json)
    printf '%s\n' 'invalid workflow source' > "$source"
    prompt '/skill review invalid-source'
    wait_for 'workflow acquisition failed: Metadata'
    cp "$proof/current.txt" "$proof/invalid-source-$size.txt"
    wait_ready
    [[ $(jq -s '[.[] | select(.model == "fixture/main")] | length' "$proof"/requests/*.json) == "$before" ]]
    cp "$proof/source.saved" "$source"
    prompt AFTER-INVALID
    wait_for INVALID-RECOVERED
    cp "$proof/current.txt" "$proof/invalid-recovered-$size.txt"
    rm -f "$proof/stall.ready"
    prompt STALL-REQUEST
    for ((attempt=0; attempt<300; attempt++)); do
        [[ ! -f "$proof/stall.ready" ]] || break
        sleep 0.1
    done
    [[ -f "$proof/stall.ready" ]]
    tmux send-keys -t wf Escape
    wait_for '╭─ NOR'
    tmux send-keys -t wf Escape
    wait_for 'Interrupted.'
    wait_ready
    capture
    cp "$proof/current.txt" "$proof/interrupted-idle-$size.txt"
    prompt AFTER-INTERRUPT
    wait_for INTERRUPT-RECOVERED
    cp "$proof/current.txt" "$proof/recovered-$size.txt"
    tmux kill-session -t wf
done
[[ ! -e "$proof/work/forbidden" ]]
stage=cleanup-and-network
jq -es 'all(.[]; .model == "fixture/main" or .model == "fixture/helper") and any(.[]; .model == "fixture/helper")' "$proof"/requests/*.json >/dev/null
! grep -Fxq "$(cat "$proof/host-network-namespace")" "$proof/network-namespaces"
sleep 0.5
for environment in /proc/[0-9]*/environ; do
    if grep -zFxq "XDG_RUNTIME_DIR=$proof/run" "$environment" 2>/dev/null; then
        printf 'fixture process survived cleanup: %s\n' "$environment" >&2
        exit 1
    fi
done
printf '%s\n' '{"headless":true,"frozen_resume":true,"refusal_exit":true,"widths":[80,160],"completion":true,"provenance":true,"manual_scroll":true,"error_recovery":true,"invalid_source_recovery":true,"invalid_source_model_calls":0,"interrupt_recovery":true,"external_network":false,"paid_calls":0,"leaks":0}' > "$proof/result.json"
echo "workflow headless and terminal acceptance passed: $proof"
