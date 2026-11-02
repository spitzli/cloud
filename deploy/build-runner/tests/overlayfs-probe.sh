#!/bin/sh
# Qualification fixture only: run in an owned, bounded gVisor Job, never on the host.
# A container marker is an accidental-execution guard, not runtime attestation.
set -eu
umask 077
if [ "$(id -u)" != 1000 ] || [ ! -f /run/.containerenv ]; then
    echo 'qualified_nonroot_container_required' >&2
    exit 1
fi
# Exact startup binary used by the admitted runner; no host devices or inherited flags.
printf '%s\n' '11d94c314dca0e882879790329ff5a9c6c0d046fbbad927daf050897c3a21e71  /usr/bin/rootlesskit' | sha256sum -c - >/dev/null
# Fresh path, so a previous probe cannot be adopted or overwritten.
mkdir /cache/overlayfs-probe
mkdir /cache/overlayfs-probe/tmp /cache/overlayfs-probe/runtime
exec env -i PATH=/usr/local/bin:/usr/bin:/bin HOME=/home/user USER=user LOGNAME=user \
 XDG_RUNTIME_DIR=/cache/overlayfs-probe/runtime TMPDIR=/cache/overlayfs-probe/tmp \
 timeout -k 5 30 /usr/bin/rootlesskit --state-dir=/cache/overlayfs-probe/rootlesskit \
 /bin/sh -eu -c '
    cd /cache/overlayfs-probe
    mkdir lower1 lower2 upper work merged
    printf lower >lower1/copy-up
    printf remove >lower1/remove
    printf one >lower1/order
    printf two >lower2/order
    # Multiple lowerdirs and whiteouts are required, not merely mount success.
    mount -t overlay overlay -o "lowerdir=$PWD/lower2:$PWD/lower1,upperdir=$PWD/upper,workdir=$PWD/work,userxattr" "$PWD/merged"
    trap '\''umount /cache/overlayfs-probe/merged'\'' EXIT
    test "$(cat merged/order)" = two
    printf changed >merged/copy-up
    test "$(cat lower1/copy-up)" = lower
    test "$(cat upper/copy-up)" = changed
    rm merged/remove
    test -f lower1/remove
    test ! -e merged/remove
    # Remount: whiteouts must persist through snapshotter-style mount lifetimes.
    umount "$PWD/merged"
    trap - EXIT
    mount -t overlay overlay -o "lowerdir=$PWD/lower2:$PWD/lower1,upperdir=$PWD/upper,workdir=$PWD/work,userxattr" "$PWD/merged"
    trap '\''umount /cache/overlayfs-probe/merged'\'' EXIT
    test ! -e merged/remove
    test "$(cat merged/copy-up)" = changed
    test "$(cat lower1/copy-up)" = lower
    echo overlayfs_mount_copyup_whiteout_remount_passed
 '
