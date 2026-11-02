#!/usr/bin/env bash
# Explicit build-host-only optional runtime. This never joins a runtime cluster.
set -euo pipefail
umask 077
[[ ${EUID} == 0 && $(uname -m) == x86_64 && ${1:-} == --qualified-build-host ]] || { echo 'Requires explicit root build-host invocation'; exit 1; }
[[ -f /etc/webdock-build-host && $(hostname) == webdock-build-eu-01 ]] || { echo 'Refusing an unowned build host'; exit 1; }
readonly version=20261026.0
readonly hash=26e4a327565f5da0f73261a6387e2c20f4695a232e330095a3111b70efd604fdf09d734e8cc8f714dbb5c5607ce8c87ea80b26df549d83a146f2efd5a1af448a
# Never restart a cluster while a customer Job may be executing.
active=$(/usr/local/bin/k3s kubectl get pods -A -o json | python3 -c 'import json,sys;print(sum(p["metadata"]["namespace"] in ("webdock-builds","webdock-build-qualification") and p.get("status",{}).get("phase") not in ("Succeeded","Failed") for p in json.load(sys.stdin)["items"]))')
[[ "$active" == 0 ]] || { echo 'Drain and reconcile active builds before changing the runtime'; exit 1; }
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y --no-install-recommends bzip2 ca-certificates curl >/dev/null
work=$(mktemp -d)
trap 'rm -rf -- "$work"' EXIT
curl --fail --location --proto '=https' --tlsv1.2 --retry 3 "https://storage.googleapis.com/gvisor/releases/release/${version}/x86_64/gvisor.tar.bz2" -o "$work/gvisor.tar.bz2"
printf '%s  %s\n' "$hash" "$work/gvisor.tar.bz2" | sha512sum -c -
# Only release-owned executables/sidecars may be installed. Never extract arbitrary paths or links.
python3 - "$work/gvisor.tar.bz2" <<'PY'
import sys,tarfile,pathlib
with tarfile.open(sys.argv[1],'r:bz2') as archive:
    for item in archive.getmembers():
        p=pathlib.PurePosixPath(item.name)
        if p.is_absolute() or '..' in p.parts or not(item.isfile() or item.isdir()):raise SystemExit('Unsafe sandbox release archive')
        if str(p) not in ('runsc','containerd-shim-runsc-v1','gvisor-bin') and (not p.parts or p.parts[0]!='gvisor-bin'):raise SystemExit('Unexpected sandbox release file')
PY
tar -xjf "$work/gvisor.tar.bz2" -C /usr/local/bin
/usr/local/bin/runsc --version | head -2
install -d -m 0755 /var/lib/rancher/k3s/agent/etc/containerd/config-v3.toml.d
cat > /etc/rancher/k3s/runsc.toml <<'CONFIG'
[runsc_config]
  platform = "systrap"
  network = "sandbox"
  allow-suid = "true"
CONFIG
chmod 0600 /etc/rancher/k3s/runsc.toml
cat > /var/lib/rancher/k3s/agent/etc/containerd/config-v3.toml.d/99-webdock-build-sandbox.toml <<'CONFIG'
[plugins.'io.containerd.cri.v1.runtime'.containerd.runtimes.webdock-build-sandbox]
  runtime_type = "io.containerd.runsc.v1"
[plugins.'io.containerd.cri.v1.runtime'.containerd.runtimes.webdock-build-sandbox.options]
  TypeUrl = "io.containerd.runsc.v1.options"
  ConfigPath = "/etc/rancher/k3s/runsc.toml"
CONFIG
# This is a gVisor guest-only ID-map helper allowance, never a host sysctl relaxation.
systemctl restart k3s
/usr/local/bin/k3s kubectl apply -f "$(dirname "$0")/sandbox-runtime.yaml"
printf 'Sandbox installed. Qualify the pinned runner and admission profile before enabling any project.\n'
