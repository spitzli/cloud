#!/bin/sh
# Fixed operator startup only. Source-selected commands run through the sandboxed BuildKit daemon.
set -eu
umask 077
if [ "$(id -u)" != 1000 ] || { [ ! -f /run/.containerenv ] && [ ! -f /.dockerenv ]; }; then
    echo 'nonroot_build_container_required' >&2
    exit 1
fi
printf '%s\n' \
 'ca3c5d4fe1defc5df56631e179824f17ddd555f1fcf5057d14dfb87496baa8c3  /usr/bin/buildkitd' \
 '11d94c314dca0e882879790329ff5a9c6c0d046fbbad927daf050897c3a21e71  /usr/bin/rootlesskit' \
 'b46c551f5e795a81144a3dbe74227c40b7cbe231024d8e9167e691208031ec30  /usr/local/libexec/mise-2026.9.15' \
 | sha256sum -c - >/dev/null
# Fresh bounded mounts per execution; do not adopt state from another attempt.
mkdir /workspace/runtime /workspace/tmp /cache/buildkit /cache/railpack /cache/railpack/mise
proxy_url=$(/usr/local/bin/spitzli-build-runner proxy-url --tools /etc/webdock-build/tools.json)
printf '%s\n' "$proxy_url" >/workspace/runtime/package-proxy-url
cp /usr/local/bin/webdock-mise-wrapper /cache/railpack/mise/mise-2026.9.15
chmod 0700 /cache/railpack/mise/mise-2026.9.15
ln -s /cache/railpack /tmp/railpack
mkfifo /workspace/runtime/daemon.pipe
# Daemon diagnostics are private, capped and never forwarded as unredacted build logs.
/usr/local/bin/webdock-build-daemon-logger </workspace/runtime/daemon.pipe >/workspace/runtime/daemon.log &
logger_pid=$!
daemon_pid=
runner_pid=
cleanup() {
    trap - EXIT INT TERM
    [ -z "$runner_pid" ] || kill -TERM "$runner_pid" 2>/dev/null || :
    [ -z "$daemon_pid" ] || kill -TERM "$daemon_pid" 2>/dev/null || :
    kill -TERM "$logger_pid" 2>/dev/null || :
    sleep 0.2
    [ -z "$runner_pid" ] || kill -KILL "$runner_pid" 2>/dev/null || :
    [ -z "$daemon_pid" ] || kill -KILL "$daemon_pid" 2>/dev/null || :
    kill -KILL "$logger_pid" 2>/dev/null || :
    # Reap only our known children; an adopted grandchild must not keep cleanup waiting forever.
    [ -z "$runner_pid" ] || wait "$runner_pid" 2>/dev/null || :
    [ -z "$daemon_pid" ] || wait "$daemon_pid" 2>/dev/null || :
    wait "$logger_pid" 2>/dev/null || :
}
trap cleanup EXIT
trap 'exit 143' TERM
trap 'exit 130' INT
# No ambient daemon flags, host sockets or disabled process sandbox.
env -i PATH=/usr/local/bin:/usr/bin:/bin HOME=/home/user USER=user LOGNAME=user \
 XDG_RUNTIME_DIR=/workspace/runtime TMPDIR=/workspace/tmp \
 HTTP_PROXY="$proxy_url" HTTPS_PROXY="$proxy_url" ALL_PROXY="$proxy_url" \
 http_proxy="$proxy_url" https_proxy="$proxy_url" all_proxy="$proxy_url" NO_PROXY= no_proxy= \
 /usr/bin/rootlesskit --state-dir=/workspace/runtime/rootlesskit \
 /usr/bin/buildkitd --addr=unix:///workspace/runtime/buildkitd.sock \
 --root=/cache/buildkit --oci-worker-snapshotter=overlayfs --containerd-worker=false \
 >/workspace/runtime/daemon.pipe 2>&1 &
daemon_pid=$!
ready=false
for attempt in $(seq 1 20); do
    if ! kill -0 "$daemon_pid" 2>/dev/null; then break; fi
    if timeout 1 /usr/bin/buildctl --addr unix:///workspace/runtime/buildkitd.sock debug workers >/dev/null 2>&1; then
        ready=true
        break
    fi
    sleep 0.1
done
if [ "$ready" != true ]; then
    echo 'buildkit_startup_failed' >&2
    exit 1
fi
/usr/local/bin/spitzli-build-runner "$@" &
runner_pid=$!
set +e
wait "$runner_pid"
status=$?
set -e
runner_pid=
# The handoff is an immutable exact-file mount, scoped only to this attempt.
if [ -f /handoff/attempt.json ]; then
    set +e
    /usr/local/bin/spitzli-build-runner upload --config /handoff/attempt.json \
        --policy /etc/webdock-build/transfer-policy.json --output-dir /workspace/output
    upload_status=$?
    set -e
    if [ "$upload_status" != 0 ]; then exit "$upload_status"; fi
fi
exit "$status"
