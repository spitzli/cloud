"""Reject alternate runtimes and qualify daemonless clients before source checks."""
import json
import hashlib
import os
from pathlib import Path
import re
import subprocess
import shutil

BINARY_SHA256={'/usr/local/bin/node':'7fde7b8afa198da66257f42ee2001d874c7355631e6d1579a5fb5ef1f246df4c','/usr/local/bin/docker':'6c71cfe71faff949a602946a4df4799b563232e79d0b8b6fd343d0ef081c9621','/usr/local/libexec/docker/cli-plugins/docker-compose':'47737f4236e6d3ccea7291995203f8f018058783b4c3a9469119811a0b5db8c0'}
NPM_CLI='/usr/local/lib/node_modules/npm/bin/npm-cli.js'
NPM_CLI_SHA256='8e5f6f3429f8cdbe693cdc29904e9d5a7b127a494bd15c804bd54c7403bfcbe7'
NO_DAEMON='unix:///nonexistent/manual-quality-no-daemon.sock'

def validate_node(value):
    if not (value.get('runtime')=='node' and value.get('node')=='24.21.0' and value.get('bun') is None and value.get('executable')=='/usr/local/bin/node'):
        raise ValueError('Actual official Node24.21.0 required; Bun-as-Node fallback is rejected')

def validate_npm(value,bundled,executable):
    if value!='11.19.0' or bundled!=value or executable!=NPM_CLI:
        raise ValueError('Bundled npm11.19.0 from the same pinned Node24 image required')


def validate_compose(docker,compose):
    if not re.search(r'^Docker version 29\.9\.2,',docker.strip()) or compose.strip().removeprefix('v')!='5.6.1':
        raise ValueError('Exact official Docker29.9.2/Compose5.6.1 clients required')

def verify(output,compose=False):
    env=dict(os.environ);env['DOCKER_HOST']=NO_DAEMON
    raw=subprocess.check_output(['node','-p',"JSON.stringify({runtime:process.release.name,node:process.versions.node,bun:process.versions.bun||null,executable:process.execPath})"],env=env,text=True,timeout=20)
    node=json.loads(raw);validate_node(node)
    binary_paths=list(BINARY_SHA256) if compose else ['/usr/local/bin/node']
    for path in binary_paths:
        if hashlib.sha256(Path(path).read_bytes()).hexdigest()!=BINARY_SHA256[path]:raise ValueError('Tool binary differs from actual official-image qualification: '+path)
    bun=subprocess.check_output(['bun','--version'],env=env,text=True,timeout=20).strip()
    if bun!='1.4.3':raise ValueError('Pinned Bun1.4.3 required')
    npm=subprocess.check_output(['npm','--version'],env=env,text=True,timeout=20).strip()
    bundled=json.loads(Path('/usr/local/lib/node_modules/npm/package.json').read_text())['version']
    validate_npm(npm,bundled,str(Path(shutil.which('npm') or '/nonexistent').resolve()))
    if hashlib.sha256(Path(NPM_CLI).read_bytes()).hexdigest()!=NPM_CLI_SHA256:raise ValueError('Bundled npm differs from official-image qualification')
    proof={'npmVersion':npm,'npmBundledWithPinnedNode':True,'npmCLISHA256':NPM_CLI_SHA256,'node':node,'bunVersion':bun,'bunAsNodeFallbackUsed':False,'verifiedBinarySHA256':{p:BINARY_SHA256[p] for p in binary_paths}}
    if compose:
        if Path('/var/run/docker.sock').exists():raise ValueError('No Docker daemon socket may enter manual source checks')
        docker=subprocess.check_output(['docker','--version'],env=env,text=True,timeout=20).strip()
        version=subprocess.check_output(['docker','compose','version','--short'],env=env,text=True,timeout=20).strip();validate_compose(docker,version)
        proof.update({'dockerClient':docker,'composeVersion':version,'dockerHost':NO_DAEMON,'dockerDaemonSocketMounted':False})
    (output/'tooling-proof.json').write_text(json.dumps(proof,indent=2)+'\n')
    return proof
