#!/usr/bin/env bash
set -euo pipefail
root=$(cd -- "$(dirname -- "$0")/.." && pwd -P)
oslo=$(command -v oslo)
scratch=$(mktemp -d /tmp/magi-config-install.XXXXXX)
trap 'rm -rf -- "$scratch"' EXIT
project="$scratch/project with spaces"
mkdir -p "$project/config/nested" "$scratch/home" "$scratch/fail-bin"
cp "$root/.make.lua" "$root/Cargo.toml" "$project/"
git -C "$project" init -q
source="$project/config/alpha.txt"
dest="$scratch/home/config/magi"
mkdir -p "$dest/nested"
printf 'nested source\n' > "$project/config/nested/source.txt"
printf 'user only\n' > "$dest/nested/user-only.txt"
run() {
    (cd "$project" && env -i PATH="${fixture_path:-$PATH}" HOME="$scratch/home" \
        XDG_CONFIG_HOME="$scratch/home/config" timeout --kill-after=2s 30s \
        "$oslo" make configs "$@") > "$scratch/out" 2>&1
}
same() { cmp -s -- "$1" "$2" || { cat "$scratch/out"; echo 'configuration mismatch' >&2; exit 1; }; }
printf 'initial\n' > "$source"
touch -t 202601010000 "$source"
run
same "$source" "$dest/alpha.txt"
same "$project/config/nested/source.txt" "$dest/nested/source.txt"
grep -Fx 'user only' "$dest/nested/user-only.txt" >/dev/null
touch -t 202601020000 "$dest/alpha.txt"
run
[[ $(stat -c %Y "$source") == $(stat -c %Y "$dest/alpha.txt") ]]
printf 'source-newer\n' > "$source"
touch -t 202601030000 "$source"
run
same "$source" "$dest/alpha.txt"
printf 'destination-newer\n' > "$dest/alpha.txt"
touch -t 202601040000 "$dest/alpha.txt"
run
same "$source" "$dest/alpha.txt"
grep -Fx 'destination-newer' "$source" >/dev/null
grep -F '<-' "$scratch/out" | grep -F 'came back' >/dev/null
printf 'source-tie\n' > "$source"
printf 'destination-tie\n' > "$dest/alpha.txt"
touch -t 202601050000 "$source" "$dest/alpha.txt"
run
grep -Fx 'source-tie' "$dest/alpha.txt" >/dev/null
for newer in source destination; do
    printf 'kept source\n' > "$source"
    printf 'kept destination\n' > "$dest/alpha.txt"
    touch -t 202601010000 "$source" "$dest/alpha.txt"
    if [[ $newer == source ]]; then touch -t 202601020000 "$source"
    else touch -t 202601020000 "$dest/alpha.txt"; fi
    run --keep
    grep -Fx 'kept source' "$source" >/dev/null
    grep -Fx 'kept destination' "$dest/alpha.txt" >/dev/null
done
printf 'new with keep\n' > "$project/config/missing.txt"
run --keep
same "$project/config/missing.txt" "$dest/missing.txt"
custom="$scratch/custom destination/magi"
run --dest="$custom"
same "$source" "$custom/alpha.txt"
printf '#!/bin/sh\nexit 37\n' > "$scratch/fail-bin/rsync"
chmod +x "$scratch/fail-bin/rsync"
for newer in source destination; do
    printf 'unchanged source\n' > "$source"
    printf 'unchanged destination\n' > "$dest/alpha.txt"
    touch -t 202601010000 "$source" "$dest/alpha.txt"
    if [[ $newer == source ]]; then touch -t 202601020000 "$source"
    else touch -t 202601020000 "$dest/alpha.txt"; fi
    if fixture_path="$scratch/fail-bin:$PATH" run; then
        echo 'failed configuration copy reported success' >&2; exit 1;
    fi
    grep -Fx 'unchanged source' "$source" >/dev/null
    grep -Fx 'unchanged destination' "$dest/alpha.txt" >/dev/null
done
echo 'config install: actual sync, newer-side import, ties, keep, custom path and copy failures PASS'
