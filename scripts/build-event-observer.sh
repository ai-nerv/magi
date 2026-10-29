#!/usr/bin/env bash
set -euo pipefail
umask 077
mkdir -p target/event-observer
report=$(mktemp -d "$PWD/target/event-observer/build-XXXXXXXX")
digest=
finish() {
    local code=$?
    trap - EXIT
    if [[ -n $digest ]]; then
        sha256sum --check "$report/binary.sha256" > "$report/integrity.log" || code=1
    fi
    jq -n --argjson code "$code" --arg digest "$digest" --arg binary "$report/observer" \
      '{schema:1,scope:"test-event-observer-artifact",status:(if $code==0 then "CAPTURED" else "FAIL" end),
        exit_code:$code,sha256:(if $digest=="" then null else $digest end),binary:$binary,
        entry:"events::collection::worker::tests::observer::runner::entry",
        request_env:"MAGI_TEST_OBSERVER_REQUEST",production_artifact:false}' > "$report/summary.json"
    printf 'Event observer artifact: %s\n' "$report"
    exit "$code"
}
trap finish EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
bash scripts/test-cargo.sh --locked --offline --release -p magi-host --lib --no-run --message-format=json \
    > "$report/build.jsonl" 2> "$report/build.stderr"
jq -Rs '[split("\n")[] | fromjson? | select(.reason=="compiler-artifact" and .target.name=="magi_host" and .profile.test==true) |
    .executable | select(type=="string")] | unique | if length==1 then .[0] else error("observer test artifact missing or ambiguous") end' \
    "$report/build.jsonl" > "$report/selected-path.json"
selected=$(jq -er . "$report/selected-path.json")
[[ $selected == "$PWD/target/release/deps/magi_host-"* && -f $selected && -x $selected ]]
size=$(stat -c %s -- "$selected")
(( size > 0 && size <= 67108864 ))
cp -- "$selected" "$report/observer"
chmod 500 "$report/observer"
cmp -- "$selected" "$report/observer"
sha256sum "$report/observer" > "$report/binary.sha256"
digest=$(sha256sum < "$report/observer"); digest=${digest%% *}
rustc --version --verbose > "$report/rustc.txt"
