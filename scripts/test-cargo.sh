#!/usr/bin/env bash
set -euo pipefail

scratch=
child=
finish() {
    local code=$?
    trap - EXIT INT TERM
    if [[ -n $child ]]; then
        kill -TERM -- "-$child" 2>/dev/null || true
        for ((attempt=0; attempt<20; attempt++)); do
            kill -0 -- "-$child" 2>/dev/null || break
            sleep 0.1
        done
        kill -KILL -- "-$child" 2>/dev/null || true
        wait "$child" 2>/dev/null || true
    fi
    if [[ -n $scratch ]]; then rm -rf -- "$scratch" || code=1; fi
    exit "$code"
}
trap finish EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
command -v setsid >/dev/null || { echo 'missing test prerequisite: setsid' >&2; exit 2; }
if backend=$(command -v bwrap); then
    scratch=$(mktemp -d "${TMPDIR:-/tmp}/magi-test-backend.XXXXXX")
    cp -- "$backend" "$scratch/bwrap"
    chmod 555 "$scratch/bwrap"
    [[ $(stat -c %h "$scratch/bwrap") == 1 ]] || { echo 'test launcher has hardlink aliases' >&2; exit 2; }
    digest=$(sha256sum "$scratch/bwrap"); digest=${digest%% *}
    printf 'test launcher: %s sha256=%s\n' "$scratch/bwrap" "$digest"
    export PATH="$scratch:$PATH"
fi
env --default-signal=INT,QUIT setsid --wait cargo test "$@" &
child=$!
set +e
wait "$child"
code=$?
child=
exit "$code"
