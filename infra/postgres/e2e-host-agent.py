"""Real daemon with disposable-node DNS and Unix-socket transports for local E2E."""
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import time
from urllib.error import HTTPError
ROOT=Path(__file__).resolve().parents[2]
BASE=ROOT/'.superpowers/sdd/2026-10-12-managed-postgresql'
if os.environ.get('SPITZLI_DISPOSABLE_POSTGRES_TEST')!='yes':raise SystemExit('Explicit disposable E2E environment required')
sys.path.insert(0,str(ROOT/'agents/kubernetes'))
import daemon
import databases
import storage_client
container='webdock-managed-pg-pilot-c2d4df72'
node=json.loads(subprocess.check_output(['docker','inspect',container]))[0]
if node['Config']['Hostname']!='postgres-pilot' or node['Config']['Labels'].get('webdock.test')!='managed-postgresql':raise SystemExit('Unexpected test container')
address=node['NetworkSettings']['Networks']['bridge']['IPAddress']
resolve=socket.getaddrinfo
def fixture_dns(host,*args,**kwargs):
    if isinstance(host,str) and re.fullmatch(r'db-[1-9][0-9]{0,18}\.example\.invalid',host):host=address
    return resolve(host,*args,**kwargs)
socket.getaddrinfo=fixture_dns
RPC="""import json,sys
sys.path.insert(0,'/tmp/webdock-e2e-agent')
from storage_client import request
p=json.load(sys.stdin)
print(json.dumps(request(p['action'],p['value'],socket_path='/run/webdock-postgres-storage/control.sock')))
"""
def fixture_storage(action,value,socket_path=None):
    if socket_path is None:raise OSError('No application storage helper in this DB-only test node')
    if socket_path!='/run/webdock-postgres-storage/control.sock':raise ValueError('Unexpected test storage socket')
    response=subprocess.run(['docker','exec','-i',container,'python3','-c',RPC],input=json.dumps({'action':action,'value':value}),capture_output=True,text=True,timeout=110)
    if response.returncode:raise ValueError('Disposable database storage RPC failed')
    return json.loads(response.stdout)
storage_client.request=fixture_storage
while True:
    try:print(json.dumps(daemon.run_once(BASE/'host-agent.json')),flush=True)
    except HTTPError as error:
        if daemon.authentication_revoked(error):raise SystemExit('Disposable agent credential revoked')
        print(json.dumps({'event':'control-unavailable','status':error.code}),flush=True)
    except Exception as error:print(json.dumps({'event':'control-unavailable','type':type(error).__name__}),flush=True)
    time.sleep(15)
