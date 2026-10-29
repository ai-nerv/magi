#!/usr/bin/env bash
set -euo pipefail
umask 077
[[ $# == 1 && ${MAGI_REQUIRE_LIVE:-} == 1 ]]
for name in MAGI_TEST_BINARY CASPER_TEST_BINARY MELCHIOR_TEST_BINARY BALTHASAR_TEST_BINARY; do
    [[ -n ${!name:-} && -x ${!name} ]]
done
artifact=$(realpath -e -- "$1")
jq -e '.schema==1 and .scope=="test-event-observer-artifact" and .status=="CAPTURED" and
  .exit_code==0 and .production_artifact==false and (.sha256|test("^[0-9a-f]{64}$")) and
  .entry=="events::collection::worker::tests::observer::runner::entry" and
  .request_env=="MAGI_TEST_OBSERVER_REQUEST"' "$artifact" >/dev/null
binary=$(jq -er .binary "$artifact")
expected=$(jq -er .sha256 "$artifact")
[[ $binary == "${artifact%/*}/observer" && -f $binary && -x $binary && ! -L $binary && $(stat -c %h "$binary") == 1 ]]
actual=$(sha256sum < "$binary"); [[ ${actual%% *} == "$expected" ]]
report=$(mktemp -d "$PWD/target/event-observer/check-XXXXXXXX")
sha256sum "$artifact" "$binary" > "$report/inputs.sha256"
code=0
env -u MAGI_TEST_OBSERVER_REQUEST "$binary" events::collection::worker::tests::observer:: \
    > "$report/tests.log" 2>&1 || code=$?
sha256sum --check "$report/inputs.sha256" > "$report/integrity.log" || code=1
jq -n --argjson code "$code" --arg sha "$expected" --arg artifact "$artifact" \
    '{schema:1,scope:"captured-event-observer-verification",status:(if $code==0 then "PASS" else "FAIL" end),
      exit_code:$code,observer_sha256:$sha,artifact_receipt:$artifact}' > "$report/summary.json"
printf 'Captured observer verification: %s\n' "$report"
tail -40 "$report/tests.log"
exit "$code"
