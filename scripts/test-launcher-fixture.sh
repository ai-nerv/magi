#!/usr/bin/env bash
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")/.." && pwd -P)
scratch=$(mktemp -d /tmp/magi-launcher-test.XXXXXX)
wrapper=
finish() {
    if [[ -n $wrapper ]]; then
        kill -TERM "$wrapper" 2>/dev/null || true
        wait "$wrapper" 2>/dev/null || true
    fi
    rm -rf -- "$scratch"
}
trap finish EXIT
mkdir "$scratch/bin" "$scratch/tmp"
printf '#!/bin/sh\nexit 99\n' > "$scratch/bin/bwrap"
chmod 755 "$scratch/bin/bwrap"
ln "$scratch/bin/bwrap" "$scratch/bin/alias"
cat > "$scratch/bin/cargo" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
[[ $# == 3 && $1 == test && $2 == --lib && $3 == 'spaces "quotes' ]]
[[ -z $(trap -p INT) && -z $(trap -p QUIT) ]]
backend=$(command -v bwrap)
[[ $backend == "$TMPDIR"/magi-test-backend.*/bwrap ]]
[[ $(stat -c %h "$backend") == 1 && $(stat -c %a "$backend") == 555 ]]
cmp -- "$backend" "$PROBE_ORIGINAL"
printf '%s\n' "$backend" > "$PROBE_CAPTURE"
case "$PROBE_MODE" in
    pass) exit 0 ;;
    fail) exit 37 ;;
    cancel) echo $$ > "$PROBE_PID"; touch "$PROBE_READY"; exec sleep 600 ;;
    *) exit 99 ;;
esac
SH
chmod +x "$scratch/bin/cargo"
export PATH="$scratch/bin:$PATH" TMPDIR="$scratch/tmp"
export PROBE_ORIGINAL="$scratch/bin/bwrap" PROBE_CAPTURE="$scratch/capture"
export PROBE_PID="$scratch/pid" PROBE_READY="$scratch/ready"
for mode in pass fail; do
    code=0
    PROBE_MODE=$mode "$root/scripts/test-cargo.sh" --lib 'spaces "quotes' > "$scratch/$mode.log" 2>&1 || code=$?
    expected=0; [[ $mode != fail ]] || expected=37
    [[ $code == "$expected" ]] || { cat "$scratch/$mode.log"; exit 1; }
    [[ -f $PROBE_CAPTURE && ! -e $(cat "$PROBE_CAPTURE") ]]
    [[ -z $(find "$TMPDIR" -mindepth 1 -print -quit) ]]
    [[ $(stat -c %h "$PROBE_ORIGINAL") == 2 && $(stat -c %a "$PROBE_ORIGINAL") == 755 ]]
done
PROBE_MODE=cancel "$root/scripts/test-cargo.sh" --lib 'spaces "quotes' > "$scratch/cancel.log" 2>&1 &
wrapper=$!
for ((attempt=0; attempt<200; attempt++)); do
    [[ ! -f $PROBE_READY ]] || break
    kill -0 "$wrapper" || { cat "$scratch/cancel.log"; exit 1; }
    sleep 0.01
done
[[ -f $PROBE_READY ]]
read -r worker < "$PROBE_PID"
kill -TERM "$wrapper"
code=0; wait "$wrapper" || code=$?
wrapper=
[[ $code == 143 && ! -d /proc/$worker ]]
[[ -z $(find "$TMPDIR" -mindepth 1 -print -quit) ]]
echo 'test launcher: capture, argv, signals, status, cancellation and cleanup PASS'
