#!/usr/bin/env bash
# Run on the explicitly selected fresh build host, never a runtime node.
set -euo pipefail
umask 077
cd -- "$(dirname -- "$0")"
readonly version='v1.35.9+k3s1'
readonly sha256='897acb21cca57d477a211102251a6591450ce767344a9faaa121a1007ca9fa9a'
readonly marker=/etc/webdock-build-host
[[ ${EUID} == 0 && $(uname -m) == x86_64 ]] || { echo 'Requires root on x86_64'; exit 1; }
[[ ${1:-} == --standalone-build-server ]] || { echo 'Use --standalone-build-server on the authorized build host'; exit 1; }
if [[ ! -f "$marker" ]] && { [[ -e /etc/rancher/k3s || -e /var/lib/rancher/k3s ]] || command -v k3s >/dev/null; }; then
  echo 'Existing K3s installation is not managed by this bootstrap'; exit 1
fi
if [[ -f "$marker" ]]; then
  [[ $(cat "$marker") == "$version $sha256" ]] || { echo 'Explicit upgrade procedure required'; exit 1; }
  cmp config.yaml /etc/rancher/k3s/config.yaml || { echo 'Refusing configuration drift'; exit 1; }
fi
# Only our host profile is managed; no account/password rotation or disk formatting.
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y --no-install-recommends ca-certificates curl nftables conntrack iptables
if [[ ! -f "$marker" ]]; then
  download=$(mktemp -d)
  trap 'rm -rf -- "$download"' EXIT
  curl --fail --location --proto '=https' --tlsv1.2 --retry 3 \
    "https://github.com/k3s-io/k3s/releases/download/${version/+/%2B}/k3s" -o "$download/k3s"
  printf '%s  %s\n' "$sha256" "$download/k3s" | sha256sum -c -
  install -m 0755 "$download/k3s" /usr/local/bin/k3s
  install -d -m 0700 /etc/rancher/k3s
  install -m 0600 config.yaml /etc/rancher/k3s/config.yaml
  printf '%s %s\n' "$version" "$sha256" > "$marker"
fi
printf '%s  %s\n' "$sha256" /usr/local/bin/k3s | sha256sum -c -
hostnamectl set-hostname webdock-build-eu-01
install -m 0600 host.nft /etc/rancher/k3s/host.nft
cat > /etc/systemd/system/webdock-build-network.service <<'UNIT'
[Unit]
Description=Private build API address and host firewall
After=network-online.target
Wants=network-online.target
Before=k3s.service
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/sh -ec 'ip link show webdock-build0 >/dev/null 2>&1 || ip link add webdock-build0 type dummy; ip address replace 10.83.0.1/32 dev webdock-build0; ip link set webdock-build0 up; nft list table inet webdock_build_host >/dev/null 2>&1 || nft add table inet webdock_build_host; nft -f /etc/rancher/k3s/host.nft'
[Install]
WantedBy=multi-user.target
UNIT
cat > /etc/systemd/system/k3s.service <<'UNIT'
[Unit]
Description=Webdock standalone BUILD K3s
After=network-online.target webdock-build-network.service
Requires=webdock-build-network.service
[Service]
Type=notify
Delegate=yes
KillMode=process
LimitNOFILE=1048576
TasksMax=infinity
TimeoutStartSec=300
Restart=always
RestartSec=5
ExecStartPre=-/sbin/modprobe br_netfilter
ExecStartPre=-/sbin/modprobe overlay
ExecStart=/usr/local/bin/k3s server --config /etc/rancher/k3s/config.yaml
[Install]
WantedBy=multi-user.target
UNIT
install -d -m 0755 /usr/local/libexec
install -m 0700 sync-system-pods.py /usr/local/libexec/webdock-build-system-pods
cat > /etc/systemd/system/webdock-build-system-pods.service <<'UNIT'
[Unit]
Description=Refresh trusted system pod IPs for private build API
After=k3s.service
[Service]
Type=oneshot
ExecStart=/usr/bin/python3 /usr/local/libexec/webdock-build-system-pods
UNIT
cat > /etc/systemd/system/webdock-build-system-pods.timer <<'UNIT'
[Unit]
Description=Refresh expiring system pod firewall admission
[Timer]
OnBootSec=5s
OnUnitActiveSec=5s
AccuracySec=1s
[Install]
WantedBy=timers.target
UNIT
systemctl daemon-reload
systemctl enable --now webdock-build-network.service
# Apply changed host rules on rerun without restarting a healthy cluster.
nft -f /etc/rancher/k3s/host.nft
systemctl enable --now k3s.service
systemctl enable --now webdock-build-system-pods.timer
systemctl start webdock-build-system-pods.service
printf 'Standalone build K3s installed; verify readiness before applying foundation.yaml.\n'
