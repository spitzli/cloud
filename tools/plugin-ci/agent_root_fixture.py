#!/usr/bin/env python3
"""One-off reviewed BUILD-only infrastructure fixture. Never normal CI or RUN."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tempfile
import threading
import time
import uuid

DMI='e6d9884c-e6bd-4699-8a0c-99f2358ae8a1'
API='https://10.83.0.1:6443'
CA_SHA='a1fa015c59dca9c67aeb8934430f7f90c5f9bafa41faac2f7079c7bad55ef292'
SOURCE_SHA='69a30142893179ca00745d5d34748ab7b6fb848f6334060a4724cd6f9a1c5e5d'
FIXTURE_SHA='d4ed62d7145ed91d50ab81a9dc43003c2357760fcdd327cda4d4436b6794ab29'
TEST='owned_k3s_cross_node_update_preserves_data_env_and_rejects_raced_binding'
IMAGE='ghcr.io/spitzli/tabularium@sha256:4627facbbdf045716949195b115c7ab5d823e8c2d810d8e3dda93eeaafd9e78f'
IMAGE_TAG='ghcr.io/spitzli/tabularium:0.25.2-spitzli-native-admission.3'
NODE='webdock-build-eu-01'
CONFIG=Path('/etc/rancher/k3s/k3s.yaml')
BASE=Path('/opt/spitzli-plugin-ci/legacy-attachment-fixture')
STAGING=Path('/opt/spitzli-plugin-ci/agent-fixture-qualification')


def require(condition,message):
    if not condition:raise ValueError(message)


def command(args):
    return subprocess.check_output(args,text=True,stderr=subprocess.PIPE,timeout=30)


def kubectl(*args):
    return command(['/usr/local/bin/k3s','kubectl','--kubeconfig',str(CONFIG),'--context','default',*args])


def pins():
    require(os.geteuid()==0,'Reviewed fixture requires authorized BUILD root execution')
    require(Path('/sys/class/dmi/id/product_uuid').read_text().strip().lower()==DMI,'Not the approved BUILD physical system')
    meta=CONFIG.lstat()
    require(CONFIG.resolve()==CONFIG and stat.S_ISREG(meta.st_mode) and meta.st_uid==0 and meta.st_nlink==1 and meta.st_mode&0o077==0,'Owned fixed BUILD-only Kubeconfig required')
    require(kubectl('config','view','--minify','--output=jsonpath={.clusters[0].cluster.server}').strip()==API,'Not the approved BUILD Kubernetes API')
    ca=Path('/var/lib/rancher/k3s/server/tls/server-ca.crt');meta=ca.lstat()
    require(not ca.is_symlink() and meta.st_uid==0 and hashlib.sha256(ca.read_bytes()).hexdigest()==CA_SHA,'Independent BUILD server CA pin mismatch')
    node=json.loads(kubectl('get','node',NODE,'-o','json'))
    require(node['metadata']['labels']['kubernetes.io/hostname']==NODE,'Unexpected Kubernetes hostname label')
    info=node['status']['nodeInfo']
    require(info['systemUUID'].lower()==DMI and info['machineID']==Path('/etc/machine-id').read_text().strip(),'Node physical identity mismatch')
    return {'dmiUUID':DMI,'api':API,'publicServerCAPEMSHA256':CA_SHA,'node':NODE}


def owned_directory(path):
    require(path.parent.exists() and path.parent.resolve()==path.parent,'Canonical fixed parent required')
    for ancestor in [path.parent,*path.parent.parents]:
        meta=ancestor.lstat();require(not ancestor.is_symlink() and meta.st_uid==0 and meta.st_mode&0o022==0,'Untrusted fixed-directory ancestor')
    if not path.exists():path.mkdir(mode=0o700)
    meta=path.lstat();require(not path.is_symlink() and stat.S_ISDIR(meta.st_mode) and meta.st_uid==0 and stat.S_IMODE(meta.st_mode)==0o700,'Fixed fixture directory must be owned root0700')


def inventory():
    data=json.loads(kubectl('get','namespaces,persistentvolumes','-o','json'))
    return sorted((item['kind'],item['metadata']['name'],item['metadata']['uid']) for item in data['items'])


def collect_owned_pods(path,stop,before):
    existing={name for kind,name,_ in before if kind=='Namespace'}
    with path.open('x') as output:
        path.chmod(0o600)
        while not stop.is_set():
            try:
                namespaces=json.loads(kubectl('get','namespaces','-l','tests.webdock.dev/owner','-o','json'))['items']
                for namespace in namespaces:
                    name=namespace['metadata']['name']
                    if name in existing:continue
                    pods=json.loads(kubectl('get','pods','-n',name,'-o','json'))['items']
                    for pod in pods:
                        item={'namespace':name,'pod':pod['metadata']['name'],'status':pod.get('status'),'containers':[{key:container.get(key) for key in ['name','image','command','args','resources','securityContext']} for container in pod['spec']['containers']]}
                        try:item['syntheticLogTail']=kubectl('logs','-n',name,pod['metadata']['name'],'--tail=20').splitlines()
                        except Exception:pass
                        output.write(json.dumps(item)+'\n');output.flush()
            except Exception:pass
            stop.wait(1)


def run(test_binary,expected):
    proof=pins()  # All physical/API/CA checks precede every host/Kubernetes write.
    require(re.fullmatch(r'[a-f0-9]{64}',expected),'Expected reviewed test-binary checksum required')
    with test_binary.open('rb') as source:require(hashlib.file_digest(source,'sha256').hexdigest()==expected,'Compiled reviewed fixture checksum mismatch')
    owned_directory(STAGING);owned_directory(BASE)
    stage=Path(tempfile.mkdtemp(prefix='source-'+SOURCE_SHA[:8]+'-',dir=STAGING));stage.chmod(0o700)
    binary=stage/'legacy-attachment-test'
    with test_binary.open('rb') as source,binary.open('xb') as target:shutil.copyfileobj(source,target)
    binary.chmod(0o500)
    require(hashlib.sha256(binary.read_bytes()).hexdigest()==expected,'Staged root-owned fixture checksum mismatch')
    versions=command(['/usr/bin/readelf','--version-info','--wide',str(binary)])
    required=sorted({tuple(map(int,value.split('.'))) for value in re.findall(r'GLIBC_([0-9]+(?:\.[0-9]+){1,2})',versions)})
    require(required and required[-1]<=(2,39) and 'GLIBC_PRIVATE' not in versions,'Fixture ABI exceeds conservative policy')
    libraries=command(['/usr/bin/ldd',str(binary)]);require('not found' not in libraries,'Fixture shared libraries missing on actual BUILD')
    env={'PATH':'/usr/local/bin:/usr/bin:/bin','HOME':'/root','LANG':'C.UTF-8'}
    listing=subprocess.run([str(binary),'--list','--ignored'],env=env,capture_output=True,text=True,timeout=15)
    require(listing.returncode==0 and TEST+': test' in listing.stdout,'Exact ignored reviewed ROOT fixture missing')
    rows=command(['/usr/local/bin/k3s','ctr','images','list'])
    row=next((value for value in rows.splitlines() if value.startswith(IMAGE_TAG+' ')),None)
    require(row is not None and IMAGE.split('@')[1] in row,'Exact approved image is not cached on BUILD')
    refs=command(['/usr/local/bin/k3s','ctr','images','list','-q']).splitlines()
    if IMAGE not in refs:command(['/usr/local/bin/k3s','ctr','images','tag',IMAGE_TAG,IMAGE])
    fixture_root=BASE/str(uuid.uuid4());require(not fixture_root.exists(),'Fresh canonical fixture root required')
    before=inventory()
    diagnostic=stage/'owned-pod-diagnostics.jsonl';stop=threading.Event()
    collector=threading.Thread(target=collect_owned_pods,args=(diagnostic,stop,before),daemon=True);collector.start()
    env.update({'WEBDOCK_OWNED_K3S_RETAINED_TEST':'1','WEBDOCK_OWNED_K3S_CONFIG':str(CONFIG),'WEBDOCK_OWNED_K3S_CONTEXT':'default','WEBDOCK_OWNED_K3S_ROOT':str(fixture_root),'WEBDOCK_OWNED_K3S_NODE':NODE,'WEBDOCK_OWNED_K3S_IMAGE':IMAGE})
    log=stage/'fixture.log'
    with log.open('xb') as output:
        log.chmod(0o600)
        try:result=subprocess.run([str(binary),'--exact',TEST,'--ignored','--nocapture','--test-threads=1'],env=env,stdout=output,stderr=subprocess.STDOUT,timeout=300)
        except subprocess.TimeoutExpired:result=subprocess.CompletedProcess([],124)
    stop.set();collector.join(timeout=35)
    after=inventory();raw=log.read_text()
    passed=result.returncode==0 and 'test result: ok. 1 passed; 0 failed;' in raw
    cleanup=before==after and not fixture_root.exists()
    proof.update({'sourceArchiveSHA256':SOURCE_SHA,'reviewedFixtureSourceSHA256':FIXTURE_SHA,'testBinarySHA256':expected,'rootOwnedStagedBinary':str(binary),'rootExecutionUID':0,'fixtureLog':str(log),'ownedPodDiagnostics':str(diagnostic),'fixtureRoot':str(fixture_root),'fixturePassed':passed,'fixtureCleanupVerified':cleanup,'namespacePVInventoryUnchanged':before==after,'exitCode':result.returncode,'image':IMAGE,'liveKubernetesFixtureExecuted':True,'productionActivationPerformed':False,'maximumRequiredGLIBC':'.'.join(map(str,required[-1])),'resolvedLibraries':libraries.splitlines()})
    (stage/'fixture-proof.json').write_text(json.dumps(proof,indent=2)+'\n');(stage/'fixture-proof.json').chmod(0o600)
    print(json.dumps({'proof':str(stage/'fixture-proof.json'),'fixturePassed':passed,'cleanupVerified':cleanup,'productionActivationPerformed':False}))
    require(passed and cleanup,'Reviewed fixture failed or cleanup needs inspection; private log '+str(log))
    return proof


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--test-binary',type=Path,required=True);parser.add_argument('--expected-test-sha256',required=True);args=parser.parse_args()
    try:run(args.test_binary,args.expected_test_sha256)
    except Exception as error:raise SystemExit(str(error)) from None
