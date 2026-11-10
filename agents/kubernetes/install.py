#!/usr/bin/env python3
"""Install a customer-owned cluster agent. Run as root on the customer's host."""
import argparse
import getpass
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
from urllib.parse import urlparse
from connect import agent_request

def main():
 parser=argparse.ArgumentParser(description=__doc__)
 parser.add_argument('--endpoint',required=True)
 parser.add_argument('--cluster',required=True)
 parser.add_argument('--kubeconfig',required=True)
 parser.add_argument('--kubectl',default='kubectl')
 args=parser.parse_args()
 if os.geteuid()!=0:parser.error('Run with sudo to install a persistent system service.')
 url=urlparse(args.endpoint)
 if url.scheme!='https' or not url.hostname or url.path not in ('','/') or url.query or url.fragment or url.username or url.password:parser.error('Use the HTTPS Webdock Auth origin.')
 if not re.fullmatch(r'[1-9][0-9]{0,18}',args.cluster):parser.error('Invalid cluster ID.')
 kubeconfig=Path(args.kubeconfig).resolve()
 executable=shutil.which(args.kubectl)
 if kubeconfig.is_relative_to('/tmp') or kubeconfig.is_relative_to('/var/tmp'):parser.error('Keep kubeconfig in a permanent private directory, not temporary storage.')
 if not kubeconfig.is_file() or not executable:parser.error('Kubeconfig and kubectl must exist locally.')
 if any(re.search(r'[\s%"\\]',str(v)) for v in [kubeconfig,executable]):parser.error('Use paths without spaces, percent signs, quotes or backslashes.')
 target=Path('/opt/webdock-byok')/args.cluster
 state=Path('/var/lib/webdock-byok')/args.cluster
 if (state/'connection.json').exists():parser.error('An agent already exists. Stop its service and explicitly archive the private connection file before re-enrollment.')
 token=getpass.getpass('One-time Webdock enrollment token: ')
 if not re.fullmatch(r'[A-Za-z0-9_-]{43}',token):parser.error('Invalid enrollment token.')
 enrolled=agent_request(args.endpoint,{},'enroll',{'clusterID':args.cluster,'token':token})
 target.mkdir(parents=True,exist_ok=True);state.mkdir(parents=True,exist_ok=True,mode=0o700);os.chmod(state,0o700)
 for name in ['daemon.py','byok.py','observer.py','connect.py','executor.py','storage_client.py','registry.py','service_bindings.py']:
  shutil.copyfile(Path(__file__).with_name(name),target/name);os.chmod(target/name,0o644)
 connection=state/'connection.json'
 fd=os.open(connection,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
 with os.fdopen(fd,'w') as stream:json.dump({'endpoint':args.endpoint,**enrolled,'sequence':0},stream)
 unit=f'webdock-byok-{args.cluster}.service'
 Path('/etc/systemd/system',unit).write_text(f'''[Unit]
Description=Webdock customer Kubernetes agent
After=network-online.target
Wants=network-online.target
[Service]
ExecStart=/usr/bin/python3 {target}/daemon.py --credentials {connection} --byok
Environment=SPITZLI_KUBECTL={executable}
Environment=SPITZLI_KUBECONFIG={kubeconfig}
Environment=SPITZLI_KUBECTL_CACHE_DIR={state}/cache
Restart=on-failure
RestartSec=15
RestartPreventExitStatus=78
UMask=0077
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ReadWritePaths={state}
[Install]
WantedBy=multi-user.target
''')
 subprocess.run(['systemctl','daemon-reload'],check=True)
 subprocess.run(['systemctl','enable','--now',unit],check=True)
 print('Agent installed. Check its connection and inventory in Webdock.')
if __name__=='__main__':main()
