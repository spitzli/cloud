"""Run the real managed daemon against the isolated E2E control plane."""
import json
import os
from pathlib import Path
import re
import socket
import time
from urllib.error import HTTPError
import daemon
import databases
if os.environ.get('SPITZLI_DISPOSABLE_POSTGRES_TEST')!='yes' or socket.gethostname()!='postgres-pilot':raise SystemExit('Disposable test node required')
original=databases.execute_database
node=databases.get('node','postgres-pilot');address=next(a['address'] for a in node['status']['addresses'] if a['type']=='InternalIP')
def execute(packet,checkpoint):
    databases.validate_packet(packet)
    host=packet['backend']['host']
    if not re.fullmatch(r'db-[1-9][0-9]{0,18}\.example\.invalid',host):raise ValueError('Unexpected fixture domain')
    hosts=Path('/etc/hosts').read_text()
    if host not in hosts.split():
        with open('/etc/hosts','a') as stream:stream.write('\n'+address+' '+host+'\n')
    return original(packet,checkpoint)
# Only DNS for the reserved synthetic domain is supplied by this fixture.
databases.execute_database=execute
while True:
    try:print(json.dumps(daemon.run_once('/tmp/webdock-e2e-agent/connection.json')),flush=True)
    except HTTPError as error:
        if daemon.authentication_revoked(error):raise SystemExit('Disposable agent credential revoked')
        print(json.dumps({'event':'control-unavailable','status':error.code}),flush=True)
    except Exception as error:print(json.dumps({'event':'control-unavailable','type':type(error).__name__}),flush=True)
    time.sleep(15)
