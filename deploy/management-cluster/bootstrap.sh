#!/usr/bin/env bash
# Only the two explicitly owned fresh infrastructure hosts; no RUN/BUILD mutation.
set -euo pipefail
umask 077
cd -- "$(dirname -- "$0")"
[[ ${EUID} == 0 && $(uname -m) == x86_64 && ${1:-} == --fresh-management-host ]] || { echo 'Explicit root fresh-host invocation required'; exit 1; }
case ${2:-} in
 core) expected=webdock-core-eu-01; address=10.0.4.1;;
 obs) expected=webdock-obs-eu-01; address=10.0.4.2;;
 *) echo 'Unsupported role'; exit 1;;
esac
[[ $(hostname) == "$expected" ]] || { echo 'Wrong hostname'; exit 1; }
[[ $(cat profile/identity) == "$expected $address" ]] || { echo 'Profile identity differs'; exit 1; }
ip -4 -o addr show | awk '{print $4}' | cut -d/ -f1 | grep -Fx "$address" >/dev/null || { echo 'Verified provider private address must exist before install'; exit 1; }
readonly version='v1.35.9+k3s1'
readonly sha256='897acb21cca57d477a211102251a6591450ce767344a9faaa121a1007ca9fa9a'
readonly marker=/etc/webdock-management-cluster
if [[ ! -f "$marker" ]] && { [[ -e /etc/rancher/k3s || -e /var/lib/rancher/k3s ]] || command -v k3s >/dev/null; }; then
 echo 'Refusing existing unowned K3s'; exit 1
fi
if [[ -f "$marker" ]]; then
 [[ $(cat "$marker") == "$expected $version $sha256" ]] || { echo 'Refusing identity/version drift'; exit 1; }
 cmp profile/config.yaml /etc/rancher/k3s/config.yaml || { echo 'Refusing configuration drift'; exit 1; }
fi
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y --no-install-recommends ca-certificates curl nftables conntrack iptables
if [[ ! -f "$marker" ]]; then
 download=$(mktemp -d)
 trap 'rm -rf -- "$download"' EXIT
 curl --fail --location --proto '=https' --tlsv1.2 --retry 3 "https://github.com/k3s-io/k3s/releases/download/${version/+/%2B}/k3s" -o "$download/k3s"
 printf '%s  %s\n' "$sha256" "$download/k3s" | sha256sum -c -
 install -m 0755 "$download/k3s" /usr/local/bin/k3s
 install -d -m 0700 /etc/rancher/k3s
 install -m 0600 profile/config.yaml /etc/rancher/k3s/config.yaml
 printf '%s %s %s\n' "$expected" "$version" "$sha256" > "$marker"
fi
printf '%s  %s\n' "$sha256" /usr/local/bin/k3s | sha256sum -c -
install -m 0600 profile/host.nft /etc/rancher/k3s/management-host.nft
cat > /etc/systemd/system/webdock-management-firewall.service <<'UNIT'
[Unit]
Description=Owned management cluster host firewall
After=network-online.target
Wants=network-online.target
Before=k3s.service
[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/bin/sh -ec 'nft list table inet webdock_management >/dev/null 2>&1 || nft add table inet webdock_management; nft --check -f /etc/rancher/k3s/management-host.nft; nft -f /etc/rancher/k3s/management-host.nft'
[Install]
WantedBy=multi-user.target
UNIT
cat > /etc/systemd/system/k3s.service <<'UNIT'
[Unit]
Description=Standalone Webdock management K3s
After=network-online.target webdock-management-firewall.service
Requires=webdock-management-firewall.service
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
systemctl daemon-reload
systemctl enable --now webdock-management-firewall.service
systemctl enable --now k3s.service
