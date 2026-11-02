#!/bin/sh
# Executed only in UUID-named, bounded local fixture containers.
set -eu
export DEBIAN_FRONTEND=noninteractive
mkdir -p /evidence/os
cp /usr/share/keyrings/debian-archive-keyring.pgp /evidence/os/archive-keyring.pgp
# Bootstrap anchor from both reviewed immutable Debian-based OCI manifests.
echo '506b815cbb32d9b6066b4a2aa524071e071761e7e7f68c3ac74f3061ba852017  /evidence/os/archive-keyring.pgp' | sha256sum -c -
cp /etc/apt/sources.list.d/debian.sources /evidence/os/sources.deb822
apt-get -o APT::Update::Error-Mode=any -o Debug::Acquire::gpgv=true update > /evidence/os/apt-update.log 2>&1
# Replace obsolete inherited packages using the same authenticated repository.
# Every resulting exact version and .deb is locked in os-packages.json.
apt-get dist-upgrade -y --no-install-recommends > /evidence/os/apt-upgrade.log 2>&1
case "$1" in
    go|plugin) packages='ca-certificates gpgv python3' ;;
    python) packages='ca-certificates gpgv python3-venv python3-dev libpq-dev build-essential python3-wheel python3-setuptools' ;;
    sidecar) packages='ca-certificates gpgv python3-venv libpq5 liblz4-1 libsnappy1v5 tar' ;;
    *) exit 2 ;;
esac
# APT verifies signed Release -> Packages -> package content before installation.
apt-get install -y --no-install-recommends $packages > /evidence/os/apt-install.log 2>&1
python3 /recipe/os_evidence.py collect /evidence
