#!/bin/sh
# Inside the disposable VM only. No host mounts, credentials or network devices exist.
mount -o remount,rw /
mount -t proc proc /proc
mount -t sysfs sysfs /sys
mount -t devtmpfs devtmpfs /dev
mkdir -p /dev/pts /dev/shm /run /sys/fs/cgroup
mount -t devpts devpts /dev/pts
mount -t tmpfs tmpfs /run
mount -t tmpfs tmpfs /dev/shm
mount -t cgroup2 none /sys/fs/cgroup
modprobe virtio_console
modprobe overlay
mdev -s
/usr/bin/buildkitd --oci-worker=true --containerd-worker=false --oci-worker-net=host >/tmp/buildkit.log 2>&1 &
build_guest_attempt=0
while [ ! -S /run/buildkit/buildkitd.sock ] && [ "$build_guest_attempt" -lt 100 ]; do
  sleep .1
  build_guest_attempt=$((build_guest_attempt+1))
done
/usr/bin/webdock-build-guest
poweroff -f
