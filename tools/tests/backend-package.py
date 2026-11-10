#!/usr/bin/env python3
"""Bounded local packaging check for a reviewed platform commit, never customer source."""
import argparse
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import subprocess
import tarfile
import tempfile

ROOT=Path(__file__).resolve().parents[2]
BUILDKIT='docker.io/moby/buildkit:v0.34.1@sha256:99368f3e46177c6405b9740e4461d91cee004128daeecca78d4ceac50ec1da5c'
ENDPOINT='unix:///var/run/docker.sock'
OWNER_LABEL='dev.webdock.backend-package'


class CleanupBlocked(RuntimeError):
    pass


class OwnedBuilder:
    def __init__(self, root, nonce, source_sha, runner=subprocess.run):
        if not re.fullmatch('[a-f0-9]{16}', nonce):
            raise CleanupBlocked('Invalid ownership nonce')
        self.root, self.runner = root, runner
        info=root.lstat()
        if not root.is_dir() or root.is_symlink() or root.resolve()!=root or info.st_uid!=os.geteuid() or info.st_mode&0o7777!=0o700:
            raise CleanupBlocked('Private ownership directory required')
        self.directory=os.open(root,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW)
        fcntl.flock(self.directory,fcntl.LOCK_EX|fcntl.LOCK_NB)
        self.builder='webdock-backend-check-'+nonce
        self.node=self.builder+'0'
        self.container='buildx_buildkit_'+self.node
        self.volume=self.container+'_state'
        self.environment={key:value for key,value in os.environ.items() if not key.startswith(('DOCKER_','BUILDX_','BUILDKIT_')) and key!='WSL_DISTRO_NAME'}
        (root/'buildx').mkdir(mode=0o700)
        self.environment['BUILDX_CONFIG']=str(root/'buildx')
        self.record={'nonce':nonce,'builder':self.builder,'node':self.node,'containerName':self.container,'volumeName':self.volume,'endpoint':ENDPOINT,'image':BUILDKIT,'sourceSHA':source_sha,'trustedPlatformCodeOnly':True,'customerCode':False,'memoryBytes':8*1024**3,'swapBytes':0,'cpuCores':4,'pidsLimit':1024,'creationIntent':False,'pendingMutation':None,'container':None,'volume':None,'builderObserved':False,'cleanup':False}
        self.save()

    def save(self):
        with tempfile.NamedTemporaryFile(mode='w',dir=self.root,delete=False) as temporary:
            name=temporary.name
            try:
                os.fchmod(temporary.fileno(),0o600)
                temporary.write(json.dumps(self.record,indent=2)+'\n')
                temporary.flush();os.fsync(temporary.fileno())
                os.replace(name,self.root/'ownership.json')
                os.fsync(self.directory)
            finally:
                if os.path.exists(name):os.unlink(name)

    def docker(self, args, timeout=30, stream=False):
        result=self.runner(['docker','--host',ENDPOINT,*args],cwd=ROOT,env=self.environment,timeout=timeout,check=False,**({} if stream else {'capture_output':True,'text':True}))
        if result.returncode:
            raise subprocess.CalledProcessError(result.returncode,result.args)
        return result.stdout or ''

    def mutate(self, phase, args, timeout=30, stream=False):
        self.record['pendingMutation']=phase;self.save()
        output=self.docker(args,timeout,stream)
        self.record['pendingMutation']=None;self.save()
        return output

    def daemon(self):
        identity=json.loads(self.docker(['info','--format','{{json .}}'])).get('ID')
        if not isinstance(identity,str) or not identity or self.record.get('daemonID',identity)!=identity:
            raise CleanupBlocked('Docker daemon identity changed')
        self.record['daemonID']=identity;self.save()

    def observed_builder(self):
        rows=[json.loads(row) for row in self.docker(['buildx','ls','--format','json']).splitlines() if row.strip()]
        found=[row for row in rows if row.get('Name')==self.builder]
        if not found:return False
        if len(found)!=1 or found[0].get('Driver')!='docker-container' or [(node.get('Name'),node.get('Endpoint')) for node in found[0].get('Nodes',[])]!=[(self.node,ENDPOINT)]:
            raise CleanupBlocked('Foreign builder metadata')
        self.record['builderObserved']=True;self.save()
        return True

    def observed_container(self):
        ids=set(self.docker(['container','ls','--all','--no-trunc','--filter','name=^/'+self.container+'$','--format','{{.ID}}']).split())
        bound=self.record['container']
        known_id=self.record.get('containerID')
        if known_id:
            ids.update(self.docker(['container','ls','--all','--no-trunc','--filter','id='+known_id,'--format','{{.ID}}']).split())
        if not ids:return None
        if len(ids)!=1 or not all(re.fullmatch('[a-f0-9]{64}',value) for value in ids):
            raise CleanupBlocked('Foreign container identity')
        rows=json.loads(self.docker(['container','inspect',next(iter(ids))]))
        if len(rows)!=1:raise CleanupBlocked('Incomplete container inventory')
        current=rows[0]
        candidate=current.get('Id')
        if not isinstance(candidate,str) or not re.fullmatch('[a-f0-9]{64}',candidate) or (known_id is not None and candidate!=known_id):
            raise CleanupBlocked('Container ID changed')
        if known_id is None:
            self.record['containerID']=candidate;self.save()
        host=current['HostConfig']
        snapshot={'ID':current['Id'],'Name':current['Name'],'ImageID':current['Image'],'ImageRef':current['Config']['Image'],'OwnerMarker':current['Config'].get('Env',[]).count('SPITZLI_BACKEND_PACKAGE_OWNER='+self.record['nonce'])==1,'Mounts':[{key:mount.get(key) for key in ['Type','Name','Destination','RW']} for mount in current.get('Mounts',[])],'Budgets':{key:host.get(key) for key in ['Memory','MemorySwap','CpuPeriod','CpuQuota','Privileged']}}
        if bound is None:
            self.record['container']=snapshot;self.save()
        elif bound!=snapshot:
            raise CleanupBlocked('Container identity or configuration changed')
        if snapshot['Name']!='/'+self.container or not re.fullmatch('[a-f0-9]{64}',snapshot['ID']) or not re.fullmatch('sha256:[a-f0-9]{64}',snapshot['ImageID']) or snapshot['ImageRef']!=BUILDKIT or not snapshot['OwnerMarker'] or snapshot['Mounts']!=[{'Type':'volume','Name':self.volume,'Destination':'/var/lib/buildkit','RW':True}]:
            raise CleanupBlocked('Container ownership not proved')
        return current

    def observed_volume(self):
        names=self.docker(['volume','ls','--filter','name=^'+self.volume+'$','--format','{{.Name}}']).split()
        if not names:return False
        if names!=[self.volume]:raise CleanupBlocked('Foreign volume inventory')
        rows=json.loads(self.docker(['volume','inspect',self.volume]))
        if len(rows)!=1:raise CleanupBlocked('Incomplete volume inventory')
        snapshot={key:rows[0].get(key) for key in ['Name','Driver','Scope','CreatedAt','Mountpoint','Labels','Options']}
        bound=self.record['volume']
        if bound is None:
            self.record['volume']=snapshot;self.save()
        elif bound!=snapshot:
            raise CleanupBlocked('Volume identity changed')
        if snapshot['Name']!=self.volume or snapshot['Driver']!='local' or snapshot['Scope']!='local' or snapshot['Labels']!={OWNER_LABEL:self.record['nonce']} or not isinstance(snapshot['CreatedAt'],str) or not snapshot['CreatedAt'] or not isinstance(snapshot['Mountpoint'],str) or not Path(snapshot['Mountpoint']).is_absolute() or snapshot['Options'] not in [None,{}]:
            raise CleanupBlocked('Volume ownership not proved')
        return True

    def start(self):
        self.daemon()
        if self.observed_builder() or self.observed_container() or self.observed_volume():
            raise CleanupBlocked('Packaging names must be absent before creation')
        self.record['creationIntent']=True;self.save()
        self.mutate('volume-create',['volume','create','--driver','local','--label',OWNER_LABEL+'='+self.record['nonce'],self.volume])
        self.observed_volume()
        self.mutate('builder-create',['buildx','create','--name',self.builder,'--node',self.node,'--driver','docker-container','--driver-opt','image='+BUILDKIT,'--driver-opt','env.SPITZLI_BACKEND_PACKAGE_OWNER='+self.record['nonce'],'--driver-opt','memory=8g,memory-swap=8g,cpu-period=100000,cpu-quota=400000',ENDPOINT])
        if not self.observed_builder():raise CleanupBlocked('Builder creation not observed')
        self.mutate('bootstrap',['buildx','inspect','--bootstrap',self.builder],timeout=180)
        current=self.observed_container()
        if current is None or self.record['container']['Budgets']!={'Memory':8*1024**3,'MemorySwap':8*1024**3,'CpuPeriod':100000,'CpuQuota':400000,'Privileged':True}:
            raise CleanupBlocked('Builder resource budget differs')
        self.mutate('pids-update',['update','--pids-limit','1024',current['Id']])
        if self.observed_container()['HostConfig'].get('PidsLimit')!=1024:raise CleanupBlocked('Builder PID budget differs')

    def cleanup(self):
        self.daemon()
        builder=self.observed_builder()
        container=self.observed_container()
        volume=self.observed_volume()
        pending=self.record['pendingMutation']
        if not self.record['creationIntent'] or (pending=='volume-create' and not volume) or (pending=='builder-create' and not builder) or (pending=='bootstrap' and not container):
            raise CleanupBlocked('Creation outcome remains uncertain')
        consumers=set(self.docker(['container','ls','--all','--no-trunc','--filter','volume='+self.volume,'--format','{{.ID}}']).split())
        if consumers-({container['Id']} if container else set()):raise CleanupBlocked('Foreign state-volume consumer')
        if builder:
            self.mutate('builder-remove',['buildx','rm','--force',self.builder],timeout=60)
        elif container:
            self.mutate('container-remove',['container','rm','--force',container['Id']],timeout=60)
        if self.observed_container() is not None:raise CleanupBlocked('Container remains present')
        if self.observed_volume():
            if self.docker(['container','ls','--all','--no-trunc','--filter','volume='+self.volume,'--format','{{.ID}}']).strip():raise CleanupBlocked('State volume remains consumed')
            self.mutate('volume-remove',['volume','rm',self.volume])
        self.daemon()
        if self.observed_builder() or self.observed_container() or self.observed_volume():raise CleanupBlocked('Owned resources remain present')
        self.record['cleanup']=True;self.record['pendingMutation']=None;self.save()

    def cleanup_safely(self):
        try:
            self.cleanup()
            return True
        except BaseException as failure:
            self.record['cleanup']=False;self.record['cleanupFailure']=type(failure).__name__
            try:self.save()
            except OSError:pass
            return False

    def run(self, operation):
        try:
            self.start()
            operation()
        except BaseException as failure:
            self.record['failure']=type(failure).__name__
            self.record['result']=getattr(failure,'returncode',1)
            self.cleanup_safely()
            raise
        else:
            if not self.cleanup_safely():raise CleanupBlocked('Packaging cleanup blocked; retain ownership evidence')
        finally:
            os.close(self.directory)

def command(args,**options):
    return subprocess.run(args,check=True,cwd=ROOT,**options)

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--trusted-platform-source',action='store_true',required=True)
    parser.add_argument('--source-sha',required=True)
    args=parser.parse_args()
    if not re.fullmatch('[a-f0-9]{40}',args.source_sha):parser.error('Exact reviewed commit SHA required')
    origin=subprocess.check_output(['git','remote','get-url','origin'],cwd=ROOT,text=True).strip()
    if origin not in ['git@github.com:spitzli/webdock.git','https://github.com/spitzli/webdock.git']:parser.error('Only the platform repository is supported')
    available=next(int(line.split()[1]) for line in Path('/proc/meminfo').read_text().splitlines() if line.startswith('MemAvailable:'))
    if available<12*1024*1024:parser.error('At least12GiB available host memory required for the bounded8GiB check')
    nonce=secrets.token_hex(8);root=ROOT/'.runtime-test/backend-package'/nonce;root.mkdir(mode=0o700,parents=True)
    source=root/'source';source.mkdir(mode=0o700)
    archive=root/'source.tar'
    with archive.open('wb') as output:command(['git','archive','--format=tar',args.source_sha],stdout=output)
    with tarfile.open(archive) as files:files.extractall(source,filter='data')
    archive.unlink()
    owner=OwnedBuilder(root,nonce,args.source_sha)
    print('Private packaging evidence:',root,flush=True)
    def build():
        output=root/'backend.oci.tar'
        if not owner.observed_builder():raise CleanupBlocked('Builder disappeared before build')
        owner.mutate('build',['buildx','build','--builder',owner.builder,'--platform','linux/amd64','--file',str(source/'deploy/docker/backend.Dockerfile'),'--target','backend','--build-arg','SOURCE_REVISION='+args.source_sha,'--provenance=false','--sbom=false','--output','type=oci,dest='+str(output),str(source)],timeout=3600,stream=True)
        owner.record['result']=0;owner.record['bytes']=output.stat().st_size
        with output.open('rb') as file:owner.record['sha256']=hashlib.file_digest(file,'sha256').hexdigest()
        owner.save()
    owner.run(build)
    raise SystemExit(owner.record.get('result',1))

if __name__=='__main__':main()
