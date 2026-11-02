#!/usr/bin/env python3
"""Connected, fresh-only Core/gRPC/native CNPG backup fixture. All output is private."""
import argparse
import base64
import importlib.util
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import subprocess
import sys
import time
from qualification import Fixture, ROOT, NETWORK, owned_cluster
from qualification_core import ROLES
from qualification_admission import documents

class Connected(Fixture):
 def prepare_connected(self):
  self.prepare()
  node=self.s['name']
  self.docker('update','--cpus','6','--memory','8g','--memory-swap','8g',node)
  self.s['connectedNodeProfile']={'cpuMillicores':6000,'memoryBytes':8589934592};self.save()
  address=json.loads(self.docker('inspect',node))[0]['NetworkSettings']['Networks'][self.network]['IPAddress']
  self.s['nodeIP']=address;self.save()
  pin=self.s['versions']['CNPG_IMAGE']
  self.k('set','env','deployment/cnpg-controller-manager','-n','cnpg-system','OPERATOR_IMAGE_NAME='+pin)
  self.k('rollout','status','deployment/cnpg-controller-manager','-n','cnpg-system','--timeout=180s',timeout=190)
  # TLS terminates only on this owned node. The signed HTTP request is forwarded
  # byte-for-byte to the existing MinIO, preserving Host and SigV4.
  for title,args in [('ca',['req','-x509','-newkey','rsa:2048','-nodes','-keyout',str(self.out/'ca.key'),'-out',str(self.out/'ca.crt'),'-days','2','-subj','/CN=Owned backup qualification CA','-addext','basicConstraints=critical,CA:TRUE','-addext','keyUsage=critical,keyCertSign,cRLSign']),('csr',['req','-newkey','rsa:2048','-nodes','-keyout',str(self.out/'s3.key'),'-out',str(self.out/'s3.csr'),'-subj','/CN=Owned MinIO TLS fixture'])]:
   self.run(['openssl',*args])
  (self.out/'s3.ext').write_text('subjectAltName=IP:'+address+'\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nauthorityKeyIdentifier=keyid,issuer\nsubjectKeyIdentifier=hash\n')
  self.run(['openssl','x509','-req','-in',str(self.out/'s3.csr'),'-CA',str(self.out/'ca.crt'),'-CAkey',str(self.out/'ca.key'),'-CAcreateserial','-out',str(self.out/'s3.crt'),'-days','2','-extfile',str(self.out/'s3.ext')])
  self.docker('exec',node,'mkdir','-m','700','/connected')
  for name in ['ca.crt','ca.key','s3.crt','s3.key']:
   (self.out/name).chmod(0o600)
   self.docker('cp',str(self.out/name),node+':/connected/'+name)
  self.docker('exec',node,'chown','0:0','/connected/ca.crt','/connected/ca.key','/connected/s3.crt','/connected/s3.key')
  self.docker('exec',node,'chmod','600','/connected/ca.key','/connected/s3.key')
  proxy=Path(__file__).with_name('qualification_tls_proxy.py')
  self.docker('cp',str(proxy),node+':/connected/tls_proxy.py')
  upstream=self.s['endpoint'].split('//')[1].split(':')[0]
  self.docker('exec','-d',node,'python3','/connected/tls_proxy.py',upstream)
  self.s['httpsEndpoint']='https://'+address+':9443';self.save()
  self.apply([{'apiVersion':'v1','kind':'Secret','metadata':{'name':'connected-ca','namespace':'cert-manager'},'type':'kubernetes.io/tls','data':{'tls.crt':base64.b64encode((self.out/'ca.crt').read_bytes()).decode(),'tls.key':base64.b64encode((self.out/'ca.key').read_bytes()).decode()}},{'apiVersion':'cert-manager.io/v1','kind':'ClusterIssuer','metadata':{'name':'connected-ca'},'spec':{'ca':{'secretName':'connected-ca'}}}])
  spec=importlib.util.spec_from_file_location('boundary',ROOT/'infra/postgres/boundary.py');boundary=importlib.util.module_from_spec(spec);spec.loader.exec_module(boundary)
  self.apply(boundary.documents([self.s['versions'][k] for k in ['CNPG_IMAGE','POSTGRES_IMAGE','PGBOUNCER_IMAGE']]+[self.s['images']['sidecar']]))
  self.apply(documents(self.s['images']['plugin'],self.s['images']['sidecar']))
  # Only this fresh Core PostgreSQL shares the disposable node's loopback network.
  administrator=secrets.token_hex(32);passwords={role:secrets.token_hex(32) for role in ROLES}
  core='wd-pg-core-'+self.s['nonce'];self.s['coreName']=core;self.s['coreAdministrator']=administrator;self.s['rolePasswords']=passwords
  self.s['masterKey']=secrets.token_hex(32);self.s['rlsKey']=secrets.token_hex(32);self.save()
  env={**os.environ,'POSTGRES_PASSWORD':administrator}
  core_id=self.docker('create','--name',core,'--label','webdock.backup.fixture='+self.s['nonce'],'--network','container:'+node,'--env','POSTGRES_PASSWORD','--env','POSTGRES_DB=webdock_rls_test','--memory','1g','--memory-swap','1g','--cpus','2','--pids-limit','512','--tmpfs','/var/lib/postgresql:rw,nosuid,nodev,size=1g','sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336',env=env).strip()
  self.s['coreID']=core_id;self.save();self.docker('start',core_id)
  self.wait(lambda:self.docker('exec',core_id,'pg_isready','-U','postgres',check=False) and self.last_exit_code==0,90)
  self.migrate_core()
  self.s['phase']='connected-prepared';self.save()
  print(json.dumps({'phase':self.s['phase'],'privateOutput':str(self.out),'cluster':node}),flush=True)
 def migrate_core(self):
  import socket, threading, select
  # The offline migrator deliberately accepts loopback targets only.
  listener=socket.socket();listener.bind(('127.0.0.1',0));listener.listen(16)
  port=listener.getsockname()[1]
  def relay(client):
   with client, socket.create_connection((self.s['nodeIP'],5432)) as upstream:
    while True:
     ready,_,_=select.select([client,upstream],[],[],60)
     if not ready:return
     for source in ready:
      data=source.recv(65536)
      if not data:return
      (upstream if source is client else client).sendall(data)
  def accept():
   try:
    while True:
     client,_=listener.accept();threading.Thread(target=relay,args=(client,),daemon=True).start()
   except OSError:pass
  threading.Thread(target=accept,daemon=True).start()
  password_file=self.out/'role-passwords.json';password_file.write_text(json.dumps(self.s['rolePasswords']));password_file.chmod(0o600)
  env={**os.environ,'WEBDOCK_MIGRATION_DATABASE_URL':f'postgresql://postgres:{self.s["coreAdministrator"]}@127.0.0.1:{port}/webdock_rls_test','DATABASE_RLS_SECRET':self.s['rlsKey'],'BETTER_AUTH_SECRET':self.s['masterKey']}
  try:self.run(['cargo','run','--locked','-p','webdock-control-plane','--bin','webdock-migrate','--','--target',f'127.0.0.1:{port}/webdock_rls_test','--initialize-empty','--provision-roles-from',str(password_file)],env=env,timeout=600)
  finally:listener.close()
  self.docker('exec',self.s['coreID'],'psql','-U','postgres','-d','webdock_rls_test','-c','INSERT INTO webdock_auth.snowflake_state(singleton) VALUES(true)')
 def binary(self):
  build=self.run(['cargo','test','--locked','-p','webdock-agent-control','--test','database_backup_connected','--no-run','--message-format=json'],timeout=600)
  artifacts=[json.loads(line) for line in build.splitlines() if line.startswith('{')]
  binary=next(Path(a['executable']) for a in artifacts if a.get('reason')=='compiler-artifact' and a.get('executable') and a['target']['name']=='database_backup_connected')
  self.docker('exec',self.s['name'],'mkdir','-p','/connected/lib')
  self.docker('cp',str(binary),self.s['name']+':/connected/fixture')
  programs=[binary,*[Path(v) for v in ['/usr/bin/mount','/usr/bin/umount','/usr/bin/findmnt','/usr/bin/sync','/usr/sbin/losetup','/usr/sbin/blkid','/usr/sbin/mkfs.ext4','/usr/bin/psql','/usr/bin/pg_dump']]]
  libraries=set()
  self.docker('exec',self.s['name'],'mkdir','-p','/connected/tools','/usr/bin','/usr/sbin')
  for program in programs:
   dependencies=subprocess.check_output(['ldd',str(program)],text=True)
   libraries.update(re.findall(r'(?:=>\s+|^\s*)(/[^\s]+)',dependencies,re.M))
   if program!=binary:
    self.docker('cp',str(program.resolve()),self.s['name']+':/connected/tools/'+program.name)
    wrapper='#!/bin/sh\nexec /connected/lib/ld-linux-x86-64.so.2 --library-path /connected/lib /connected/tools/'+program.name+' "$@"\n'
    self.docker('exec','-i',self.s['name'],'sh','-c','cat > '+str(program)+'; chmod 755 '+str(program),input=wrapper.encode())
  for library in libraries:self.docker('cp',str(Path(library).resolve()),self.s['name']+':/connected/lib/'+Path(library).name)
  self.s['fixtureBinarySHA256']=__import__('hashlib').sha256(binary.read_bytes()).hexdigest();self.save()
 def pin_fixture_images(self):
  from qualification import PROOF, PUBLIC_PIN
  self.run(['python3',str(ROOT/'infra/postgres/backup/provenance.py'),'verify','--output',str(PROOF),'--public-key-sha256',PUBLIC_PIN,'--require-images'],timeout=300)
  evidence=json.loads((ROOT/'docs/verification/2026-10-25-barman-local-source-provenance.json').read_text())
  for item in evidence['images']:
   ref=item['name'].split(':')[0]+'@'+item['manifestDigest']
   # Kubelet may collect an imported image after the source is hibernated.
   self.docker('exec','-i',self.s['name'],'ctr','-n','k8s.io','images','import','-',input=Path(item['ociTar']).read_bytes(),timeout=180)
   self.docker('exec',self.s['name'],'ctr','-n','k8s.io','images','tag','--force',item['name'],ref)
   for image in [item['name'],ref]:self.docker('exec',self.s['name'],'ctr','-n','k8s.io','images','label',image,'io.cri-containerd.pinned=pinned')
 def run_test(self,phase):
  self.binary()
  self.pin_fixture_images()
  p=self.s['rolePasswords']
  def dsn(role):return f'postgresql://{role}:{p[role]}@127.0.0.1:5432/webdock_rls_test'
  config={'nonce':self.s['nonce'],'nodeIP':self.s['nodeIP'],'endpoint':self.s['httpsEndpoint'],'bucket':self.s['bucket'],'write':self.s['write'],'read':self.s['read'],'versions':self.s['versions'],'images':self.s['images']}
  if phase=='restore':
   config.pop('write');config.pop('read')
  config_path=self.out/'connected-config.json';config_path.write_text(json.dumps(config));config_path.chmod(0o600)
  self.docker('cp',str(config_path),self.s['name']+':/connected/config.json')
  env={**os.environ,'DATABASE_URL':dsn('webdock_auth_runtime'),'AUTH_DATABASE_URL':dsn('webdock_auth_bootstrap'),'WEBDOCK_PLUGIN_WORKER_DATABASE_URL':dsn('webdock_worker_runtime'),'WEBDOCK_TEST_MIGRATION_DATABASE_URL':f'postgresql://postgres:{self.s["coreAdministrator"]}@127.0.0.1:5432/webdock_rls_test','BETTER_AUTH_SECRET':self.s['masterKey'],'DATABASE_RLS_SECRET':self.s['rlsKey'],'WEBDOCK_CONNECTED_BACKUP_TEST':'yes','WEBDOCK_CONNECTED_BACKUP_PHASE':phase,'RUST_MIN_STACK':'16777216'}
  import threading
  stop=threading.Event();errors=[]
  def faults():
   last=None
   try:
    while not stop.wait(1):
     raw=self.docker('exec',self.s['name'],'cat','/connected/fault-request.json',check=False)
     if not raw:continue
     request=json.loads(raw)
     if request==last:continue
     assert request['nonce']==self.s['nonce'] and request['action'] in ['revoke','resume']
     self.mc('admin','user','disable' if request['action']=='revoke' else 'enable','fixture',self.s['read']['access'])
     self.docker('exec','-i',self.s['name'],'sh','-c','umask 077; cat > /connected/fault-ack.json',input=json.dumps(request).encode())
     last=request
   except Exception as error:errors.append(type(error).__name__)
  if phase=='restore':self.docker('exec',self.s['name'],'rm','-f','/connected/fault-request.json','/connected/fault-ack.json')
  worker=threading.Thread(target=faults,daemon=True) if phase=='restore' else None
  if worker:worker.start()
  try:
   self.docker('exec',*[arg for key in env if key.startswith('WEBDOCK_') or key in ['DATABASE_URL','AUTH_DATABASE_URL','BETTER_AUTH_SECRET','DATABASE_RLS_SECRET','RUST_MIN_STACK'] for arg in ['--env',key]],self.s['name'],'/connected/lib/ld-linux-x86-64.so.2','--library-path','/connected/lib','/connected/fixture','--ignored','--nocapture','--test-threads=1',env=env,timeout=1400)
  finally:
   stop.set()
   if worker:
    worker.join(timeout=45)
    self.mc('admin','user','enable','fixture',self.s['read']['access'])
  if errors:raise RuntimeError('Owned fault-controller failed: '+','.join(errors))
  public=self.docker('exec',self.s['name'],'cat','/connected/evidence.json')
  self.s['connectedEvidence']=json.loads(public);self.save();print(public,flush=True)
 def archive_core(self):
  # Copy only backup/identity recovery artifacts into a separately retained bundle.
  bundle=self.out/'recovery-bundle';bundle.mkdir(mode=0o700)
  ownership=self.docker('exec',self.s['coreID'],'psql','-X','-Atq','-U','postgres','-d','webdock_rls_test','-c',"SELECT string_agg(format('ALTER FUNCTION %s OWNER TO %I;',p.oid::regprocedure,r.rolname),E'\\n' ORDER BY p.oid) FROM pg_proc p JOIN pg_depend d ON d.classid='pg_proc'::regclass AND d.objid=p.oid AND d.deptype='e' JOIN pg_extension e ON e.oid=d.refobjid JOIN pg_roles r ON r.oid=p.proowner WHERE e.extname='pgcrypto'")
  (bundle/'extension-ownership.sql').write_text(ownership);(bundle/'extension-ownership.sql').chmod(0o600)
  roles=self.docker('exec',self.s['coreID'],'pg_dumpall','-U','postgres','--roles-only')
  (bundle/'roles.sql').write_text(roles);(bundle/'roles.sql').chmod(0o600)
  dump=self.docker('exec',self.s['coreID'],'pg_dump','-U','postgres','-d','webdock_rls_test','--format=p')
  (bundle/'core.sql').write_text(dump);(bundle/'core.sql').chmod(0o600)
  (bundle/'identity.json').write_text(json.dumps({k:self.s[k] for k in ['masterKey','rlsKey','coreAdministrator','rolePasswords']}));(bundle/'identity.json').chmod(0o600)
  for name in ['ca.crt','ca.key']:
   shutil.copyfile(self.out/name,bundle/name);(bundle/name).chmod(0o600)
  issuer=self.obj('clusterissuer','connected-ca');ca_secret=self.obj('secret','connected-ca','cert-manager')
  assert base64.b64decode(ca_secret['data']['tls.crt'])==(bundle/'ca.crt').read_bytes()
  self.s['sourceIssuerIdentity']={'issuerUID':issuer['metadata']['uid'],'secretUID':ca_secret['metadata']['uid'],'caSHA256':__import__('hashlib').sha256((bundle/'ca.crt').read_bytes()).hexdigest()}
  self.k('delete','clusterissuer','connected-ca');self.k('delete','secret','connected-ca','-n','cert-manager')
  self.docker('cp',self.s['name']+':/connected/restore-selection.json',str(bundle/'restore-selection.json'))
  self.docker('cp',self.s['name']+':/connected/custody.json',str(self.out/'custody.json'))
  # Destroy the original Core runtime and metadata, rather than borrowing its pools.
  self.s['recoveryBundleSHA256']={p.name:__import__('hashlib').sha256(p.read_bytes()).hexdigest() for p in bundle.iterdir()}
  self.s['sourceCoreID']=self.s['coreID']
  self.docker('rm','-f',self.s['coreID']);assert not self.docker('ps','-a','--filter','id='+self.s['coreID'],'--format','{{.ID}}').strip();self.s['sourceCoreDestroyed']=True;self.save()
  self.docker('exec',self.s['name'],'rm','-f','/connected/restore-selection.json','/connected/config.json','/connected/ca.key','/connected/ca.crt','/connected/setup.json','/connected/source-order.json')
  (self.out/'role-passwords.json').unlink(missing_ok=True)
  (self.out/'connected-config.json').unlink(missing_ok=True)
  (self.out/'ca.key').unlink(missing_ok=True)
  for key in ['masterKey','rlsKey','coreAdministrator','rolePasswords']:self.s.pop(key,None)
  self.save()
 def recover_core(self):
  bundle=self.out/'recovery-bundle';identity=json.loads((bundle/'identity.json').read_text());self.s.update(identity)
  assert self.s['sourceCoreDestroyed']
  assert __import__('hashlib').sha256((bundle/'ca.crt').read_bytes()).hexdigest()==self.s['sourceIssuerIdentity']['caSHA256']
  self.docker('cp',str(bundle/'ca.crt'),self.s['name']+':/connected/ca.crt');self.docker('exec',self.s['name'],'chown','0:0','/connected/ca.crt')
  self.apply([{'apiVersion':'v1','kind':'Secret','metadata':{'name':'connected-ca','namespace':'cert-manager'},'type':'kubernetes.io/tls','data':{'tls.crt':base64.b64encode((bundle/'ca.crt').read_bytes()).decode(),'tls.key':base64.b64encode((bundle/'ca.key').read_bytes()).decode()}},{'apiVersion':'cert-manager.io/v1','kind':'ClusterIssuer','metadata':{'name':'connected-ca'},'spec':{'ca':{'secretName':'connected-ca'}}}])
  recovered_issuer=self.obj('clusterissuer','connected-ca');assert recovered_issuer['metadata']['uid']!=self.s['sourceIssuerIdentity']['issuerUID']
  self.s['recoveredIssuerUID']=recovered_issuer['metadata']['uid']
  env={**os.environ,'POSTGRES_PASSWORD':identity['coreAdministrator']}
  name='wd-pg-recovered-core-'+self.s['nonce']+'-'+secrets.token_hex(3);self.s['coreName']=name
  self.s['coreID']=self.docker('create','--name',name,'--label','webdock.backup.fixture='+self.s['nonce'],'--network','container:'+self.s['name'],'--env','POSTGRES_PASSWORD','--env','POSTGRES_DB=webdock_rls_test','--memory','1g','--memory-swap','1g','--cpus','2','--pids-limit','512','--tmpfs','/var/lib/postgresql:rw,nosuid,nodev,size=1g','sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336',env=env).strip();self.save()
  self.docker('start',self.s['coreID']);self.wait(lambda:self.docker('exec',self.s['coreID'],'pg_isready','-U','postgres',check=False) and self.last_exit_code==0,90)
  roles=(bundle/'roles.sql').read_text()
  # The already-existing bootstrap superuser is the only expected duplicate.
  roles=roles.replace('CREATE ROLE postgres;', '')
  self.docker('exec','-i',self.s['coreID'],'psql','-v','ON_ERROR_STOP=1','-U','postgres','-d','webdock_rls_test',input=roles.encode())
  self.docker('exec','-i',self.s['coreID'],'psql','-v','ON_ERROR_STOP=1','-U','postgres','-d','webdock_rls_test',input=(bundle/'core.sql').read_bytes(),timeout=180)
  self.recover_extension_ownership()
  self.docker('cp',str(bundle/'restore-selection.json'),self.s['name']+':/connected/restore-selection.json')
  self.s['recoveredCoreFromPrivateBundle']=True;self.save()
 def recover_extension_ownership(self):
  bundle=self.out/'recovery-bundle'
  path=bundle/'extension-ownership.sql'
  if not path.exists():
   # Existing diagnostic bundle predates the ownership inventory. The checked-in
   # bootstrap invariant is independent of the destroyed source; no ACL is broadened.
   sql="DO $$ DECLARE f record; BEGIN FOR f IN SELECT p.oid::regprocedure::text AS name FROM pg_proc p JOIN pg_depend d ON d.classid='pg_proc'::regclass AND d.objid=p.oid AND d.deptype='e' JOIN pg_extension e ON e.oid=d.refobjid WHERE e.extname='pgcrypto' LOOP EXECUTE 'ALTER FUNCTION '||f.name||' OWNER TO webdock_policy_owner'; END LOOP; END $$;"
   path.write_text(sql);path.chmod(0o600)
   self.s['extensionOwnershipRecovery']='pinned native bootstrap invariant; source already destroyed'
  elif path.read_text().lstrip().startswith('DO $$'):self.s['extensionOwnershipRecovery']='pinned native bootstrap invariant; original diagnostic bundle predated inventory capture'
  else:self.s['extensionOwnershipRecovery']='separately retained extension ownership inventory'
  self.docker('exec','-i',self.s['coreID'],'psql','-v','ON_ERROR_STOP=1','-U','postgres','-d','webdock_rls_test',input=path.read_bytes())
  self.s['extensionOwnershipSHA256']=__import__('hashlib').sha256(path.read_bytes()).hexdigest();self.save()
 def cleanup_connected(self):
  node=self.s['name']
  if self.s.get('coreID'):
   info=json.loads(self.docker('inspect',self.s['coreID'],check=False) or '[]')
   if info:
    assert info[0]['Config']['Labels']['webdock.backup.fixture']==self.s['nonce']
    self.docker('rm','-f',self.s['coreID'])
  info=json.loads(self.docker('inspect',node,check=False) or '[]')
  if info:
   assert owned_cluster(info[0],node,self.s['nonce'])
   raw=self.docker('exec',node,'cat','/connected/custody.json',check=False)
   custody=json.loads(raw) if self.last_exit_code==0 else {'namespaces':[]}
   if custody['namespaces']:self.k('delete','namespace',*custody['namespaces'],'--ignore-not-found=true','--wait=true','--timeout=120s',timeout=130)
   records=self.docker('exec',node,'cat','/connected/volumes/_pool/state.json',check=False)
   records=json.loads(records)['volumes'] if self.last_exit_code==0 else {}
   loop_checks=[]
   for installation in records:
    assert re.fullmatch('[1-9][0-9]{0,18}',installation)
    volume='/connected/volumes/'+installation+'/volume.ext4';mount='/connected/volumes/'+installation+'/mount'
    attached=self.docker('exec',node,'losetup','-j',volume).strip()
    if attached:
     self.docker('exec',node,'umount',mount)
     remaining=self.docker('exec',node,'losetup','-j',volume).strip()
     if remaining:self.docker('exec',node,'losetup','-d',remaining.split(':',1)[0])
     assert not self.docker('exec',node,'losetup','-j',volume).strip()
    loop_checks.append({'installationID':installation,'hadLoop':bool(attached),'remaining':False})
   self.s['nativeLoopChecks']=loop_checks;self.s['nativeVolumesDetached']=sum(c['hadLoop'] for c in loop_checks);self.save()
  sensitive=[self.s.get(k,'') for k in ['masterKey','rlsKey','coreAdministrator']]+list(self.s.get('rolePasswords',{}).values())
  for path in self.out.glob('command-*.log'):
   data=path.read_bytes()
   for secret in filter(None,sensitive):data=data.replace(secret.encode(),b'[REDACTED]')
   path.write_bytes(data)
  self.cleanup()
  for path in ['recovery-bundle','role-passwords.json','connected-config.json','ca.key','s3.key']:
   target=self.out/path
   if target.is_dir():shutil.rmtree(target)
   else:target.unlink(missing_ok=True)
  for key in ['masterKey','rlsKey','coreAdministrator','rolePasswords']:self.s.pop(key,None)
  self.s['cleanup']['recoveryBundleDestroyed']=True;self.save()

if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('phase',choices=['prepare','backup','archive_core','recover_core','restore','cleanup']);p.add_argument('--output',required=True);a=p.parse_args()
 f=Connected(a.output,create=a.phase=='prepare')
 try:
  if a.phase=='prepare':f.prepare_connected()
  elif a.phase in ['backup','restore']:f.run_test(a.phase)
  elif a.phase=='cleanup':f.cleanup_connected()
  else:getattr(f,a.phase)()
 except Exception:
  f.s['failed']=True;f.save();raise
