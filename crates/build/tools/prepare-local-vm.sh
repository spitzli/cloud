#!/usr/bin/env bash
# Explicit disposable TEST image preparation; never reads an existing host/production VM.
set -euo pipefail
[[ "${1:-}" == --allow-disposable-local-vm ]] || { echo 'Explicit disposable local VM opt-in required.' >&2; exit 2; }
build_fixture_root="$(cd "$(dirname "$0")/../../.." && pwd)"
build_fixture_output="${2:-$build_fixture_root/.runtime-test/build-vm}"
[[ ! -e "$build_fixture_output" ]] || { echo 'Use a fresh destination; existing fixtures are never overwritten.' >&2; exit 2; }
mkdir -m 700 -p "$build_fixture_output"
build_fixture_output="$(cd "$build_fixture_output" && pwd)"
build_fixture_tag="webdock-build-fixture:prepare-$$"
build_fixture_container="webdock-build-fixture-prepare-$$"
cleanup() { docker rm -f "$build_fixture_container" >/dev/null 2>&1 || true; docker image rm "$build_fixture_tag" >/dev/null 2>&1 || true; }
trap cleanup EXIT
curl --fail --location --proto '=https' --tlsv1.2 \
  'https://dl-cdn.alpinelinux.org/alpine/v3.22/releases/x86_64/alpine-minirootfs-3.22.6-x86_64.tar.gz' \
  --output "$build_fixture_output/alpine.tar.gz"
printf '%s  %s\n' '27694aaa55fd7a9e3ef596e0ad4eb66802308bb20172b17030cd5f4d8ae9bac2' "$build_fixture_output/alpine.tar.gz" | sha256sum --check -
docker import "$build_fixture_output/alpine.tar.gz" "$build_fixture_tag" >/dev/null
# apk verifies signatures using the official keys in the checksummed root filesystem.
# Networking exists only during trusted package preparation, never during customer builds.
docker run --name "$build_fixture_container" --network bridge "$build_fixture_tag" /bin/sh -ec \
  'apk add --no-cache linux-virt=6.12.114-r0 buildkit=0.21.1-r5 buildctl=0.21.1-r5 runc=1.3.4-r1; apk info -v; buildctl --version' \
  > "$build_fixture_output/packages.txt"
docker export "$build_fixture_container" --output "$build_fixture_output/rootfs.tar"
mkdir "$build_fixture_output/rootfs"
tar --extract --file "$build_fixture_output/rootfs.tar" --directory "$build_fixture_output/rootfs" --no-same-owner
cd "$build_fixture_root"
rustup target add x86_64-unknown-linux-musl
cargo build --release -p webdock-build --no-default-features --bin webdock-build-guest --target x86_64-unknown-linux-musl
install -m 755 "${CARGO_TARGET_DIR:-target}/x86_64-unknown-linux-musl/release/webdock-build-guest" "$build_fixture_output/rootfs/usr/bin/webdock-build-guest"
install -m 755 crates/build/tools/vm-init.sh "$build_fixture_output/rootfs/init"
truncate --size 2G "$build_fixture_output/base.raw"
mkfs.ext4 -q -F -d "$build_fixture_output/rootfs" "$build_fixture_output/base.raw"
cp "$build_fixture_output/rootfs/boot/vmlinuz-virt" "$build_fixture_output/kernel"
cp "$build_fixture_output/rootfs/boot/initramfs-virt" "$build_fixture_output/initrd"
chmod 600 "$build_fixture_output/base.raw" "$build_fixture_output/kernel" "$build_fixture_output/initrd"
sha256sum "$build_fixture_output/base.raw" "$build_fixture_output/kernel" "$build_fixture_output/initrd" > "$build_fixture_output/SHA256SUMS"
printf 'Prepared isolated test image in %s\n' "$build_fixture_output"
