#!/usr/bin/env bash
# Owned local fixture only. Mounts are private to a fresh unprivileged user/mount namespace.
set -euo pipefail
script_path=$(realpath -- "$0")
repo_root=$(realpath -- "$(dirname -- "$script_path")/../..")
if [[ ${1:-} == --inside-owned-namespace ]]; then
  work=$2
  test_binary=$3
  evidence=$4
  root="$work/upload-tmpfs"
  mkdir -m 700 -- "$root"
  cleanup() {
    # The test may have stacked a replacement mount when it failed; peel only this exact path.
    while mountpoint -q -- "$root"; do umount -- "$root" || return 1; done
    rmdir -- "$root"
  }
  trap cleanup EXIT
  mount --make-rprivate /
  mount -t tmpfs -o size=2097152,mode=0700,nosuid,nodev,noexec webdock-upload-owned-test "$root"
  export WEBDOCK_UPLOAD_LIVE_ROOT="$root"
  export WEBDOCK_UPLOAD_LIVE_EVIDENCE="$evidence"
  "$test_binary" --exact real_tmpfs_and_native_http_upload_gate --ignored --nocapture --test-threads=1
  exit
fi
cd -- "$repo_root"
evidence=${1:-"$repo_root/docs/verification/2026-10-30-build-upload-gateway-local.json"}
evidence=$(realpath -m -- "$evidence")
mkdir -p -- "$(dirname -- "$evidence")"
work=$(mktemp -d "${TMPDIR:-/tmp}/webdock-upload-gate.XXXXXXXX")
cleanup_outer() {
  rm -f -- "$work/build.json" "$work/evidence.json"
  rmdir -- "$work"
}
trap cleanup_outer EXIT
cargo test -p webdock-build --features worker,kubernetes-job --test upload_gateway_live --no-run --message-format=json > "$work/build.json"
test_binary=$(python3 - "$work/build.json" <<'PY'
import json, sys
executables=[]
with open(sys.argv[1]) as source:
    for line in source:
        row=json.loads(line)
        if row.get('reason')=='compiler-artifact' and row.get('target',{}).get('name')=='upload_gateway_live' and row.get('executable'):
            executables.append(row['executable'])
assert len(executables)==1, 'expected exactly one native upload-gateway integration executable'
print(executables[0])
PY
)
rm -- "$work/build.json"
unshare --user --map-root-user --map-auto --mount --propagation private -- "$script_path" --inside-owned-namespace "$work" "$test_binary" "$work/evidence.json"
mv -- "$work/evidence.json" "$evidence"
printf 'Verified native upload gateway; evidence: %s\n' "$evidence"
