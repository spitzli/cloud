#!/usr/bin/env python3
"""Explicit BUILD-only API outage while a separate owned local RUN app serves HTTP.
No production RUN/Core changes. Requires operator authorization for the fixed,
marked BUILD host. The existing runtime fixture owns all local containers/data.
"""
import argparse
import datetime
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time

ROOT=Path(__file__).resolve().parents[2]
SSH=['ssh','-o','IdentityAgent=none','-o','IdentitiesOnly=yes','-o','BatchMode=yes','-o','ConnectTimeout=5','-o','ServerAliveInterval=5','-o','ServerAliveCountMax=2','-i',str(Path.home()/'.ssh/id_ed25519'),'root@167.86.75.144']
REMOTE=r'''
import os,socket,pathlib,subprocess,json,hashlib,struct,time
assert os.geteuid()==0 and socket.gethostname()=='webdock-build-eu-01' and pathlib.Path('/etc/webdock-build-host').is_file()
operation=OPERATION
config='/etc/rancher/k3s/k3s.yaml'
def run(args,timeout=8):
 return subprocess.run(args,capture_output=True,text=True,timeout=timeout)
def kube(*args):
 r=run(['k3s','kubectl','--kubeconfig',config,'--request-timeout=3s',*args]);assert r.returncode==0,'owned build API inventory failed';return json.loads(r.stdout)
def empty_sources():
 summary={}
 for kind in ['jobs','pods','persistentvolumeclaims']:
  data=kube('get',kind,'-A','-o','json');items=[v for v in data['items'] if v['metadata']['namespace'] in ['webdock-builds','webdock-build-qualification']]
  assert not items,'source execution resources must be absent';summary[kind]=0
 return summary
def scrub(value):
 if isinstance(value,dict):return {k:scrub(v) for k,v in value.items() if k not in ['handle','packets','bytes','expires']}
 if isinstance(value,list):return [scrub(v) for v in value]
 return value
def snapshot():
 states={unit:run(['systemctl','is-active',unit]).stdout.strip() for unit in ['k3s','webdock-package-proxy','webdock-package-proxy-firewall.service','webdock-package-proxy-firewall.timer']}
 assert states['k3s']=='active' and states['webdock-package-proxy']=='active'
 source=empty_sources();ns=kube('get','namespace','webdock-build-qualification','-o','json')
 annotations=ns['metadata'].get('annotations',{});assert 'webdock.dev/storage-profile' not in annotations
 quotas=kube('get','resourcequota','-n','webdock-build-qualification','-o','json')
 assert any(v['spec']['hard'].get('persistentvolumeclaims')=='0' for v in quotas['items'])
 firewall={}
 for table in ['webdock_build_host','webdock_package_proxy']:
  result=run(['nft','-j','list','table','inet',table]);assert result.returncode==0
  firewall[table]=hashlib.sha256(json.dumps(scrub(json.loads(result.stdout)),sort_keys=True,separators=(',',':')).encode()).hexdigest()
 mounts=json.loads(run(['findmnt','-J','-o','TARGET,SOURCE,FSTYPE,OPTIONS']).stdout)
 selected=[]
 def visit(row):
  if row['target'].startswith('/var/lib/webdock-build'):selected.append({k:v for k,v in row.items() if k!='children'})
  for child in row.get('children',[]):visit(child)
 for row in mounts['filesystems']:visit(row)
 return {'hostname':socket.gethostname(),'services':states,'sourceResources':source,'namespaceUID':ns['metadata']['uid'],'runnerAnnotation':annotations.get('webdock.dev/runner-image'),'hardStorageAnnotationAbsent':True,'pvcQuotaZero':True,'firewallPolicySHA256':firewall,'buildMounts':sorted(selected,key=lambda row:row['target'])}
if operation=='snapshot':print(json.dumps(snapshot()))
elif operation=='stop':
 prior=snapshot();r=run(['systemctl','stop','k3s'],40);assert r.returncode==0,'BUILD service stop failed'
 state=run(['systemctl','is-active','k3s']).stdout.strip();assert state!='active'
 probes=[]
 for _ in range(3):
  r=run(['k3s','kubectl','--kubeconfig',config,'--request-timeout=2s','-n','webdock-build-qualification','get','jobs','--chunk-size=1','-o','json'],5)
  probes.append({'unavailable':r.returncode!=0,'exitCode':r.returncode});assert r.returncode!=0,'BUILD API still available'
 print(json.dumps({'serviceState':state,'apiProbes':probes,'before':prior}))
elif operation=='start':
 r=run(['systemctl','start','k3s'],40);assert r.returncode==0,'BUILD service start failed';print(json.dumps({'startRequested':True}))
elif operation=='ready':
 nodes=kube('get','nodes','-o','json');assert len(nodes['items'])==1
 assert any(c['type']=='Ready' and c['status']=='True' for c in nodes['items'][0]['status']['conditions'])
 assert run(['systemctl','is-active','webdock-package-proxy']).stdout.strip()=='active'
 # Direct private DNS response, not the host's ambient resolver.
 ident=27131;labels=b''.join(bytes([len(v)])+v.encode() for v in 'registry.npmjs.org'.split('.'))+bytes([0])
 query=struct.pack('!HHHHHH',ident,0x100,1,0,0,0)+labels+struct.pack('!HH',1,1)
 with socket.socket(socket.AF_INET,socket.SOCK_DGRAM) as dns:
  dns.settimeout(3);dns.sendto(query,('10.83.0.1',53));reply,_=dns.recvfrom(4096)
 header=struct.unpack('!HHHHHH',reply[:12]);assert header[0]==ident and header[1]&15==0 and header[3]>0
 listener=run(['ss','-H','-ltn','sport', '=', ':3128'])
 assert listener.returncode==0 and '10.83.0.1:3128' in listener.stdout,'private package proxy listener missing'
 print(json.dumps({'nodeReady':True,'privateDnsResponded':True,'proxyServiceActive':True,'proxyPrivateListener':True,'sourceResources':empty_sources()}))
elif operation=='logs':
 r=run(['journalctl','-u','k3s','-n','30','--no-pager','-o','cat'])
 lines=['[sensitive startup line omitted]' if any(word in line.lower() for word in ['token','password','secret']) else line for line in r.stdout.splitlines()]
 print(json.dumps({'boundedLogs':'\n'.join(lines)[-16384:]}))
else:raise RuntimeError('unknown fixed operator action')
'''

def host(operation):
 code=REMOTE.replace('OPERATION',repr(operation))
 r=subprocess.run([*SSH,'python3 -'],input=code,capture_output=True,text=True,timeout=55)
 if r.returncode:raise RuntimeError('marked_BUILD_'+operation+'_failed')
 return json.loads(r.stdout)

def write(path,value):
 path.write_text(json.dumps(value,indent=2)+'\n');path.chmod(0o600)

def wait_json(path,thread,predicate,seconds=120):
 until=time.monotonic()+seconds
 while time.monotonic()<until:
  if path.exists():
   value=json.loads(path.read_text())
   if predicate(value):return value
  if not thread.is_alive():raise RuntimeError('owned_RUN_fixture_exited_before_handshake')
  time.sleep(0.2)
 raise RuntimeError('owned_outage_handshake_deadline')

def main():
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--oci',required=True,type=Path);parser.add_argument('--qualified-build-api-outage',action='store_true');args=parser.parse_args()
 if not args.qualified_build_api_outage:parser.error('explicit fixed BUILD outage authorization flag required')
 available=int(next(line.split()[1] for line in Path('/proc/meminfo').read_text().splitlines() if line.startswith('MemAvailable:')))*1024
 if available<6*1024**3:raise RuntimeError('at_least6GiB_available_for_owned3GiB_RUN_fixture_required')
 os.environ['DOCKER_HOST']='unix:///var/run/docker.sock';os.environ.pop('DOCKER_CONTEXT',None)
 spec=importlib.util.spec_from_file_location('owned_runtime',Path(__file__).with_name('kubernetes-build-runtime.py'));runtime=importlib.util.module_from_spec(spec);spec.loader.exec_module(runtime)
 oci=runtime.private_file(args.oci.resolve());assert oci.stat().st_size<=512*1024**2
 work=Path(tempfile.mkdtemp(prefix='owned-',dir=ROOT/'.runtime-test/build-api-outage'));work.chmod(0o700)
 os.environ['SPITZLI_BUILD_OUTAGE_TEST_WORK']=str(work)
 before=host('snapshot');write(work/'host-before.json',before)
 write(work/'host-ready-before.json',host('ready'))
 record=ownership=kubeconfig=None;stopped=False;thread=None;result={}
 try:
  record,ownership,kubeconfig=runtime.create_run();write(work/'run-ownership.json',record)
  def accept():
   try:result['code']=runtime.acceptance(oci,ownership,kubeconfig,health_only=True)
   except BaseException as failure:result['failureType']=type(failure).__name__
  thread=threading.Thread(target=accept,daemon=True);thread.start()
  ready=wait_json(work/'run-ready.json',thread,lambda value:True)
  wait_json(work/'http-samples.json',thread,lambda value:len(value['samples'])>=3)
  stopped=True;outage=host('stop');write(work/'host-offline.json',outage)
  offline_ms=int(time.time()*1000)
  during=wait_json(work/'http-samples.json',thread,lambda value:len([s for s in value['samples'] if s['unixMs']>=offline_ms])>=3,seconds=20)
  recovery_ms=int(time.time()*1000);host('start')
  until=time.monotonic()+80;healthy=None
  while time.monotonic()<until:
   try:healthy=host('ready');break
   except RuntimeError:time.sleep(1)
  assert healthy,'BUILD API/node/privateDNS/proxy recovery failed';write(work/'host-ready.json',healthy)
  after=host('snapshot');write(work/'host-after.json',after)
  assert before['firewallPolicySHA256']==after['firewallPolicySHA256'],'owned host firewall policy changed; retained evidence'
  assert before['buildMounts']==after['buildMounts'],'owned build mount state changed; retained evidence'
  assert before['namespaceUID']==after['namespaceUID'] and before['runnerAnnotation']==after['runnerAnnotation']
  final=wait_json(work/'http-samples.json',thread,lambda value:len([s for s in value['samples'] if s['unixMs']>=recovery_ms])>=3,seconds=20)
  (work/'outage-complete').touch(mode=0o600)
  thread.join(timeout=60);assert not thread.is_alive() and result.get('code')==0,'owned RUN acceptance failed'
  stopped=False
  samples=[s for s in final['samples'] if offline_ms<=s['unixMs']<recovery_ms];assert len(samples)>=3
  write(work/'result.json',{'passed':True,'availableMemoryBeforeBytes':available,'beforeImageID':ready['imageID'],'afterImageID':final['imageID'],'digestUnchanged':ready['imageID']==final['imageID'],'actualBuildApiUnavailableProbes':outage['apiProbes'],'duringOutageHttpSamples':samples,'allHttpSamples':len(final['samples']),'buildReadyDnsProxyRecovered':healthy,'hostFirewallUnchanged':True,'buildMountsUnchanged':True,'sourceNamespaceUnchanged':True,'productionRunOrCoreChanged':False})
  print('PASS BUILD API offline; owned RUN HTTP/digest unchanged; API/node/privateDNS/proxy restored. Evidence:',work,flush=True)
 finally:
  recovery_error=None
  if stopped:
   try:
    host('start')
    until=time.monotonic()+90
    while True:
     try:host('ready');break
     except RuntimeError:
      if time.monotonic()>=until:raise RuntimeError('BUILD_recovery_unproven_operator_attention_required')
      time.sleep(1)
   except BaseException as failure:recovery_error=type(failure).__name__
  try:write(work/'bounded-build-logs.json',host('logs'))
  except RuntimeError:write(work/'bounded-build-logs.json',{'unavailable':True})
  (work/'outage-complete').touch(mode=0o600)
  if thread and thread.is_alive():thread.join(timeout=50)
  if thread and thread.is_alive():raise RuntimeError('owned_RUN_fixture_still_active_retained_for_exact_cleanup')
  if record:
   runtime.cleanup_created_run(record)
   runtime.private_file(kubeconfig).unlink(missing_ok=True)
   write(work/'cleanup.json',{'ownedRunContainerRemoved':True,'ownedRunVolumeRemoved':True,'privateKubeconfigRemoved':True,'buildServiceRestored':recovery_error is None})
  if recovery_error:raise RuntimeError('BUILD_recovery_unproven_operator_attention_required')
if __name__=='__main__':
 (ROOT/'.runtime-test/build-api-outage').mkdir(parents=True,exist_ok=True,mode=0o700)
 main()
