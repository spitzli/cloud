#!/bin/sh
# Operator-only bounded sandbox fixture; does not modify the production profile.
set -eu
umask 077
if [ "$(id -u)" != 1000 ] || [ ! -f /run/.containerenv ]; then
    echo 'qualified_nonroot_container_required' >&2
    exit 1
fi
printf '%s\n' \
 'ca3c5d4fe1defc5df56631e179824f17ddd555f1fcf5057d14dfb87496baa8c3  /usr/bin/buildkitd' \
 '11d94c314dca0e882879790329ff5a9c6c0d046fbbad927daf050897c3a21e71  /usr/bin/rootlesskit' \
 '8e64362dceba989827b23af567915feda10426ebd40611772318662e33bf2bfd  /usr/bin/buildctl' \
 | sha256sum -c - >/dev/null
mkdir /cache/overlayfs-buildkit
cd /cache/overlayfs-buildkit
mkdir tmp runtime context
# Use only binaries from the approved immutable image; no pull or package egress.
cp /bin/busybox context/busybox
cp -L /lib/ld-musl-x86_64.so.1 context/ld-musl-x86_64.so.1
printf seed >context/remove
cat >context/Dockerfile <<'DOCKERFILE'
FROM scratch
COPY busybox /bin/busybox
COPY ld-musl-x86_64.so.1 /lib/ld-musl-x86_64.so.1
COPY remove /remove
RUN ["/bin/busybox", "sh", "-eu", "-c", "test \"$(/bin/busybox cat /remove)\" = seed; /bin/busybox rm /remove; printf webdock-overlayfs-buildkit-proof >/proof"]
DOCKERFILE
exec env -i PATH=/usr/local/bin:/usr/bin:/bin HOME=/home/user USER=user LOGNAME=user \
 XDG_RUNTIME_DIR=/cache/overlayfs-buildkit/runtime TMPDIR=/cache/overlayfs-buildkit/tmp \
 timeout -k 5 105 /usr/bin/rootlesskit --state-dir=/cache/overlayfs-buildkit/rootlesskit \
 /bin/sh -eu -c '
    cd /cache/overlayfs-buildkit
    /usr/bin/buildkitd --addr=unix:///cache/overlayfs-buildkit/runtime/buildkitd.sock \
      --root=/cache/overlayfs-buildkit/state --oci-worker-snapshotter=overlayfs \
      --containerd-worker=false >daemon.log 2>&1 &
    daemon=$!
    trap '\''kill -TERM "$daemon" 2>/dev/null || :; wait "$daemon" 2>/dev/null || :'\'' EXIT
    ready=false
    for n in $(seq 1 80); do
      if ! kill -0 "$daemon" 2>/dev/null; then break; fi
      if /usr/bin/buildctl --addr unix:///cache/overlayfs-buildkit/runtime/buildkitd.sock debug workers -v >workers.txt 2>/dev/null; then
        ready=true
        break
      fi
      sleep 0.25
    done
    if [ "$ready" != true ]; then cat daemon.log; echo overlayfs_worker_start_failed; exit 1; fi
    cat workers.txt
    grep "snapshotter:.*overlayfs" workers.txt >/dev/null
    timeout -k 3 60 /usr/bin/buildctl --addr unix:///cache/overlayfs-buildkit/runtime/buildkitd.sock \
      build --progress plain --frontend dockerfile.v0 \
      --local context=/cache/overlayfs-buildkit/context \
      --local dockerfile=/cache/overlayfs-buildkit/context \
      --output type=oci,dest=/cache/overlayfs-buildkit/image.tar \
      --output type=local,dest=/cache/overlayfs-buildkit/rootfs
    test "$(cat rootfs/proof)" = webdock-overlayfs-buildkit-proof
    test ! -e rootfs/remove
    test -s image.tar
    sha256sum image.tar
    echo overlayfs_buildkit_run_export_verified_ready
    # Give the operator a bounded window to fetch the OCI for independent validation.
    sleep 25
 '
