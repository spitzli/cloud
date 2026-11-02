#!/usr/bin/env python3
"""Own-table-only host OUTPUT policy for the trusted persistent CI UID."""
from __future__ import annotations
import json
import os
from pathlib import Path
import subprocess
import sys

UID=1001
TABLE='spitzli_plugin_ci'
REGISTRY_IPV4='79.143.177.157'
PLATFORM_IPV4=['167.86.75.144','79.143.177.157']
PLATFORM_IPV6=['2a02:c207:3021:866::1']
PRIVATE_IPV4=['0.0.0.0/8','10.0.0.0/8','100.64.0.0/10','127.0.0.0/8','169.254.0.0/16','172.16.0.0/12','192.0.0.0/24','192.0.2.0/24','192.168.0.0/16','198.18.0.0/15','198.51.100.0/24','203.0.113.0/24','224.0.0.0/4','240.0.0.0/4','168.63.129.16/32']
PRIVATE_IPV6=['::/128','::1/128','fc00::/7','fe80::/10','fec0::/10','ff00::/8','64:ff9b::/96','64:ff9b:1::/48','2001:db8::/32','2001:2::/48']


def rules():
    ipv4=', '.join(PRIVATE_IPV4+PLATFORM_IPV4)
    ipv6=', '.join(PRIVATE_IPV6+PLATFORM_IPV6)
    return f'''table inet {TABLE} {{
  chain output {{
    type filter hook output priority 0; policy accept;
    meta skuid {UID} ip daddr 127.0.0.53 udp dport 53 counter accept comment "Exact host stub DNS"
    meta skuid {UID} ip daddr 127.0.0.53 tcp dport 53 counter accept comment "Exact host stub DNS TCP"
    meta skuid {UID} ip daddr {REGISTRY_IPV4} tcp dport 443 counter accept comment "Owner-approved shared RUN HTTPS ingress exception for release registry"
    meta skuid {UID} ip daddr {{ {ipv4} }} counter reject with icmpx type admin-prohibited comment "CI private metadata and platform IPv4 denied"
    meta skuid {UID} ip6 daddr {{ {ipv6} }} counter reject with icmpx type admin-prohibited comment "CI private metadata and platform IPv6 denied"
  }}
}}
'''


def apply():
    if os.geteuid()!=0:raise ValueError('Root-only host OUTPUT setup')
    existing=subprocess.run(['/usr/sbin/nft','list','table','inet',TABLE],capture_output=True).returncode==0
    batch=('delete table inet '+TABLE+'\n' if existing else '')+rules()
    subprocess.run(['/usr/sbin/nft','--check','--file','-'],input=batch,text=True,check=True)
    subprocess.run(['/usr/sbin/nft','--file','-'],input=batch,text=True,check=True)
    print(json.dumps({'table':'inet '+TABLE,'uid':UID,'registryHTTPSException':REGISTRY_IPV4,'otherTablesModified':False}))

if __name__=='__main__':
    if sys.argv[1:]==['--apply']:apply()
    elif not sys.argv[1:]:print(rules(),end='')
    else:raise SystemExit('Use --apply or no arguments to inspect source rules')
