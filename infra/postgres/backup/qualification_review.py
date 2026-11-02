#!/usr/bin/env python3
"""Fresh-only review transition fixture: owned MinIO, Core PG and Kubernetes."""
import argparse
import http.client
import json
import os
from pathlib import Path
import secrets
import time
from qualification_connected import Connected, ROOT

class Review(Connected):
 @property
 def network(self):return self.s['network']
 @property
 def minio_container(self):return self.s['minioContainer']
 def minio_credentials(self):
  return self.s['ownedMinioUser'],self.s['ownedMinioSecret']
 def prepare_review(self):
  assert self.s['phase']=='new'
  self.s['network']='wd-pg-review-net-'+self.s['nonce']
  self.s['minioContainer']='wd-pg-review-minio-'+self.s['nonce']
  self.s['ownedMinioUser']='review'+secrets.token_hex(12)
  self.s['ownedMinioSecret']=secrets.token_hex(32);self.save()
  self.s['ownedNetworkID']=self.docker('network','create','--label','webdock.backup.fixture='+self.s['nonce'],self.network).strip();self.save()
  env={**os.environ,'MINIO_ROOT_USER':self.s['ownedMinioUser'],'MINIO_ROOT_PASSWORD':self.s['ownedMinioSecret']}
  self.s['minioID']=self.docker('create','--name',self.minio_container,'--label','webdock.backup.fixture='+self.s['nonce'],'--network',self.network,'--env','MINIO_ROOT_USER','--env','MINIO_ROOT_PASSWORD','--memory','512m','--memory-swap','512m','--cpus','1','--pids-limit','256','--tmpfs','/data:rw,nosuid,nodev,size=2g,uid=10001,gid=10001,mode=0700','sha256:74d077c5981eeaaf88688deca6dfece7e21d3e1627264d1c2183750380683833','server','/data','--address',':9000','--console-address',':9001',env=env).strip();self.save();self.docker('start',self.s['minioID'])
  info=json.loads(self.docker('inspect',self.s['minioID']))[0];address=info['NetworkSettings']['Networks'][self.network]['IPAddress'];self.s['endpoint']='http://'+address+':9000';self.save()
  def healthy():
   try:
    c=http.client.HTTPConnection(address,9000,timeout=2);c.request('GET','/minio/health/live');response=c.getresponse();good=response.status==200;response.read();c.close();return good
   except OSError:return False
  self.wait(healthy,60)
  self.prepare_connected()
 def registry(self,expected):
  if not self.s.get('registryPrepared'):
   build=self.run(['cargo','test','--locked','-p','webdock-control-plane','--test','database_backup_transitions','--no-run','--message-format=json'],timeout=600)
   binary=next(Path(item['executable']) for item in map(json.loads,filter(lambda line:line.startswith('{'),build.splitlines())) if item.get('reason')=='compiler-artifact' and item.get('executable') and item['target']['name']=='database_backup_transitions')
   self.docker('cp',str(binary),self.s['name']+':/connected/registry-test')
   self.s['registryPrepared']=True;self.save()
  env={**os.environ,'WEBDOCK_BACKUP_LIVE_REGISTRY':'yes','WEBDOCK_BACKUP_ARCHIVE_EXPECT':expected,'AUTH_DATABASE_URL':f'postgresql://webdock_auth_bootstrap:{self.s["rolePasswords"]["webdock_auth_bootstrap"]}@127.0.0.1:5432/webdock_rls_test','WEBDOCK_TEST_MIGRATION_DATABASE_URL':f'postgresql://postgres:{self.s["coreAdministrator"]}@127.0.0.1:5432/webdock_rls_test','WEBDOCK_REGISTRY_DATABASE_URL':f'postgresql://webdock_registry_runtime:{self.s["rolePasswords"]["webdock_registry_runtime"]}@127.0.0.1:5432/webdock_rls_test','BETTER_AUTH_SECRET':self.s['masterKey'],'DATABASE_RLS_SECRET':self.s['rlsKey'],'RUST_MIN_STACK':'16777216'}
  self.docker('exec',*[arg for key in env if key.startswith('WEBDOCK_') or key in ['AUTH_DATABASE_URL','BETTER_AUTH_SECRET','DATABASE_RLS_SECRET','RUST_MIN_STACK'] for arg in ['--env',key]],self.s['name'],'/connected/lib/ld-linux-x86-64.so.2','--library-path','/connected/lib','/connected/registry-test','--ignored','--exact','backup_review::live_registry_archive_obeys_actual_native_backup_observation','--nocapture',env=env,timeout=180)
 def cleanup_review(self):
  if self.s.get('coreID') or self.s['owned']:self.cleanup_connected()
  else:self.s['cleanup']={'errors':[],'ownedResourcesRemaining':[],'fixtureCredentialsDestroyed':True,'earlyProviderFailure':True}
  info=json.loads(self.docker('inspect',self.s['minioID']))[0];assert info['Config']['Labels']['webdock.backup.fixture']==self.s['nonce']
  self.docker('rm','-f',self.s['minioID']);assert not self.docker('ps','-aq','--filter','id='+self.s['minioID']).strip()
  network=json.loads(self.docker('network','inspect',self.network))[0];assert network['Id']==self.s['ownedNetworkID'] and network['Labels']['webdock.backup.fixture']==self.s['nonce']
  self.docker('network','rm',self.network)
  self.s['cleanup']['ownedMinioRemoved']=True;self.s['cleanup']['ownedNetworkRemoved']=True;self.s['cleanup']['existingServicesTouched']=False
  self.s.pop('ownedMinioUser',None);self.s.pop('ownedMinioSecret',None);self.save();print(json.dumps(self.s['cleanup']))

if __name__=='__main__':
 p=argparse.ArgumentParser();p.add_argument('phase',choices=['prepare','review-initial','review-repair','review-rotate-restore','review-export','review-stop','registry-blocked','registry-stopped','cleanup']);p.add_argument('--output',required=True);a=p.parse_args();f=Review(a.output,create=a.phase=='prepare')
 try:
  if a.phase=='prepare':f.prepare_review()
  elif a.phase=='cleanup':f.cleanup_review()
  elif a.phase.startswith('registry-'):f.registry(a.phase.removeprefix('registry-'))
  else:f.run_test(a.phase)
 except Exception:
  f.s['failed']=True;f.save();raise
