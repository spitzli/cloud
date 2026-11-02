#!/usr/bin/env python3
"""Explicitly owned disposable CNPG/Barman/MinIO qualification. Private output only.

No production kubeconfig, stopped cluster, second S3 server or existing volume is used.
"""
import argparse
import base64
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import stat
import subprocess
import time
import urllib.parse
import urllib.request
import yaml

ROOT=Path(__file__).resolve().parents[3]
PROOF=Path('/tmp/webdock-barman-proof-20261025-final')
PUBLIC_PIN='9a037db0b52fa7b4fb4032a882d9d7ebbfa82c527e2a9fa5367533092214ef6a'
MC='quay.io/minio/mc:RELEASE.2025-08-13T08-35-41Z'
NETWORK='webdock-dev_default'
PLUGIN='barman-cloud.cloudnative-pg.io'

def owned_cluster(info,name,nonce):
 return info.get('Name')=='/'+name and info.get('Config',{}).get('Labels',{}).get('webdock.backup.fixture')==nonce

def recovery_target(base,target,timeline):
 if not re.fullmatch(r'[0-9]{8}T[0-9]{6}',base) or type(timeline) is not int or not 1<=timeline<=2147483646:raise ValueError('Exact base backup and numeric timeline required')
 datetime.fromisoformat(target.replace('Z','+00:00'))
 return {'backupID':base,'targetTime':target,'targetTLI':str(timeline)}

class Fixture:
 def __init__(self,out,create=False):
  self.out=Path(out).absolute()
  if create:
   self.out.mkdir(mode=0o700)
   self.s={'nonce':secrets.token_hex(8),'phase':'new','createdAt':datetime.now(timezone.utc).isoformat(),'owned':[]}
   self.s['name']='wd-pg-backup-'+self.s['nonce'];self.save()
  else:
   meta=self.out.lstat()
   if meta.st_uid!=os.getuid() or meta.st_mode & 0o077:raise ValueError('Private owned output required')
   self.s=json.loads((self.out/'state.json').read_text())
  self.deadline=time.monotonic()+2700
 def save(self):
  p=self.out/'state.json';p.write_text(json.dumps(self.s,indent=2));p.chmod(0o600)
 def run(self,args,input=None,timeout=120,env=None,check=True):
  if time.monotonic()>self.deadline:raise TimeoutError('45-minute fixture budget exhausted')
  n=time.time_ns()
  # Commands can contain private SQL or keys. All logs stay inside the private directory.
  started=time.monotonic()
  result=subprocess.run(args,input=input,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=timeout,env=env)
  log=self.out/f'command-{n:04d}.log';log.write_bytes(result.stdout+b'\nSTDERR\n'+result.stderr);log.chmod(0o600)
  with (self.out/'commands.jsonl').open('a') as stream:stream.write(json.dumps({'n':n,'program':args[0],'exitCode':result.returncode,'timeout':timeout,'seconds':round(time.monotonic()-started,3)})+'\n')
  self.last_exit_code=result.returncode
  if result.returncode and check:raise RuntimeError(f'Command {n} failed; private diagnostic: {log}')
  return result.stdout.decode()
 def docker(self,*args,**kw):return self.run(['docker',*args],**kw)
 def k(self,*args,input=None,**kw):return self.docker('exec','-i',self.s['name'],'kubectl',*args,input=input,**kw)
 def obj(self,kind,name,ns=None):return json.loads(self.k('get',kind,name,*(['-n',ns] if ns else []),'-o','json'))
 def apply(self,objects):
  if not isinstance(objects,list):objects=[objects]
  return self.k('apply','--server-side','--field-manager=webdock-backup-qualification','-f','-',input=json.dumps({'apiVersion':'v1','kind':'List','items':objects}).encode(),timeout=180)
 def wait(self,fn,seconds=300):
  end=time.monotonic()+seconds
  while time.monotonic()<end:
   try:
    value=fn()
    if value:return value
   except (RuntimeError,KeyError):pass
   time.sleep(3)
  raise TimeoutError('Owned fixture observation timed out; inspect private logs')
 @property
 def network(self):return self.s.get('network',NETWORK)
 @property
 def minio_container(self):return self.s.get('minioContainer','webdock-dev-minio-1')
 def minio_credentials(self):
  p=ROOT/'.dev-local/minio.env';m=p.lstat()
  assert stat.S_ISREG(m.st_mode) and m.st_uid==os.getuid() and m.st_nlink==1 and not m.st_mode&0o077
  values=dict(line.split('=',1) for line in p.read_text().splitlines() if '=' in line)
  return values['MINIO_ROOT_USER'],values['MINIO_ROOT_PASSWORD']
 def mc(self,*args,input=None,check=True,access=None,secret=None):
  if access is None:access,secret=self.minio_credentials()
  u=urllib.parse.urlsplit(self.s['endpoint'])
  env={**os.environ,'MC_HOST_fixture':f'{u.scheme}://{urllib.parse.quote(access,safe="")}:{urllib.parse.quote(secret,safe="")}@{u.netloc}'}
  return self.docker('run','--rm','--network',self.network,'-i','--env','MC_HOST_fixture',MC,*args,input=input,env=env,check=check)
 def prepare(self):
  if self.s['phase']!='new':raise ValueError('Fresh fixture required')
  self.run(['python3',str(ROOT/'infra/postgres/backup/provenance.py'),'verify','--output',str(PROOF),'--public-key-sha256',PUBLIC_PIN,'--require-images'],timeout=300)
  minio=json.loads(self.docker('inspect','--format','{"Id":{{json .Id}},"State":{"Running":{{json .State.Running}}},"NetworkSettings":{"Networks":{{json .NetworkSettings.Networks}}}}',self.minio_container))
  assert minio['State']['Running']
  address=minio['NetworkSettings']['Networks'][self.network]['IPAddress'];assert address
  self.s['endpoint']=f'http://{address}:9000';self.s['minioID']=minio['Id']
  self.s['bucket']='wd-pg-backup-'+self.s['nonce'];self.save()
  self.mc('mb','fixture/'+self.s['bucket']);self.s['owned'].append(['bucket',self.s['bucket']]);self.save()
  for kind in ['write','read']:
   access='wdpg'+kind+self.s['nonce'];secret=secrets.token_hex(32);policy=access
   self.s[kind]={'access':access,'secret':secret,'policy':policy};self.save()
   self.mc('admin','user','add','fixture',access,secret);self.s['owned'].append(['user',access]);self.save()
   actions=['s3:GetObject']+(['s3:PutObject','s3:AbortMultipartUpload','s3:ListMultipartUploadParts'] if kind=='write' else [])
   policy_body={'Version':'2012-10-17','Statement':[{'Effect':'Allow','Action':actions,'Resource':[f'arn:aws:s3:::{self.s["bucket"]}/postgresql/*']},{'Effect':'Allow','Action':['s3:ListBucket','s3:GetBucketLocation','s3:ListBucketMultipartUploads'],'Resource':[f'arn:aws:s3:::{self.s["bucket"]}']}]}
   self.mc('admin','policy','create','fixture',policy,'/dev/stdin',input=json.dumps(policy_body).encode());self.s['owned'].append(['policy',policy]);self.save()
   self.mc('admin','policy','attach','fixture',policy,'--user',access);self.s['owned'].append(['attachment',[policy,access]]);self.save()
  self.docker('run','-d','--name',self.s['name'],'--label','webdock.backup.fixture='+self.s['nonce'],'--network',self.network,'--privileged','--cpus','4','--memory','6g','--memory-swap','6g','--pids-limit','3072','--env','PATH=/bin/aux:/usr/local/bin:/usr/bin:/usr/sbin:/sbin:/bin','--tmpfs','/run','--tmpfs','/var/run','--stop-timeout','20','webdock-postgres-pilot:c2d4df72','server','--prefer-bundled-bin','--node-name','backup-qualification','--disable','traefik,servicelb,metrics-server','--write-kubeconfig-mode','600',timeout=60)
  self.s['owned'].append(['cluster',self.s['name']]);self.save()
  self.wait(lambda: json.loads(self.k('get','nodes','-o','json'))['items'][0].get('status',{}).get('conditions',[])[-1].get('status')=='True',180)
  cert=self.out/'cert-manager.yaml'
  cert.write_bytes(urllib.request.urlopen('https://github.com/cert-manager/cert-manager/releases/download/v1.21.2/cert-manager.yaml',timeout=60).read())
  self.s['certManagerManifestSHA256']=hashlib.sha256(cert.read_bytes()).hexdigest();self.save()
  self.apply([x for x in yaml.safe_load_all(cert.read_text()) if x]);self.k('rollout','status','deployment/cert-manager-webhook','-n','cert-manager','--timeout=240s',timeout=250)
  cnpg=PROOF/'gomod/github.com/cloudnative-pg/cloudnative-pg@v1.30.1/releases/cnpg-1.30.1.yaml'
  versions=dict(line.split('=',1) for line in (ROOT/'infra/postgres/versions.env').read_text().splitlines() if line and not line.startswith('#'))
  objects=[x for x in yaml.safe_load_all(cnpg.read_text()) if x]
  for obj in objects:
   if obj['kind']=='Deployment':
    container=obj['spec']['template']['spec']['containers'][0];container['image']=versions['CNPG_IMAGE'];container['resources']={'requests':{'cpu':'100m','memory':'128Mi'},'limits':{'cpu':'500m','memory':'512Mi'}}
  self.apply(objects);self.k('rollout','status','deployment/cnpg-controller-manager','-n','cnpg-system','--timeout=240s',timeout=250)
  evidence=json.loads((ROOT/'docs/verification/2026-10-25-barman-local-source-provenance.json').read_text())
  refs={}
  for i,kind in enumerate(['plugin','sidecar']):
   item=evidence['images'][i];ref=item['name'].split(':')[0]+'@'+item['manifestDigest'];refs[kind]=ref
   with Path(item['ociTar']).open('rb') as stream:
    # Each immutable provenance OCI archive is imported only into this task's node.
    self.docker('exec','-i',self.s['name'],'ctr','-n','k8s.io','images','import','-',input=stream.read(),timeout=180)
   self.docker('exec',self.s['name'],'ctr','-n','k8s.io','images','tag',item['name'],ref)
  objects=[x for x in yaml.safe_load_all((PROOF/'src/manifest.yaml').read_text()) if x]
  for obj in objects:
   if obj['kind']=='Secret' and 'SIDECAR_IMAGE' in obj.get('data',{}):obj['data']['SIDECAR_IMAGE']=base64.b64encode(refs['sidecar'].encode()).decode()
   if obj['kind']=='Deployment':
    container=obj['spec']['template']['spec']['containers'][0];container['image']=refs['plugin'];container['imagePullPolicy']='Never';container['resources']={'requests':{'cpu':'100m','memory':'128Mi'},'limits':{'cpu':'500m','memory':'384Mi'}}
  self.apply(objects);self.k('rollout','status','deployment/barman-cloud','-n','cnpg-system','--timeout=240s',timeout=250)
  self.s['versions']=versions;self.s['images']=refs;self.s['phase']='prepared';self.save()
  print(json.dumps({'phase':'prepared','privateOutput':str(self.out),'cluster':self.s['name']}),flush=True)
 def sql(self,ns,sql,database='postgres',role='postgres',password=None,host=None):
  args=['exec','-i','-n',ns,'postgres-1','-c','postgres','--']
  env=[]
  if password:env=['env','PGPASSWORD='+password]
  if host:env+=['PGSSLMODE=verify-full','PGSSLROOTCERT=/controller/run/qualification-server-ca.crt']
  connect_host=host
  if host=='pooler':
   connect_host='postgres-rw';env+=['PGHOSTADDR='+self.obj('service','pooler',ns)['spec']['clusterIP']]
  args+=env+['psql','-X','-v','ON_ERROR_STOP=1','-Atq','-h',connect_host or '/controller/run','-U',role,'-d',database]
  return self.k(*args,input=sql.encode(),timeout=90).strip()
 def storage(self,installation):
  if installation not in [str(n) for n in range(910001,910011)]:raise ValueError('Bounded fixture identity required')
  ns='wd-db-'+installation;path='/qualification/'+installation
  self.docker('exec',self.s['name'],'test','-e',path+'/volume.ext4',check=False)
  if self.last_exit_code==0:raise ValueError('Refusing to reinitialize an existing fixture volume')
  self.docker('exec',self.s['name'],'sh','-ec',f'for n in 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do test -e /dev/loop$n || mknod /dev/loop$n b 7 $n; done; mkdir -p {path}/mount; truncate -s 1610612736 {path}/volume.ext4; mkfs.ext4 -q -F {path}/volume.ext4; mount -o loop {path}/volume.ext4 {path}/mount; mkdir {path}/mount/data; chown 26:26 {path}/mount/data',timeout=90)
  self.apply([{'apiVersion':'v1','kind':'Namespace','metadata':{'name':ns,'labels':{'webdock.dev/database-installation':installation,'pod-security.kubernetes.io/enforce':'restricted'}}},{'apiVersion':'v1','kind':'PersistentVolume','metadata':{'name':ns,'labels':{'webdock.dev/database-installation':installation}},'spec':{'capacity':{'storage':'1536Mi'},'accessModes':['ReadWriteOnce'],'persistentVolumeReclaimPolicy':'Retain','storageClassName':'','local':{'path':path+'/mount/data'},'nodeAffinity':{'required':{'nodeSelectorTerms':[{'matchExpressions':[{'key':'kubernetes.io/hostname','operator':'In','values':['backup-qualification']}]}]}},'claimRef':{'namespace':ns,'name':'postgres-1'}}}])
  # Network policy has an explicit container-reachable MinIO /32; no public fixture port.
  address=urllib.parse.urlsplit(self.s['endpoint']).hostname
  node_ip=json.loads(self.docker('inspect',self.s['name']))[0]['NetworkSettings']['Networks'][self.network]['IPAddress']
  self.apply({'apiVersion':'networking.k8s.io/v1','kind':'NetworkPolicy','metadata':{'name':'fixture-isolation','namespace':ns},'spec':{'podSelector':{},'policyTypes':['Ingress','Egress'],'ingress':[{'from':[{'podSelector':{}}]},{'from':[{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':'cnpg-system'}}}],'ports':[{'port':8000}]}],'egress':[{'to':[{'podSelector':{}}]},{'to':[{'ipBlock':{'cidr':address+'/32'}}],'ports':[{'port':9000}]},{'to':[{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':'kube-system'}}}],'ports':[{'port':53,'protocol':'UDP'},{'port':53,'protocol':'TCP'}]},{'to':[{'ipBlock':{'cidr':'10.43.0.1/32'}},{'ipBlock':{'cidr':node_ip+'/32'}}],'ports':[{'port':443},{'port':6443}]}]}})
  # Quota reserves the source plus recovery/sidecar/init peak inside each independent target.
  self.apply({'apiVersion':'v1','kind':'ResourceQuota','metadata':{'name':'database-budget','namespace':ns},'spec':{'hard':{'requests.cpu':'1800m','limits.cpu':'3','requests.memory':'2Gi','limits.memory':'3Gi','requests.ephemeral-storage':'2Gi','limits.ephemeral-storage':'2Gi','requests.storage':'1536Mi','persistentvolumeclaims':'1','pods':'4'}}})
  return ns
 def store(self,ns,read=False,foreign=False,name="archive"):
  key=self.s['read' if read else 'write'];labels={'webdock.dev/database-installation':ns.removeprefix('wd-db-')}
  self.apply([{'apiVersion':'v1','kind':'Secret','metadata':{'name':name+'-credentials','namespace':ns,'labels':labels},'type':'Opaque','immutable':True,'data':{'ACCESS_KEY_ID':base64.b64encode(key['access'].encode()).decode(),'ACCESS_SECRET_KEY':base64.b64encode(key['secret'].encode()).decode()}},{'apiVersion':'barmancloud.cnpg.io/v1','kind':'ObjectStore','metadata':{'name':name,'namespace':ns,'labels':labels},'spec':{'configuration':{'destinationPath':'s3://'+self.s['bucket']+'/postgresql/'+('foreign' if foreign else self.s['nonce'])+'/'+('' if name=='archive' else name+'/'),'endpointURL':self.s['endpoint'],'s3Credentials':{'accessKeyId':{'name':name+'-credentials','key':'ACCESS_KEY_ID'},'secretAccessKey':{'name':name+'-credentials','key':'ACCESS_SECRET_KEY'}},'wal':{'compression':'gzip'},'data':{'compression':'gzip','jobs':2}},'instanceSidecarConfiguration':{'resources':{'requests':{'cpu':'100m','memory':'128Mi','ephemeral-storage':'256Mi'},'limits':{'cpu':'500m','memory':'256Mi','ephemeral-storage':'256Mi'}},'env':[{'name':'AWS_DEFAULT_REGION','value':'us-east-1'}]}}}])
 def cluster(self,ns,recovery=None):
  installation=ns.removeprefix('wd-db-');labels={'webdock.dev/database-installation':installation}
  resources={'cpu':'250m','memory':'512Mi','ephemeral-storage':'256Mi'}
  cluster={'apiVersion':'postgresql.cnpg.io/v1','kind':'Cluster','metadata':{'name':'postgres','namespace':ns,'labels':labels},'spec':{'instances':1,'smartShutdownTimeout':10,'imageName':self.s['versions']['POSTGRES_IMAGE'],'enableSuperuserAccess':False,'resources':{'requests':resources,'limits':resources},'storage':{'size':'1536Mi','storageClass':'','pvcTemplate':{'volumeName':ns,'accessModes':['ReadWriteOnce']}},'postgresql':{'parameters':{'archive_timeout':'10s','max_wal_size':'128MB','min_wal_size':'32MB','log_statement':'none'},'pg_hba':['hostssl all all all scram-sha-256','host all all all reject']}}}
  if recovery:
   cluster['spec']['bootstrap']={'recovery':{'source':'source','recoveryTarget':recovery}}
   cluster['spec']['externalClusters']=[{'name':'source','plugin':{'name':PLUGIN,'parameters':{'barmanObjectName':'archive','serverName':'wd-'+self.s['nonce']}}}]
  else:
   cluster['spec']['plugins']=[{'name':PLUGIN,'isWALArchiver':True,'parameters':{'barmanObjectName':'archive','serverName':'wd-'+self.s['nonce']}}]
  self.apply(cluster)
  return self.wait(lambda:self.obj('cluster','postgres',ns).get('status',{}).get('readyInstances')==1,420)
 def base_backup(self,ns,name='qualification-base'):
  self.apply({'apiVersion':'postgresql.cnpg.io/v1','kind':'Backup','metadata':{'name':name,'namespace':ns},'spec':{'cluster':{'name':'postgres'},'method':'plugin','pluginConfiguration':{'name':PLUGIN}}})
  def done():
   obj=self.obj('backup',name,ns)
   if obj.get('status',{}).get('phase')=='failed':raise AssertionError('Real base backup failed')
   return obj if obj.get('status',{}).get('phase')=='completed' else False
  backup=self.wait(done,420);self.s['backup']=backup['status'];self.save();return backup
 def qualify(self):
  if self.s['phase']!='prepared':raise ValueError('Prepared fresh fixture required')
  source=self.storage('910001');self.store(source);self.cluster(source)
  self.s['sourceUID']=self.obj('cluster','postgres',source)['metadata']['uid'];self.save()
  password=secrets.token_urlsafe(32);self.s['applicationPassword']=password;self.save()
  self.sql(source,"CREATE ROLE wd_owner_920001 NOLOGIN; CREATE ROLE wd_migrate_920001 LOGIN; CREATE ROLE wd_app_920001 LOGIN; COMMENT ON ROLE wd_owner_920001 IS 'webdock:database:920001'; COMMENT ON ROLE wd_migrate_920001 IS 'webdock:database:920001'; COMMENT ON ROLE wd_app_920001 IS 'webdock:database:920001'; CREATE DATABASE db_920001 OWNER wd_owner_920001; COMMENT ON DATABASE db_920001 IS 'webdock:database:920001';")
  self.sql(source,"ALTER ROLE wd_app_920001 PASSWORD '"+password+"'; ALTER ROLE wd_migrate_920001 PASSWORD '"+password+"';")
  self.sql(source,'CREATE SCHEMA app AUTHORIZATION wd_migrate_920001; CREATE TABLE app.sentinels(id text PRIMARY KEY,value text NOT NULL); ALTER TABLE app.sentinels OWNER TO wd_migrate_920001; GRANT USAGE ON SCHEMA app TO wd_app_920001; GRANT SELECT,INSERT,UPDATE,DELETE ON app.sentinels TO wd_app_920001;',database='db_920001')
  backup=self.base_backup(source);base=backup['status']['backupId']
  self.sql(source,"INSERT INTO app.sentinels VALUES('A','before-target');",database='db_920001')
  target=self.sql(source,"SELECT clock_timestamp()::text;")
  digest_sql=(ROOT/'crates/agent/src/database/backup-digest.sql').read_text()
  expected=self.sql(source,digest_sql,database='db_920001').splitlines()[-1]
  time.sleep(2)
  self.sql(source,"INSERT INTO app.sentinels VALUES('B','after-target');",database='db_920001')
  # Deliberate switch only for the deterministic PITR case, separate from crash RPO below.
  self.sql(source,'SELECT pg_switch_wal();');time.sleep(12)
  self.s['pitr']={'base':base,'targetTime':target,'expectedSHA256':expected};self.save()
  self.k('annotate','cluster','postgres','-n',source,'cnpg.io/hibernation=on','--overwrite')
  self.wait(lambda:not [p for p in json.loads(self.k('get','pods','-n',source,'-o','json'))['items'] if p.get('status',{}).get('phase') not in ['Succeeded','Failed']],180)
  target_ns=self.storage('910002');self.store(target_ns,read=True)
  started=time.monotonic();self.cluster(target_ns,recovery_target(base,target,1));rto=time.monotonic()-started
  self.s['timedPitrRtoSeconds']=rto;self.save()
  self.verify_target(source,target_ns,rto)
 def pitr(self):
  source='wd-db-910001';p=self.s['pitr'];target_ns=self.storage('910003');self.store(target_ns,read=True)
  started=time.monotonic();self.cluster(target_ns,recovery_target(p['base'],p['targetTime'],1));rto=time.monotonic()-started
  self.s['timedPitrRtoSeconds']=rto;self.save();self.verify_target(source,target_ns,rto)
 def verify_target(self,source,target_ns,rto):
  base=self.s['pitr']['base'];target=self.s['pitr']['targetTime'];expected=self.s['pitr']['expectedSHA256'];password=self.s['applicationPassword']
  digest_sql=(ROOT/'crates/agent/src/database/backup-digest.sql').read_text()
  rows=self.sql(target_ns,'SELECT id FROM app.sentinels ORDER BY id;',database='db_920001')
  assert rows=='A','PITR must contain A and exclude B'
  actual=self.sql(target_ns,digest_sql,database='db_920001').splitlines()[-1];assert actual==expected
  # Real direct TLS + transaction-pooler access using recovered SCRAM versions.
  self.apply({'apiVersion':'postgresql.cnpg.io/v1','kind':'Pooler','metadata':{'name':'pooler','namespace':target_ns},'spec':{'cluster':{'name':'postgres'},'instances':1,'type':'rw','pgbouncer':{'poolMode':'transaction','parameters':{'server_tls_sslmode':'verify-full','client_tls_sslmode':'require'}},'template':{'spec':{'initContainers':[{'name':'bootstrap-controller','resources':{'requests':{'cpu':'100m','memory':'128Mi','ephemeral-storage':'128Mi'},'limits':{'cpu':'100m','memory':'128Mi','ephemeral-storage':'128Mi'}}}],'containers':[{'name':'pgbouncer','image':self.s['versions']['PGBOUNCER_IMAGE'],'resources':{'requests':{'cpu':'100m','memory':'128Mi','ephemeral-storage':'128Mi'},'limits':{'cpu':'100m','memory':'128Mi','ephemeral-storage':'128Mi'}}}]}}}})
  self.wait(lambda:self.obj('deployment','pooler',target_ns),60)
  self.k('rollout','status','deployment/pooler','-n',target_ns,'--timeout=180s',timeout=190)
  self.tls(target_ns)
  source_cluster=self.obj('cluster','postgres',source);assert source_cluster['metadata']['uid']==self.s['sourceUID']
  self.s['result']={'fixtureOnly':True,'pitr':{'sourceUnavailable':True,'A_present':True,'B_absent':True,'dataHashMatches':True,'baseBackupID':base,'targetTime':target,'timeline':1,'clusterReadySeconds':round(rto,3)},'rolesAndPasswordVersionRecovered':True,'directAndPoolerSQL':True,'sourceUIDPreserved':True,**{k:v for k,v in self.s.get('result',{}).items() if k not in ['pitr','rolesAndPasswordVersionRecovered','directAndPoolerSQL','sourceUIDPreserved','fixtureOnly']}}
  self.s['phase']='pitr-qualified';self.s['failed']=False;self.save()
  print(json.dumps(self.s['result']),flush=True)
 def full_pitr(self):
  source='wd-db-910001';p=self.s['pitr'];started=time.monotonic()
  installation='910010'
  self.s['fullPitrStartedAt']=datetime.now(timezone.utc).isoformat();self.save()
  ns=self.storage(installation);self.store(ns,read=True)
  cluster_started=time.monotonic();self.cluster(ns,recovery_target(p['base'],p['targetTime'],1));cluster_seconds=time.monotonic()-cluster_started
  self.verify_target(source,ns,cluster_seconds);self.tls(ns)
  self.s['result']['pitr']['productRtoSeconds']=round(time.monotonic()-started,3)
  self.s['result']['pitr']['rtoIncludes']='new bounded volume, material, recovery, hash, direct/pooled SQL, verify-full TLS'
  self.save();print(json.dumps(self.s['result']['pitr']),flush=True)
 def tls(self,ns='wd-db-910003'):
  identity=self.s.get('targetDatabaseIDByNamespace',{}).get(ns,'920001')
  certificate=base64.b64decode(self.k('get','secret','postgres-ca','-n',ns,'-o','jsonpath={.data.ca\\.crt}'))
  self.wait(lambda:(self.k('exec','-i','-n',ns,'postgres-1','-c','postgres','--','sh','-c','cat > /controller/run/qualification-server-ca.crt',input=certificate),True)[1],60)
  for host in ['postgres-rw','pooler']:
   result=self.wait(lambda:self.sql(ns,"SELECT current_user,ssl FROM pg_stat_ssl WHERE pid=pg_backend_pid(); BEGIN; INSERT INTO app.sentinels VALUES('tls-probe','rollback'); SELECT count(*) FROM app.sentinels; ROLLBACK;",database='db_'+identity,role='wd_app_'+identity,password=self.s['applicationPassword'],host=host),60)
   assert 'wd_app_'+identity+'|t' in result and '2' in result
  self.s['result']['tlsVerifyFullDirectAndPooler']=True;self.save()
  print(json.dumps({'tlsVerifyFullDirectAndPooler':True}),flush=True)
 def rpo(self):
  source='wd-db-910001'
  self.k('annotate','cluster','postgres','-n',source,'cnpg.io/fencedInstances=[]','cnpg.io/reconciliationLoop=enabled','cnpg.io/hibernation=off','--overwrite')
  self.wait(lambda:self.obj('cluster','postgres',source).get('status',{}).get('readyInstances')==1,240)
  self.sql(source,'DROP TABLE IF EXISTS app.crash_writer; CREATE TABLE app.crash_writer(sequence integer PRIMARY KEY,committed_at timestamptz NOT NULL); ALTER TABLE app.crash_writer OWNER TO wd_migrate_920001; GRANT SELECT,INSERT ON app.crash_writer TO wd_app_920001;',database='db_920001')
  acknowledged=[]
  for n in range(1,31):
   result=self.sql(source,f'INSERT INTO app.crash_writer VALUES({n},clock_timestamp()) RETURNING sequence,extract(epoch FROM committed_at);',database='db_920001')
   row=result.split('|');acknowledged.append((int(row[0]),float(row[1])));time.sleep(0.7)
  self.s['rpoAcknowledged']=acknowledged;self.save()
  # Disable reconciliation before fencing and force-deleting this source pod. There
  # is no final WAL switch, checkpoint, hibernation or graceful termination.
  self.k('annotate','cluster','postgres','-n',source,'cnpg.io/reconciliationLoop=disabled','--overwrite')
  self.k('delete','pod','postgres-1','-n',source,'--grace-period=0','--force','--wait=true',timeout=90)
  self.k('annotate','cluster','postgres','-n',source,'cnpg.io/fencedInstances=["postgres-1"]','--overwrite')
  crash=self.s['rpoCrashAt']=datetime.now(timezone.utc).isoformat();self.save()
  installation='910005' if self.s.get('rpoCrashAt') else '910004'
  ns=self.storage(installation);self.store(ns,read=True)
  started=time.monotonic();self.cluster(ns,{'backupID':self.s['pitr']['base'],'targetTLI':'1'});rto=time.monotonic()-started
  recovered=self.sql(ns,'SELECT max(sequence),extract(epoch FROM max(committed_at)) FROM app.crash_writer;',database='db_920001')
  last,ts=recovered.split('|');last=int(last);ts=float(ts)
  assert 0<last<=acknowledged[-1][0]
  self.s['result']['unforcedCrashRPO']={'acknowledgedCommits':len(acknowledged),'recoveredLastSequence':last,'lostAcknowledgedCommits':len(acknowledged)-last,'acknowledgedTailSeconds':round(acknowledged[-1][1]-ts,6),'lastRecoveredCommitAgeAtCrashSeconds':round(datetime.fromisoformat(crash).timestamp()-ts,6),'restoreSeconds':round(rto,3),'finalWalSwitch':False,'sourceKilledBeforeFencing':True,'fixtureOnly':True}
  self.s['phase']='rpo-qualified';self.save();print(json.dumps(self.s['result']['unforcedCrashRPO']),flush=True)
 def rename(self):
  ns='wd-db-910010';target='920010';source='920001'
  self.sql(ns,f"ALTER DATABASE db_{source} WITH ALLOW_CONNECTIONS false; SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname='db_{source}'; ALTER DATABASE db_{source} RENAME TO db_{target}; ALTER DATABASE db_{target} WITH ALLOW_CONNECTIONS true; COMMENT ON DATABASE db_{target} IS 'webdock:database:{target}';"+''.join(f"ALTER ROLE {prefix}{source} RENAME TO {prefix}{target}; COMMENT ON ROLE {prefix}{target} IS 'webdock:database:{target}';" for prefix in ['wd_owner_','wd_migrate_','wd_app_']))
  digest=self.sql(ns,(ROOT/'crates/agent/src/database/backup-digest.sql').read_text(),database='db_'+target).splitlines()[-1]
  assert digest==self.s['pitr']['expectedSHA256']
  self.s.setdefault('targetDatabaseIDByNamespace',{})[ns]=target;self.save();self.tls(ns)
  self.sql(ns,'BEGIN; CREATE TABLE app.native_restore_probe(value integer); INSERT INTO app.native_restore_probe VALUES(1); ROLLBACK;',database='db_'+target,role='wd_migrate_'+target,password=self.s['applicationPassword'],host='postgres-rw')
  self.s['result']['newProductIdentityPreservesRolesScramAndData']=True;self.save();print(json.dumps({'newProductIdentityPreservesRolesScramAndData':True}),flush=True)
 def barman(self,command,*args,check=True):
  key=self.s['read'];image=self.s['images']['sidecar'].split('@')[-1]
  environment={**os.environ,'AWS_ACCESS_KEY_ID':key['access'],'AWS_SECRET_ACCESS_KEY':key['secret'],'AWS_DEFAULT_REGION':'us-east-1','AWS_MAX_ATTEMPTS':'1'}
  return self.docker('run','--rm','--name','wd-pg-backup-fault-'+self.s['nonce'],'--label','webdock.backup.fixture='+self.s['nonce'],'--network',self.network,'--memory','256m','--memory-swap','256m','--cpus','1','--pids-limit','128','--cap-drop','ALL','--read-only','--tmpfs','/restore:rw,nosuid,nodev,size=16m,mode=1777','--env','AWS_ACCESS_KEY_ID','--env','AWS_SECRET_ACCESS_KEY','--env','AWS_DEFAULT_REGION','--env','AWS_MAX_ATTEMPTS','--entrypoint','/venv/bin/'+command,image,'--cloud-provider','aws-s3','--endpoint-url',self.s['endpoint'],*args,env=environment,check=check,timeout=120)
 def faults(self):
  result={};prefix='postgresql/'+self.s['nonce']+'/';server='wd-'+self.s['nonce'];url='s3://'+self.s['bucket']+'/'+prefix.rstrip('/')
  self.barman('barman-cloud-restore','s3://'+self.s['bucket']+'/postgresql/foreign',server,self.s['pitr']['base'],'/restore/data',check=False)
  assert self.last_exit_code!=0;result['foreignArchiveRejected']=True
  read=self.s['read']
  self.mc('pipe','fixture/'+self.s['bucket']+'/'+prefix+'read-only-must-not-write',input=b'fixture',access=read['access'],secret=read['secret'],check=False)
  assert self.last_exit_code!=0;result['recoveryCredentialCannotWrite']=True
  self.mc('admin','user','disable','fixture',read['access'])
  try:
   self.barman('barman-cloud-backup-list',url,server,check=False)
   assert self.last_exit_code!=0;result['revokedRecoveryCredentialRejected']=True
  finally:self.mc('admin','user','enable','fixture',read['access'])
  self.barman('barman-cloud-backup-list',url,server);result['recoveryCredentialResumed']=True
  objects=[json.loads(line) for line in self.mc('ls','--recursive','--json','fixture/'+self.s['bucket']).splitlines() if line.strip()]
  key=next(o['key'] for o in objects if re.search(r'/[A-F0-9]{24}\.gz$',o.get('key','')))
  wal=key.rsplit('/',1)[1][:-3]
  source='fixture/'+self.s['bucket']+'/'+key
  quarantine='fixture/'+self.s['bucket']+'/quarantine/'+self.s['nonce']+'/'+key.rsplit('/',1)[1]
  self.mc('mv',source,quarantine)
  try:
   self.barman('barman-cloud-wal-restore',url,server,wal,'/restore/wal',check=False)
   assert self.last_exit_code!=0;result['missingWalRejected']=True
  finally:self.mc('mv',quarantine,source)
  self.barman('barman-cloud-wal-restore',url,server,wal,'/restore/wal');result['walRecoveredAfterRepair']=True
  self.storage('910008')
  probe='/qualification/910008/mount/data/capacity-probe'
  self.docker('exec',self.s['name'],'sh','-c',f'dd if=/dev/zero of={probe} bs=1048576 count=2048 2>/dev/null',check=False,timeout=90)
  assert self.last_exit_code!=0
  written=int(self.docker('exec',self.s['name'],'stat','-c','%s',probe).strip())
  self.docker('exec',self.s['name'],'rm',probe)
  assert 0<written<1610612736;result['boundedFilesystemENOSPC']={'volumeBytes':1610612736,'writtenBeforeENOSPC':written}
  self.s['result']['faults']=result;self.save();print(json.dumps(result),flush=True)
 def schedule(self):
  ns='wd-db-910003';name='archive-schedule';self.store(ns,name=name)
  self.k('patch','cluster','postgres','-n',ns,'--type=merge','-p',json.dumps({'spec':{'plugins':[{'name':PLUGIN,'isWALArchiver':True,'parameters':{'barmanObjectName':name,'serverName':'wd-'+self.s['nonce']+'-schedule-910003'}}]}}))
  self.wait(lambda:self.obj('pod','postgres-1',ns).get('status',{}).get('containerStatuses',[]) and len(self.obj('pod','postgres-1',ns).get('status',{}).get('initContainerStatuses',[]))>=1,180)
  self.apply({'apiVersion':'postgresql.cnpg.io/v1','kind':'ScheduledBackup','metadata':{'name':'authority-recurrence','namespace':ns},'spec':{'schedule':'*/10 * * * * *','cluster':{'name':'postgres'},'method':'plugin','pluginConfiguration':{'name':PLUGIN},'suspend':False,'immediate':False,'backupOwnerReference':'self'}})
  schedule=self.obj('scheduledbackup','authority-recurrence',ns);uid=schedule['metadata']['uid']
  def backups():return [b for b in json.loads(self.k('get','backups','-n',ns,'-o','json'))['items'] if any(o['uid']==uid for o in b['metadata'].get('ownerReferences',[]))]
  done=self.wait(lambda:next((b for b in backups() if b.get('status',{}).get('phase')=='completed'),False),300)
  self.k('patch','scheduledbackup','authority-recurrence','-n',ns,'--type=merge','-p','{"spec":{"suspend":true}}')
  applied=self.obj('scheduledbackup','authority-recurrence',ns);assert applied['metadata']['uid']==uid and applied['spec']['suspend']
  time.sleep(12);before=len(backups());time.sleep(25);after=len(backups());assert before==after
  pod=self.obj('pod','postgres-1',ns);sidecar=next(c for c in pod['spec']['initContainers'] if c['name']=='plugin-barman-cloud')
  assert sidecar['image']==self.s['images']['sidecar'] and sidecar['restartPolicy']=='Always'
  for dimension in ['cpu','memory','ephemeral-storage']:assert dimension in sidecar['resources']['requests'] and dimension in sidecar['resources']['limits']
  result={'installedRBACRecurrenceWithoutSession':True,'completedBaseBackupID':done['status']['backupId'],'suspendObservedUID':uid,'suspendObservedResourceVersion':applied['metadata']['resourceVersion'],'noNewBackupsAfterSuspensionSeconds':25,'restartableSidecarPinnedAndBounded':True,'fixtureCronOnly':'*/10 * * * * *','coreProductSchedule':'daily six-field schedule'}
  self.s['result']['schedule']=result;self.save();print(json.dumps(result),flush=True)
 def cleanup(self):
  errors=[]
  for kind,value in reversed(self.s['owned']):
   try:
    if kind=='cluster':
     info=json.loads(self.docker('inspect',value))[0]
     if not owned_cluster(info,value,self.s['nonce']):raise ValueError('Foreign cluster')
     namespaces=['wd-db-'+str(i) for i in range(910001,910011)]
     self.k('delete','namespace',*namespaces,'--ignore-not-found=true','--wait=true','--timeout=90s',timeout=100)
     self.s['loopCleanup']=[]
     for installation in range(910001,910011):
      volume='/qualification/'+str(installation)+'/volume.ext4';mount='/qualification/'+str(installation)+'/mount'
      attached=self.docker('exec',value,'losetup','-j',volume).strip()
      if attached:
       self.docker('exec',value,'umount',mount)
       remaining=self.docker('exec',value,'losetup','-j',volume).strip()
       if remaining:
        device=remaining.split(':',1)[0]
        if not re.fullmatch(r'/dev/loop[0-9]+',device):raise ValueError('Unrecognized owned loop')
        self.docker('exec',value,'losetup','-d',device)
       if self.docker('exec',value,'losetup','-j',volume).strip():raise ValueError('Owned loop remains')
       self.s['loopCleanup'].append({'installation':str(installation),'detached':True})
     self.save()
     self.docker('rm','-f',value,timeout=90)
     if self.docker('ps','-a','--filter','name=^/'+value+'$','--format','{{.ID}}').strip():raise ValueError('Cluster remains')
    elif kind=='attachment':self.mc('admin','policy','detach','fixture',value[0],'--user',value[1])
    elif kind=='policy':self.mc('admin','policy','rm','fixture',value)
    elif kind=='user':self.mc('admin','user','rm','fixture',value)
    elif kind=='bucket':
     self.mc('rm','--recursive','--force','fixture/'+value);self.mc('rb','fixture/'+value)
    self.s['owned'].remove([kind,value]);self.save()
   except Exception as e:errors.append(type(e).__name__+':'+kind)
  minio=json.loads(self.docker('inspect','--format','{"Id":{{json .Id}},"State":{"Running":{{json .State.Running}}},"NetworkSettings":{"Networks":{{json .NetworkSettings.Networks}}}}',self.minio_container))
  assert minio['Id']==self.s['minioID'] and minio['State']['Running']
  self.s['cleanup']={'errors':errors,'ownedResourcesRemaining':self.s['owned'],'existingMinioPreserved':True};self.save()
  if not errors:
   root_user,root_password=self.minio_credentials()
   sensitive=[root_user,root_password,self.s.get('applicationPassword',''),self.s.get('write',{}).get('secret',''),self.s.get('read',{}).get('secret','')]
   for path in self.out.glob('command-*.log'):
    data=path.read_bytes()
    for value in filter(None,sensitive):data=data.replace(value.encode(),b'[REDACTED]')
    path.write_bytes(data)
   for kind in ['write','read']:self.s.get(kind,{}).pop('secret',None)
   self.s.pop('applicationPassword',None);self.s['cleanup']['fixtureCredentialsDestroyed']=True;self.save()
  if errors:raise RuntimeError('Exact fixture cleanup incomplete: '+','.join(errors))

def main():
 p=argparse.ArgumentParser();p.add_argument('phase',choices=['prepare','qualify','pitr','full_pitr','tls','rpo','faults','schedule','rename','cleanup']);p.add_argument('--output',required=True);a=p.parse_args()
 fixture=Fixture(a.output,create=a.phase=='prepare')
 try:getattr(fixture,a.phase)()
 except Exception:
  fixture.s['failed']=True;fixture.save();raise
if __name__=='__main__':main()
