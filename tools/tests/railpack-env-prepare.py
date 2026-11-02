#!/usr/bin/env python3
"""Pinned Railpack prepare-only proof with operator sentinels; never runs an app/build."""
import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[2]
IMAGE='sha256:9e3cad070f793a1b437085c445705af7f9e1a6914cbc929c60af6176ec932d78'
DOCKER=['docker','--context','default']
VALUES={'FIXTURE_REVISION':'owned-public-revision-sentinel-7301','BUILD_TOKEN':'owned-private-build-sentinel-9842'}

def docker(*args):
    result=subprocess.run([*DOCKER,*args],capture_output=True,text=True,timeout=20)
    if result.returncode:raise RuntimeError('owned_prepare_'+args[0]+'_failed')
    return result.stdout.strip()


def main():
    context=json.loads(docker('context','inspect','default'))[0]
    assert context['Endpoints']['docker']['Host'] in ['unix:///var/run/docker.sock',f'unix:///run/user/{os.getuid()}/docker.sock'], 'owned_local_docker_required'
    image=json.loads(docker('image','inspect',IMAGE))[0]
    assert image['Id']==IMAGE and image['Config']['User']=='1000:1000'
    work=Path(tempfile.mkdtemp(prefix='railpack-env-',dir=ROOT/'.runtime-test'));work.chmod(0o700)
    source=work/'source';output=work/'output';source.mkdir(mode=0o700);output.mkdir(mode=0o700)
    (source/'package.json').write_text(json.dumps({'name':'owned-env-prepare','version':'1.0.0','engines':{'node':'24.10.0'},'scripts':{'build':'node build.js','start':'node server.js'}}))
    (source/'build.js').write_text("throw new Error('prepare must never execute this file');\n")
    (source/'server.js').write_text("throw new Error('prepare must never execute this file');\n")
    (source/'railpack.json').write_text(json.dumps({'provider':'shell'}))
    (source/'start.sh').write_text('#!/bin/sh\nexit 99 # prepare must never execute this file\n')
    env=work/'sentinels.env';env.write_text(''.join(name+'='+value+'\n' for name,value in VALUES.items()));env.chmod(0o600)
    run=secrets.token_hex(8);container=None
    script='''set -eu
export HOME=/home/user TMPDIR=/workspace XDG_CACHE_HOME=/cache XDG_CONFIG_HOME=/cache/config
mkdir -p /workspace/runtime /cache/railpack/mise
cp /usr/local/libexec/mise-2026.9.15 /cache/railpack/mise/mise-2026.9.15
ln -s /cache/railpack /tmp/railpack
sha256sum /usr/local/bin/railpack >/output/binary.sha256
exec /usr/local/bin/railpack prepare --env FIXTURE_REVISION --env BUILD_TOKEN --plan-out /output/plan.json --info-out /output/info.json --hide-pretty-plan /source
'''
    try:
        container=docker('create','--pull=never','--label','webdock.railpack-env='+run,'--network=none','--cpus=1','--memory=512m','--memory-swap=512m','--pids-limit=32','--read-only','--cap-drop=ALL','--security-opt','no-new-privileges:true','--env-file',str(env),'--mount','type=bind,src='+str(source)+',dst=/source,readonly','--mount','type=bind,src='+str(output)+',dst=/output','--tmpfs','/workspace:rw,nosuid,nodev,size=16m,uid=1000,gid=1000,mode=0700','--tmpfs','/cache:rw,exec,nosuid,nodev,size=256m,uid=1000,gid=1000,mode=0700','--tmpfs','/tmp:rw,nosuid,nodev,size=16m,uid=1000,gid=1000,mode=0700','--entrypoint','/bin/sh',IMAGE,'-ec',script)
        result=subprocess.run([*DOCKER,'start','--attach',container],capture_output=True,text=True,timeout=40)
        logs=result.stdout+result.stderr
        assert all(value not in logs for value in VALUES.values()),'sentinel value leaked to prepare diagnostics'
        (work/'prepare.log').write_text(logs)
        info=json.loads((output/'info.json').read_text()) if (output/'info.json').exists() else None
        plan=json.loads((output/'plan.json').read_text()) if (output/'plan.json').exists() else None
        observed=json.loads(docker('inspect',container))[0]
        assert observed['HostConfig']['NetworkMode']=='none' and observed['HostConfig']['Memory']==512*1024**2
        assert observed['HostConfig']['MemorySwap']==512*1024**2 and observed['HostConfig']['ReadonlyRootfs']
        assert observed['HostConfig']['CapDrop']==['ALL'] and not observed['HostConfig']['Privileged']
        assert all(value not in json.dumps(observed['Config']['Cmd']) for value in VALUES.values())
        proof={'image':IMAGE,'railpackBinarySHA256':(output/'binary.sha256').read_text().split()[0],'exitCode':observed['State']['ExitCode'],'infoSuccess':info.get('success') if info else None,'planSecrets':plan.get('secrets') if plan else None,'steps':[{key:step[key] for key in ['name','secrets'] if key in step} for step in plan.get('steps',[])] if plan else None,'valuesInPlan':any(value in json.dumps(plan) for value in VALUES.values()) if plan else None,'deployVariables':plan.get('deploy',{}).get('variables') if plan else None,'valuesInDiagnostics':False,'work':str(work),'network':'none','customerCodeExecuted':False}
        assert result.returncode==0 and proof['exitCode']==0 and proof['infoSuccess'] is True, 'pinned_prepare_failed; see private owned diagnostic'
        assert proof['railpackBinarySHA256']=='6c99bdd6d9606261a218fb22f3d663eb72fc483e8dc8c839844c15aa0c2fbdf1'
        assert info['railpackVersion']=='0.40.1'
        assert sorted(proof['planSecrets'])==sorted(VALUES)
        assert proof['valuesInPlan'] is False and all(value not in json.dumps(info) for value in VALUES.values())
        assert not any(name in (proof['deployVariables'] or {}) for name in VALUES)
        proof.update(railpackVersion=info['railpackVersion'],detectedProvider=info.get('metadata',{}).get('providers'),configuredProvider='shell',environmentArguments=['--env','FIXTURE_REVISION','--env','BUILD_TOKEN'],sentinelValuesInArgv=False,valuesInInfo=False,planSHA256=hashlib.sha256((output/'plan.json').read_bytes()).hexdigest(),memoryMaxBytes=512*1024**2,swapMaxBytes=0,pidsLimit=32,readOnlyRoot=True,capabilitiesDropped=['ALL'],prepareOnly=True)
        print(json.dumps(proof,indent=2));(work/'proof.json').write_text(json.dumps(proof,indent=2)+'\n')
    finally:
        if container:
            observed=json.loads(docker('inspect',container))[0]
            assert observed['Id']==container and observed['Config']['Labels']['webdock.railpack-env']==run
            docker('rm','-f','-v',container)
        env.unlink(missing_ok=True)

if __name__=='__main__':main()
