#!/usr/bin/env python3
"""Owned full-pipeline storage fixture; only the already-qualified dedicated BUILD host."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import secrets
import subprocess
import uuid

ROOT = Path(__file__).resolve().parents[2]
SSH = ["ssh", "-F", "/dev/null", "-o", "IdentityAgent=none", "-o", "IdentitiesOnly=yes", "-i", str(Path.home()/".ssh/id_ed25519"), "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "-o", "StrictHostKeyChecking=yes", "-o", "UserKnownHostsFile="+str(Path.home()/".ssh/known_hosts")]
HOST = "root@167.86.75.144"

REMOTE = r'''
import base64,hashlib,json,os,socket,subprocess,time
from pathlib import Path
p=PAYLOAD
assert os.geteuid()==0 and socket.gethostname()=='webdock-build-eu-01' and Path('/etc/webdock-build-host').is_file()
work=Path(p['work']); assert work.parent==Path('/var/lib/webdock-build-native') and work.name==p['run']
def k(*args,value=None):
 r=subprocess.run(['/usr/local/bin/k3s','kubectl',*args],input=None if value is None else json.dumps(value),capture_output=True,text=True,timeout=30)
 if r.returncode:raise RuntimeError('owned storage API operation failed: '+args[0])
 return json.loads(r.stdout) if r.stdout.strip().startswith('{') else r.stdout.strip()
def save(name,value):
 path=work/name;fd=os.open(path,os.O_CREAT|os.O_EXCL|os.O_WRONLY,0o600)
 with os.fdopen(fd,'w') as f:json.dump(value,f);f.flush();os.fsync(f.fileno())
def idle():
 for kind in ['pods','jobs','persistentvolumeclaims']:
  assert not k('get',kind,'-n','webdock-build-qualification','-o','json')['items'],'active execution resources; retain fixture'
if p['action']=='start':
 idle(); assert not work.exists()
 work.mkdir(mode=0o700,parents=True)
 unit='webdock-build-native-storage-'+p['run']
 save('owner.json',{'run':p['run'],'unit':unit,'target':p['target'],'pool':p['pool'],'priorPolicy':p['priorPolicy']})
 for name in ['node','helper']:
  data=base64.b64decode(p['binaries'][name]);assert hashlib.sha256(data).hexdigest()==p['hashes'][name]
  fd=os.open(work/name,os.O_CREAT|os.O_EXCL|os.O_WRONLY,0o700)
  with os.fdopen(fd,'wb') as f:f.write(data)
 pool=Path(p['pool']);pool.mkdir(mode=0o700)
 save('pool.json',{'root':str(pool),'node':'webdock-build-eu-01','clusterID':p['clusterID'],'poolID':p['poolID'],'capacityBytes':68719476736,'reserveBytes':21474836480})
 save('trust.json',{'version':1,'target':p['target'],'publicKey':p['publicKey']})
 r=subprocess.run([str(work/'helper'),'--config',str(work/'pool.json'),'--builds','initialize'],capture_output=True,timeout=30);assert r.returncode==0,'pool initialize failed'
 ca=base64.b64decode(k('config','view','--raw','-o','json')['clusters'][0]['cluster']['certificate-authority-data'])
 token=k('create','token','webdock-build-storage-node','-n','webdock-build-storage-system','--duration=2h')
 save('client.json',{'apiVersion':'v1','kind':'Config','clusters':[{'name':'owned-node','cluster':{'server':'https://10.83.0.1:6443','certificate-authority-data':base64.b64encode(ca).decode()}}],'users':[{'name':'node','user':{'token':token}}],'contexts':[{'name':'owned-node','context':{'cluster':'owned-node','user':'node','namespace':'webdock-build-qualification'}}],'current-context':'owned-node'})
 ns=k('get','namespace','webdock-build-qualification','-o','json');quota=k('get','resourcequota','-n','webdock-build-qualification','-o','json')['items'];assert len(quota)==1
 save('prior.json',{'namespaceUID':ns['metadata']['uid'],'annotation':ns['metadata'].get('annotations',{}).get('webdock.dev/storage-profile'),'quota':quota[0]})
 q=quota[0];q['spec']['hard']['persistentvolumeclaims']='4';q['spec']['hard']['requests.storage']='64Gi';k('replace','-f','-','-o','json',value=q)
 k('annotate','namespace','webdock-build-qualification','webdock.dev/storage-profile=fixed-ext4-v1','--overwrite')
 unit='webdock-build-native-storage-'+p['run']
 args=[str(work/'node'),'--config',str(work/'pool.json'),'--trust',str(work/'trust.json'),'--kubeconfig',str(work/'client.json'),'--context','owned-node','--endpoint','https://10.83.0.1:6443','--ca-sha256',hashlib.sha256(ca).hexdigest(),'--namespace','webdock-build-qualification','--requests-namespace','webdock-build-storage-requests','--receipts-namespace','webdock-build-storage-receipts']
 subprocess.run(['systemd-run','--unit',unit,'--property=MemoryMax=256M','--property=TasksMax=128','--property=CPUQuota=50%',*args],check=True,capture_output=True,timeout=20)
 time.sleep(1)
 assert subprocess.run(['systemctl','is-active','--quiet',unit]).returncode==0
 print(json.dumps({'ready':True,'target':p['target']}))
else:
 assert work.stat().st_uid==0 and work.stat().st_mode&0o077==0
 owner=json.loads((work/'owner.json').read_text());assert owner['run']==p['run'] and owner['target']==p['target']
 idle()
 status=subprocess.run(['systemctl','show',owner['unit'],'--property=LoadState','--value'],capture_output=True,text=True,check=True).stdout.strip()
 if status!='not-found':subprocess.run(['systemctl','stop',owner['unit']],check=True,timeout=20)
 idle()
 ledger_path=Path(owner['pool'])/'_pool/builds.json'
 ledger=json.loads(ledger_path.read_text()) if ledger_path.exists() else {'entries':{}}
 assert all(e['phase']=='retired' for e in ledger['entries'].values()),'physical allocations not fully retired'
 assert not list(Path(owner['pool']).glob('*/volume.ext4')),'backing allocation remains'
 for path in Path(owner['pool']).glob('*/mount'):
  assert not subprocess.run(['mountpoint','-q',str(path)]).returncode==0
 if (work/'prior.json').exists():
  prior=json.loads((work/'prior.json').read_text());ns=k('get','namespace','webdock-build-qualification','-o','json');assert ns['metadata']['uid']==prior['namespaceUID']
  if prior['annotation'] is None:k('annotate','namespace','webdock-build-qualification','webdock.dev/storage-profile-')
  else:k('annotate','namespace','webdock-build-qualification','webdock.dev/storage-profile='+prior['annotation'],'--overwrite')
  q=k('get','resourcequota',prior['quota']['metadata']['name'],'-n','webdock-build-qualification','-o','json');assert q['metadata']['uid']==prior['quota']['metadata']['uid'];q['spec']=prior['quota']['spec'];k('replace','-f','-','-o','json',value=q)
 policy=k('get','validatingadmissionpolicy','webdock-build-storage-local-volume','-o','json');assert policy['spec']['validations'] in [p['desiredValidations'],owner['priorPolicy']['spec']['validations']]
 policy['spec']=owner['priorPolicy']['spec'];k('replace','-f','-','-o','json',value=policy)
 (work/'client.json').unlink(missing_ok=True);save('cleaned.json',{'cleaned':True,'retiredVolumes':len(ledger['entries'])})
 print(json.dumps({'cleaned':True,'retiredVolumes':len(ledger['entries'])}))
'''


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action",choices=["start","stop"])
    parser.add_argument("--state-root",type=Path,required=True)
    parser.add_argument("--cluster-id")
    parser.add_argument("--public-key")
    a=parser.parse_args();state=a.state_root.resolve();state.mkdir(parents=True,mode=0o700,exist_ok=True);state.chmod(0o700)
    if a.action=="start":
        assert re.fullmatch(r"[1-9][0-9]{0,18}",a.cluster_id or "")
        assert re.fullmatch(r"[a-f0-9]{64}",a.public_key or "")
        run=secrets.token_hex(10);pool=str(uuid.uuid4());target={"clusterID":a.cluster_id,"node":"webdock-build-eu-01","poolID":pool}
        p={"run":run,"work":"/var/lib/webdock-build-native/"+run,"pool":"/var/lib/webdock-build-native-"+run,"poolID":pool,"clusterID":a.cluster_id,"publicKey":a.public_key,"target":target}
        guard="""import json,os,socket,subprocess;from pathlib import Path
assert os.geteuid()==0 and socket.gethostname()=='webdock-build-eu-01' and Path('/etc/webdock-build-host').is_file()
for kind in ['pods','jobs','persistentvolumeclaims']:
 assert not json.loads(subprocess.check_output(['/usr/local/bin/k3s','kubectl','get',kind,'-n','webdock-build-qualification','-o','json']))['items'],'active source resources'
print(subprocess.check_output(['/usr/local/bin/k3s','kubectl','get','validatingadmissionpolicy','webdock-build-storage-local-volume','-o','json'],text=True))
"""
        prior=subprocess.run([*SSH,HOST,"python3 -"],input=guard,text=True,capture_output=True,check=True,timeout=30)
        p['priorPolicy']=json.loads(prior.stdout)
        owner=state/"storage-owner.json";assert not owner.exists();owner.write_text(json.dumps(p));owner.chmod(0o600)
        import importlib.util
        spec=importlib.util.spec_from_file_location("storage_renderer",ROOT/"deploy/build-cluster/render-storage.py");module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
        manifest={"apiVersion":"v1","kind":"List","items":module.render(pool_root=p['pool'])}
        pv=next(o for o in manifest['items'] if o['kind']=='ValidatingAdmissionPolicy' and o['metadata']['name']=='webdock-build-storage-local-volume')
        manifest['items'].remove(pv)
        subprocess.run([*SSH,HOST,"/usr/local/bin/k3s kubectl apply -f -"],input=json.dumps(manifest),text=True,check=True,stdout=subprocess.DEVNULL,timeout=45)
        # Replace this one owned policy with its observed UID/version; never merge stale last-applied metadata.
        current=json.loads(json.dumps(p['priorPolicy']));current['spec']=pv['spec']
        subprocess.run([*SSH,HOST,"/usr/local/bin/k3s kubectl replace -f -"],input=json.dumps(current),text=True,check=True,stdout=subprocess.DEVNULL,timeout=30)
        binaries={name:(ROOT/"target/x86_64-unknown-linux-musl/release"/binary).read_bytes() for name,binary in [("node","webdock-build-storage-node"),("helper","webdock-storage")]}
        import base64
        p.update(binaries={n:base64.b64encode(b).decode() for n,b in binaries.items()},hashes={n:hashlib.sha256(b).hexdigest() for n,b in binaries.items()})
    else:p=json.loads((state/"storage-owner.json").read_text())
    import importlib.util
    spec=importlib.util.spec_from_file_location("storage_renderer",ROOT/"deploy/build-cluster/render-storage.py");module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    p['desiredValidations']=next(o['spec']['validations'] for o in module.render(pool_root=p['pool']) if o['kind']=='ValidatingAdmissionPolicy' and o['metadata']['name']=='webdock-build-storage-local-volume')
    p['action']=a.action
    result=subprocess.run([*SSH,HOST,"python3 -"],input=REMOTE.replace("PAYLOAD",repr(p)),text=True,capture_output=True,timeout=180)
    (state/(a.action+"-storage.log")).write_text(result.stdout+result.stderr)
    if result.returncode:raise SystemExit("Owned storage fixture failed; keep custody and reconcile: "+str(state))
    if a.action=="start":print(json.dumps(p['target']))
    else:print(result.stdout.strip())


if __name__=="__main__":main()
