#!/usr/bin/env python3
"""Render fixed owned Core/OBS cluster profiles; never contacts a host or API."""
import argparse
from pathlib import Path
PROFILES={'core':('webdock-core-eu-01','10.0.4.1','10.89.0.0/16','10.90.0.0/16','10.90.0.10'),'obs':('webdock-obs-eu-01','10.0.4.2','10.92.0.0/16','10.93.0.0/16','10.93.0.10')}
def render(role,public_https=False):
 if role not in PROFILES:raise ValueError('Only owned Core/OBS roles supported')
 host,private,pods,services,dns=PROFILES[role]
 config=f'''node-name: {host}
node-ip: {private}
bind-address: {private}
advertise-address: {private}
tls-san:
  - 127.0.0.1
cluster-cidr: {pods}
service-cidr: {services}
cluster-dns: {dns}
secrets-encryption: true
write-kubeconfig-mode: '0600'
disable:
  - traefik
  - servicelb
  - local-storage
kubelet-arg:
  - 'system-reserved=cpu=500m,memory=1Gi,ephemeral-storage=8Gi,pid=1000'
  - 'kube-reserved=cpu=500m,memory=1Gi,ephemeral-storage=8Gi,pid=1000'
  - 'pod-max-pids=512'
  - 'eviction-hard=memory.available<768Mi,nodefs.available<15%,imagefs.available<15%'
'''
 rules=f'''flush table inet webdock_management
table inet webdock_management {{
 chain input {{
  type filter hook input priority -10; policy drop;
  iifname "lo" accept
  iifname "eth0" ip saddr {pods} drop
  ct state established,related accept
  ct state invalid drop
  meta l4proto {{ icmp, ipv6-icmp }} accept
  tcp dport 22 accept
  iifname "cni0" ip saddr {pods} ip daddr {private} tcp dport {{ 6443, 10250 }} accept
 }}
 chain forward {{
  type filter hook forward priority -10; policy accept;
  iifname "eth*" oifname "eth*" drop
  ct state established,related accept
  iifname "eth0" oifname {{ "cni0", "flannel.1" }} drop
 }}
}}
'''
 if role=='obs' or public_https:rules=rules.replace('  tcp dport 22 accept\n','  tcp dport 22 accept\n  iifname "eth0" tcp dport { 80, 443 } accept\n')
 return {'config.yaml':config,'host.nft':rules,'identity':host+' '+private+'\n'}
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('role',choices=PROFILES);p.add_argument('--output',required=True,type=Path);a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True)
 for name,text in render(a.role).items():(a.output/name).write_text(text)
if __name__=='__main__':main()
