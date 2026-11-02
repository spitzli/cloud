#!/bin/sh
# Fixed maximum: Python's pre-dependency chain needs three configure passes.
set -eu
export DEBIAN_FRONTEND=noninteractive
ok=0
for attempt in 1 2 3; do
    if dpkg -i /evidence/os/debs/*.deb > "/evidence/offline-os-pass-$attempt.log" 2>&1; then
        ok=1
        break
    fi
done
test "$ok" = 1
dpkg --audit > /evidence/offline-os-audit.txt
test ! -s /evidence/offline-os-audit.txt
python3 /recipe/os_evidence.py verify-installed /evidence
