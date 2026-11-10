#!/usr/bin/env python3
"""Owned fixed ext4/PV/PVC qualification on the explicitly selected dedicated BUILD host."""
import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import json
from pathlib import Path
import secrets
import subprocess
import uuid

ROOT = Path(__file__).resolve().parents[2]
HOST = "root@167.86.75.144"
SSH = ["ssh", "-o", "ConnectTimeout=10", "-o", "IdentityAgent=none", "-o", "IdentitiesOnly=yes", "-i", str(Path.home()/".ssh/id_ed25519"), "-o", "BatchMode=yes"]
IMAGE = "docker.io/library/spitzli-build-runner@sha256:81a0228242287a8800d26dbecd68136593ed652590e62123c987e8876b197409"

REMOTE = r'''
import base64,hashlib,json,os,socket,subprocess,time
from pathlib import Path
p=PAYLOAD
assert socket.gethostname()=='webdock-build-eu-01' and os.geteuid()==0 and Path('/etc/webdock-build-host').is_file()
work=Path(p['work']); assert work.parent==Path('/var/lib/webdock-build-volume-proof') and work.name==p['run']
assert work.stat().st_uid==0 and work.stat().st_mode & 0o077==0
namespace='webdock-build-volume-proof-'+p['run']; role='wd-volume-proof-'+p['run']
report={'run':p['run'],'namespace':namespace,'checks':[],'cleanup':False}
ns_uid=None; role_uid=None; binding_uid=None; daemon=None; channels=[]
def k(*args,value=None,check=True):
 r=subprocess.run(['/usr/local/bin/k3s','kubectl',*args],input=None if value is None else json.dumps(value),text=True,capture_output=True,timeout=25)
 if check and r.returncode:raise RuntimeError('owned-volume-api-'+args[0])
 return r
def get(kind,name,ns=True):
 r=k('get',kind,name,*(['-n',namespace] if ns else []),'-o','json',check=False)
 if r.returncode:
  if 'NotFound' in r.stderr:return None
  raise RuntimeError('owned-volume-get')
 return json.loads(r.stdout)
def create(value):return json.loads(k('create','-f','-','-o','json',value=value).stdout)
def save(name,value):
 path=work/name
 fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL,0o600)
 with os.fdopen(fd,'w') as f:json.dump(value,f,separators=(',',':'));f.flush();os.fsync(f.fileno())
 return path
def node(packet,expected=None):
 if p.get('queueMode'):
  signed=p['packets'][packet];request=signed['reservation']
  fingerprint=hashlib.sha256(b'webdock-build-storage-v1\0'+json.dumps(request,separators=(',',':')).encode()).hexdigest()
  name='wd-vq-'+request['volumeID']+'-'+str(request['epoch'])+'-'+fingerprint[:16]
  current=k('get','configmap',name,'-n',channels[0],'-o','json',check=False)
  if current.returncode:
   frame={'apiVersion':'v1','kind':'ConfigMap','metadata':{'name':name,'namespace':channels[0],'labels':{'webdock.dev/component':'build-volume-request','webdock.dev/volume-id':request['volumeID'],'webdock.dev/generation':'1','webdock.dev/epoch':str(request['epoch'])},'annotations':{'webdock.dev/request-sha256':fingerprint}},'immutable':True,'data':{'request.json':json.dumps(signed,separators=(',',':'))}}
   current=k('--kubeconfig',str(work/'producer.json'),'--context','owned-node','create','-f','-','-o','json',value=frame)
  uid=json.loads(current.stdout)['metadata']['uid']
  deadline=time.monotonic()+(4 if expected=='retiring' else 25)
  while time.monotonic()<deadline:
   reply=k('--kubeconfig',str(work/'producer.json'),'--context','owned-node','get','configmap',name,'-n',channels[1],'-o','json',check=False)
   if reply.returncode==0:
    data=json.loads(reply.stdout);answer=json.loads(data['data']['reply.json'])
    assert answer['requestUid']==uid and answer['requestFingerprint']==fingerprint
    for ns in channels:
     k('--kubeconfig',str(work/'producer.json'),'--context','owned-node','delete','configmap',name,'-n',ns,'--wait=true','--timeout=10s')
    if expected=='denied':assert answer['failureCode'] is not None;return None
    assert answer['failureCode'] is None
    result=answer['result'];assert expected is None or result['phase']==expected
    return result
   time.sleep(.25)
  if expected=='retiring':
   ledger=json.loads((work/'pool/_pool/builds.json').read_text());assert ledger['entries'][request['volumeID']]['phase']=='retired'
   return {'phase':'retiring'}
  raise RuntimeError('owned queue reply deadline exceeded')
 r=subprocess.run([str(work/'node'),'--config',str(work/'pool.json'),'--trust',str(work/'trust.json'),'--request',str(work/(packet+'.json')),
  '--kubeconfig',str(work/'client.json'),'--context','owned-node','--endpoint','https://10.83.0.1:6443','--ca-sha256',ca_hash,
  '--namespace',namespace,'--wait-seconds','15'],env={**os.environ,'RUST_LOG':'webdock_agent=debug'},text=True,capture_output=True,timeout=40)
 if expected=='denied':
  assert r.returncode!=0;return None
 if r.returncode:raise RuntimeError('owned-node-'+packet+':'+r.stderr[:250])
 value=json.loads(r.stdout)
 if expected is not None:assert value['phase']==expected,(packet,value['phase'])
 return value
def delete_pod(name):
 pod=get('pod',name)
 if pod is None:return
 assert pod['metadata']['labels']['webdock.dev/volume-proof']==p['run']
 k('delete','pod',name,'-n',namespace,'--wait=true','--timeout=20s')
def probe(name,owner,claim,script):
 pod={'apiVersion':'v1','kind':'Pod','metadata':{'name':name,'namespace':namespace,'labels':{'webdock.dev/volume-proof':p['run'],'webdock.dev/build-id':owner['buildID'],'webdock.dev/generation':'1'}},'spec':{
 'runtimeClassName':'webdock-build-sandbox','automountServiceAccountToken':False,'restartPolicy':'Never','activeDeadlineSeconds':60,'terminationGracePeriodSeconds':1,
 'securityContext':{'runAsNonRoot':True,'runAsUser':1000,'runAsGroup':1000,'fsGroup':1000,'seccompProfile':{'type':'RuntimeDefault'}},
 'volumes':[{'name':'owned','persistentVolumeClaim':{'claimName':claim}}],
 'containers':[{'name':'probe','image':p['image'],'imagePullPolicy':'Never','command':['/bin/sh','-c',script],
 'securityContext':{'readOnlyRootFilesystem':True,'allowPrivilegeEscalation':False,'capabilities':{'drop':['ALL']}},
 'resources':{'requests':{'cpu':'500m','memory':'256Mi','ephemeral-storage':'16Mi'},'limits':{'cpu':'500m','memory':'256Mi','ephemeral-storage':'16Mi'}},
 'volumeMounts':[{'name':'owned','mountPath':'/work','subPath':'workspace'}]}]}}
 return create(pod)
def logs(name):
 r=k('logs',name,'-n',namespace,'--tail=15','--limit-bytes=2000','--pod-running-timeout=1s',check=False)
 return r.stdout if r.returncode==0 else ''
try:
 for binary in ['node','helper']:
  assert hashlib.sha256((work/binary).read_bytes()).hexdigest()==p['binaryHashes'][binary]
 for ns in ['webdock-builds','webdock-build-qualification']:
  assert not json.loads(k('get','pods','-n',ns,'-o','json').stdout)['items'],'active build; refusing fixture'
 (work/'pool').mkdir(mode=0o700)
 save('pool.json',{'root':str(work/'pool'),'node':'webdock-build-eu-01','clusterID':'78','poolID':p['poolID'],'capacityBytes':268435456,'reserveBytes':21474836480})
 save('trust.json',{'version':1,'target':p['target'],'publicKey':p['publicKey']})
 for name,packet in p['packets'].items():save(name+'.json',packet)
 result=subprocess.run([str(work/'helper'),'--config',str(work/'pool.json'),'--builds','initialize'],text=True,capture_output=True,timeout=15)
 assert result.returncode==0,'owned pool initialization'
 ns=create({'apiVersion':'v1','kind':'Namespace','metadata':{'name':namespace,'labels':{'webdock.dev/volume-proof':p['run'],'pod-security.kubernetes.io/enforce':'restricted'}}});ns_uid=ns['metadata']['uid']
 create({'apiVersion':'networking.k8s.io/v1','kind':'NetworkPolicy','metadata':{'name':'deny-all','namespace':namespace},'spec':{'podSelector':{},'policyTypes':['Ingress','Egress']}})
 create({'apiVersion':'v1','kind':'ServiceAccount','metadata':{'name':'storage-node','namespace':namespace},'automountServiceAccountToken':False})
 create({'apiVersion':'rbac.authorization.k8s.io/v1','kind':'Role','metadata':{'name':role,'namespace':namespace},'rules':[
  {'apiGroups':[''],'resources':['persistentvolumeclaims'],'verbs':['get','create','delete']},
  {'apiGroups':[''],'resources':['pods'],'verbs':['list']},{'apiGroups':['batch'],'resources':['jobs'],'verbs':['get']}]})
 create({'apiVersion':'rbac.authorization.k8s.io/v1','kind':'RoleBinding','metadata':{'name':role,'namespace':namespace},'roleRef':{'apiGroup':'rbac.authorization.k8s.io','kind':'Role','name':role},'subjects':[{'kind':'ServiceAccount','name':'storage-node','namespace':namespace}]})
 role_object=create({'apiVersion':'rbac.authorization.k8s.io/v1','kind':'ClusterRole','metadata':{'name':role,'labels':{'webdock.dev/volume-proof':p['run']}},'rules':[
  {'apiGroups':[''],'resources':['persistentvolumes'],'verbs':['create']},
  {'apiGroups':[''],'resources':['persistentvolumes'],'verbs':['get','delete'],'resourceNames':p['volumeNames']}]});role_uid=role_object['metadata']['uid']
 binding=create({'apiVersion':'rbac.authorization.k8s.io/v1','kind':'ClusterRoleBinding','metadata':{'name':role,'labels':{'webdock.dev/volume-proof':p['run']}},'roleRef':{'apiGroup':'rbac.authorization.k8s.io','kind':'ClusterRole','name':role},'subjects':[{'kind':'ServiceAccount','name':'storage-node','namespace':namespace}]});binding_uid=binding['metadata']['uid']
 token=k('create','token','storage-node','-n',namespace,'--duration=15m').stdout.strip()
 admin=json.loads(k('config','view','--raw','-o','json').stdout)
 cluster=admin['clusters'][0]['cluster'];ca=cluster['certificate-authority-data'];ca_hash=hashlib.sha256(base64.b64decode(ca)).hexdigest()
 save('client.json',{'apiVersion':'v1','kind':'Config','contexts':[{'name':'owned-node','context':{'cluster':'build','user':'node','namespace':namespace}}],
  'clusters':[{'name':'build','cluster':{'server':'https://10.83.0.1:6443','certificate-authority-data':ca}}],'users':[{'name':'node','user':{'token':token}}]})
 for action,resource,scope in [('create','pods',namespace),('get','secrets','kube-system'),('get','persistentvolume/foreign-volume',namespace)]:
  denied=k('--kubeconfig',str(work/'client.json'),'--context','owned-node','auth','can-i',action,resource,'-n',scope,check=False)
  assert denied.returncode!=0 and denied.stdout.strip()=='no'
 report['checks'].append('node identity cannot create Pods, read system Secrets or inspect foreign PVs')
 if p.get('queueMode'):
  channels=['webdock-build-storage-requests-'+p['run'],'webdock-build-storage-receipts-'+p['run']]
  create({'apiVersion':'v1','kind':'ServiceAccount','metadata':{'name':'volume-producer','namespace':namespace},'automountServiceAccountToken':False})
  for i,channel in enumerate(channels):
   create({'apiVersion':'v1','kind':'Namespace','metadata':{'name':channel,'labels':{'webdock.dev/volume-proof':p['run'],'webdock.dev/build-storage-channel':'request' if i==0 else 'reply','pod-security.kubernetes.io/enforce':'restricted'}}})
   for account,verbs in [('storage-node',['get','list'] if i==0 else ['get','list','create','delete']),('volume-producer',['get','create','delete'] if i==0 else ['get','delete'])]:
    create({'apiVersion':'rbac.authorization.k8s.io/v1','kind':'Role','metadata':{'name':account,'namespace':channel},'rules':[{'apiGroups':[''],'resources':['configmaps'],'verbs':verbs}]})
    create({'apiVersion':'rbac.authorization.k8s.io/v1','kind':'RoleBinding','metadata':{'name':account,'namespace':channel},'roleRef':{'apiGroup':'rbac.authorization.k8s.io','kind':'Role','name':account},'subjects':[{'kind':'ServiceAccount','name':account,'namespace':namespace}]})
  producer=json.loads((work/'client.json').read_text());producer['users'][0]['user']['token']=k('create','token','volume-producer','-n',namespace,'--duration=15m').stdout.strip();save('producer.json',producer)
  denied=k('--kubeconfig',str(work/'producer.json'),'--context','owned-node','auth','can-i','create','configmaps','-n',channels[1],check=False);assert denied.stdout.strip()=='no'
  report['checks'].append('producer identity cannot forge node receipt ConfigMaps')
  log=(work/'queue.log').open('wb')
  daemon=subprocess.Popen([str(work/'node'),'--config',str(work/'pool.json'),'--trust',str(work/'trust.json'),'--kubeconfig',str(work/'client.json'),'--context','owned-node','--endpoint','https://10.83.0.1:6443','--ca-sha256',ca_hash,'--namespace',namespace,'--requests-namespace',channels[0],'--receipts-namespace',channels[1]],stdout=log,stderr=log)
 a=node('a-reserve','ready');b=node('b-reserve','ready')
 ah=a['handle'];bh=b['handle'];assert ah['pvcUid']!=bh['pvcUid'] and ah['filesystemUuid']!=bh['filesystemUuid']
 report['checks'].append('two distinct signed fixed filesystems and exact PV/PVC UIDs ready with a token-only node identity')
 node('foreign-scope','denied');report['checks'].append('valid signer cannot reassign an existing volume to another tenant')
 probe('writer-a',p['packets']['a-reserve']['reservation'],ah['pvcName'],"dd if=/dev/zero of=/work/fill bs=1048576 count=128 2>&1; code=$?; rm -f /work/fill; printf 'tenant-a' >/work/marker-a; printf 'OWNED_ENOSPC=%s\\n' \"$code\"; sleep 45")
 deadline=time.monotonic()+25
 while time.monotonic()<deadline:
  text=logs('writer-a')
  if 'OWNED_ENOSPC=1' in text and 'No space left on device' in text:break
  time.sleep(0.25)
 else:
  report['writerLogs']=logs('writer-a');report['writerStatus']=get('pod','writer-a').get('status',{})
  report['writerEvents']=json.loads(k('get','events','-n',namespace,'--field-selector','involvedObject.name=writer-a','-o','json').stdout)
  raise RuntimeError('owned writer did not prove ENOSPC')
 node('a-retire','retiring');assert get('pvc',ah['pvcName']) is not None
 report['checks'].append('actual nonroot gVisor PVC writer hit ENOSPC; active consumer blocked claim deletion and filesystem purge')
 delete_pod('writer-a');node('a-retire','retired');node('a-reserve','denied')
 report['checks'].append('exact consumer removal allowed ordered PVC/PV deletion and purge; late reserve replay denied')
 probe('reader-b',p['packets']['b-reserve']['reservation'],bh['pvcName'],"test ! -e /work/marker-a && printf tenant-b >/work/marker-b && printf 'OWNED_TENANT_B\\n'")
 deadline=time.monotonic()+20
 while time.monotonic()<deadline:
  if 'OWNED_TENANT_B' in logs('reader-b'):break
  time.sleep(0.25)
 else:raise RuntimeError('owned tenant B read/write control failed')
 delete_pod('reader-b')
 report['checks'].append('tenant B PVC did not contain tenant A marker and remained writable after A retirement')
 # Wait for kubelet to release the exact source before simulating mount loss.
 mount=work/'pool'/p['packets']['b-reserve']['reservation']['volumeID']/'mount'
 result=subprocess.run(['/usr/bin/umount',str(mount)],capture_output=True,timeout=10);assert result.returncode==0
 node('b-inspect','denied');assert not (mount/'data').exists()
 probe('missing-mount',p['packets']['b-reserve']['reservation'],bh['pvcName'],"printf 'UNEXPECTED_EXECUTION\\n'")
 time.sleep(4)
 missing=get('pod','missing-mount');assert missing['status']['phase']=='Pending' and not logs('missing-mount')
 delete_pod('missing-mount');node('b-renew','ready')
 report['checks'].append('actual unmounted local PV failed closed: no source directory recreated and Pod never executed; signed renewal restored exact filesystem')
 # Deletion/recreation is an operator-only hostile fixture, never node adoption.
 old=get('pvc',bh['pvcName']);k('delete','pvc',bh['pvcName'],'-n',namespace,'--wait=true','--timeout=20s')
 replacement=create({'apiVersion':'v1','kind':'PersistentVolumeClaim','metadata':{'name':bh['pvcName'],'namespace':namespace,'labels':old['metadata']['labels'],'annotations':old['metadata'].get('annotations',{})},'spec':old['spec']})
 assert replacement['metadata']['uid']!=bh['pvcUid'];node('b-renew','denied')
 current=get('pvc',bh['pvcName']);assert current['metadata']['uid']==replacement['metadata']['uid']
 k('delete','pvc',bh['pvcName'],'-n',namespace,'--wait=true','--timeout=20s')
 node('b-retire','retired');report['checks'].append('replacement PVC UID was rejected rather than adopted; original volume retired only after the replacement was removed')
 report['status']='qualified-owned-volume-substrate; controller-queue-and-admission-integration-still-required'
finally:
 if daemon is not None:
  daemon.terminate()
  try:daemon.wait(timeout=10)
  except subprocess.TimeoutExpired:daemon.kill();daemon.wait(timeout=5)
 for pod_name in ['writer-a','reader-b','missing-mount']:
  if ns_uid is not None:delete_pod(pod_name)
 # Never remove uncertain storage or a foreign UID merely to clean a fixture.
 state_path=work/'pool/_pool/state.json'
 purged=state_path.exists() and all(v['state']=='purged' for v in json.loads(state_path.read_text())['volumes'].values())
 if purged:
  for channel in channels:
   current=get('namespace',channel,False);assert current['metadata']['labels']['webdock.dev/volume-proof']==p['run']
   k('delete','namespace',channel,'--wait=true','--timeout=20s')
  if ns_uid is not None:
   current=get('namespace',namespace,False);assert current['metadata']['uid']==ns_uid
   k('delete','namespace',namespace,'--wait=true','--timeout=20s')
  for kind,expected in [('clusterrolebinding',binding_uid),('clusterrole',role_uid)]:
   if expected is not None:
    current=get(kind,role,False);assert current['metadata']['uid']==expected and current['metadata']['labels']['webdock.dev/volume-proof']==p['run']
    k('delete',kind,role)
  assert not any(get('pv',name,False) for name in p['volumeNames'])
  for name in ['client.json','producer.json']:
   (work/name).unlink(missing_ok=True)
  report['cleanup']=True
 else:report['retainedOwnedWork']=str(work)
 print(json.dumps(report,indent=2),flush=True)
'''


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--qualified-build-host",action="store_true",required=True)
    parser.add_argument("--queue",action="store_true")
    options=parser.parse_args()
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    from cryptography.hazmat.primitives.serialization import Encoding,PublicFormat
    run=secrets.token_hex(10)
    directory=ROOT/".runtime-test/build-volume-live"/run
    directory.mkdir(parents=True,mode=0o700);directory.chmod(0o700)
    work="/var/lib/webdock-build-volume-proof/"+run
    pool=str(uuid.uuid4());key=Ed25519PrivateKey.generate()
    target={"clusterID":"78","node":"webdock-build-eu-01","poolID":pool}
    expiration=(datetime.now(timezone.utc)+timedelta(seconds=285)).replace(microsecond=0).strftime("%Y-%m-%dT%H:%M:%SZ")
    ids=[str(secrets.randbelow(10**17)+10**17) for _ in range(2)]
    def signed(customer,project,build,volume,action,epoch):
        reservation={"version":1,"audience":"webdock-build-storage-v1","volumeID":volume,"customerID":customer,"projectID":project,"buildID":build,"generation":1,"target":target,"volumeBytes":67108864,"action":action,"epoch":epoch,"expiresAt":expiration}
        signature=key.sign(b"webdock-build-storage-v1\0"+json.dumps(reservation,separators=(",",":"),ensure_ascii=False).encode()).hex()
        return {"reservation":reservation,"signature":signature}
    packets={}
    for name,action,epoch in [("a-reserve","reserve",1),("a-retire","retire",2)]:packets[name]=signed("12","34","56",ids[0],action,epoch)
    for name,action,epoch in [("b-reserve","reserve",1),("b-inspect","inspect",1),("b-renew","reserve",2),("b-retire","retire",3)]:packets[name]=signed("13","35","57",ids[1],action,epoch)
    packets["foreign-scope"]=signed("99","34","56",ids[0],"reserve",2)
    binaries={"node":ROOT/"target/x86_64-unknown-linux-musl/release/spitzli-build-storage-node","helper":ROOT/"target/x86_64-unknown-linux-musl/release/spitzli-storage"}
    payload={"run":run,"work":work,"poolID":pool,"target":target,"publicKey":key.public_key().public_bytes(Encoding.Raw,PublicFormat.Raw).hex(),"packets":packets,"volumeNames":[f"wd-bv-{build}-1-{vol}" for build,vol in zip(["56","57"],ids)],"binaryHashes":{name:hashlib.sha256(path.read_bytes()).hexdigest() for name,path in binaries.items()},"image":IMAGE}
    setup="import os,socket; from pathlib import Path; assert os.geteuid()==0 and socket.gethostname()=='webdock-build-eu-01' and Path('/etc/webdock-build-host').is_file(); Path("+repr(work)+").mkdir(mode=0o700,parents=True,exist_ok=False)"
    subprocess.run([*SSH,HOST,"python3 -"],input=setup,text=True,check=True,timeout=20)
    scp=["scp","-q","-o","IdentityAgent=none","-o","IdentitiesOnly=yes","-i",str(Path.home()/".ssh/id_ed25519")]
    for name,path in binaries.items():subprocess.run([*scp,str(path),HOST+":"+work+"/"+name],check=True,timeout=45)
    payload['queueMode']=options.queue
    source=REMOTE.replace("PAYLOAD",repr(payload))
    record=directory/"custody.json";record.write_text(json.dumps({"run":run,"work":work,"host":"167.86.75.144","poolID":pool}));record.chmod(0o600)
    log=directory/"qualification.log"
    with log.open("w") as output:
        log.chmod(0o600)
        result=subprocess.run([*SSH,HOST,"python3 -"],input=source,text=True,stdout=output,stderr=subprocess.STDOUT,timeout=350)
    print(log.read_text()[-7500:])
    print("Private evidence:",log)
    if result.returncode:raise SystemExit("Owned volume qualification failed; retain exact work identity for reconciliation")


if __name__=="__main__":main()
