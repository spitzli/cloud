#!/usr/bin/env python3
"""Check standalone assets under restricted runtime conditions, without credentials."""
import subprocess
import time
import uuid

for component, app, port in [('console', 'console', 3120), ('auth', 'auth', 3125)]:
    name = 'webdock-smoke-' + uuid.uuid4().hex[:12]
    subprocess.run(['docker', 'run', '-d', '--name', name, '--network', 'none', '--read-only', '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges', '--tmpfs', '/tmp:rw,nosuid,size=64m,mode=1777', f'webdock-distribution-{component}:local'], check=True, capture_output=True)
    try:
        script = f"const fs=require('node:fs');const f=fs.readdirSync('/app/apps/{app}/.next/static/chunks').find(f=>f.endsWith('.js'));fetch('http://127.0.0.1:{port}/_next/static/chunks/'+f).then(r=>{{if(r.status!==200)process.exit(1)}}).catch(()=>process.exit(1))"
        for attempt in range(20):
            result = subprocess.run(['docker', 'exec', name, 'node', '-e', script], capture_output=True)
            if result.returncode == 0:
                break
            time.sleep(1)
        assert result.returncode == 0, f'{component}: standalone static assets unavailable'
        print(f'{component}: HTTP 200 static assets, network isolated and read-only')
    finally:
        subprocess.run(['docker', 'rm', '-f', name], check=True, capture_output=True)
