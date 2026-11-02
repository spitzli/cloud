#!/usr/bin/env python3
"""Actual pinned Caddy + controlled HTTP peer, loopback only; never deploys."""
import argparse
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler,ThreadingHTTPServer
import importlib.util
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import secrets
import threading
import time
import urllib.parse

ROOT=Path(__file__).resolve().parent
BINARY_SHA256='678ade3bfc088749c81a681adc603333ee0bb023b6a6cfe3c0f58bef8ff854e9'
ARCHIVE_SHA256='727b91701a392de6ebc5027509f548bf39979e5216340d0faed8fa5e69c84f8b'
QUALIFICATION_BASE='sha256:74d077c5981eeaaf88688deca6dfece7e21d3e1627264d1c2183750380683833'
spec=importlib.util.spec_from_file_location('gateway_render',ROOT/'render.py');render=importlib.util.module_from_spec(spec);spec.loader.exec_module(render)
HOSTS={'console':'console.example.test','studio':'studio.example.test','account':'auth.example.test','workspace':'api.example.test'}
TECHNICAL='edge.example.test'


def check(value,code):
 if not value:raise RuntimeError(code)


class Peer(BaseHTTPRequestHandler):
 protocol_version='HTTP/1.1'
 records=[]
 def log_message(self,*args):pass
 def reply(self,status,body=b'',headers=None):
  self.send_response(status)
  for key,value in headers or []:self.send_header(key,value)
  self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body);self.wfile.flush()
 def body(self):
  if self.headers.get('Transfer-Encoding','').lower()=='chunked':
   chunks=[]
   while True:
    size=int(self.rfile.readline().split(b';',1)[0].strip(),16)
    if not size:
     while self.rfile.readline() not in [b'\r\n',b'\n',b'']:pass
     break
    chunks.append(self.rfile.read(size));check(self.rfile.read(2)==b'\r\n','chunk_framing')
   return b''.join(chunks)
  return self.rfile.read(int(self.headers.get('Content-Length','0')))
 def handle_request(self):
  host=self.headers.get('Host');body=self.body()
  record={'method':self.command,'target':self.path,'host':host,'headers':{key.lower():value for key,value in self.headers.items()},'bodySHA256':hashlib.sha256(body).hexdigest(),'bytes':len(body)}
  self.records.append(record)
  if host not in HOSTS.values():self.reply(404);return
  forbidden=[key for key in record['headers'] if key=='forwarded' or key.startswith(('x-forwarded-','x-vercel-')) or key in ['x-real-ip','via','connection','proxy-connection','keep-alive','proxy-authenticate','proxy-authorization','upgrade','x-poison']]
  if forbidden:self.reply(400,b'forwarded-or-hop-rejected');return
  origin=self.headers.get('Origin')
  if self.command not in ['GET','HEAD','OPTIONS'] and origin is not None and origin!='https://'+host:self.reply(403,b'origin-rejected');return
  path=urllib.parse.urlsplit(self.path).path
  if path=='/console-config.json':
   config={'apiOrigin':'https://'+host,'workspaceApiOrigin':'https://'+HOSTS['workspace'],'authOrigin':'https://'+HOSTS['account']}
   if host in [HOSTS['console'],HOSTS['studio']]:config['browserFacade']=True
   if host==HOSTS['account']:config['surface']='account'
   self.reply(200,json.dumps(config).encode(),[('Content-Type','application/json')]);return
  if path=='/api/sso/callback':
   self.reply(307,b'callback-body',[('Location','https://auth.example.test/api/auth/oauth2/authorize?state=A%2fb+%252e&x=1&x=2'),('Set-Cookie','one=first; Path=/; Secure; HttpOnly; SameSite=Lax'),('Set-Cookie','two=second; Path=/; Secure; HttpOnly; SameSite=Strict')]);return
  if path=='/api/console/events':
   self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Cache-Control','no-store');self.send_header('Connection','close');self.end_headers()
   self.wfile.write(b'id: 1\nevent: logs\ndata: first\n\n');self.wfile.flush();time.sleep(0.75)
   self.wfile.write(b'id: 2\nevent: logs\ndata: second\n\n');self.wfile.flush();self.close_connection=True;return
  self.reply(200,json.dumps(record).encode(),[('Content-Type','application/json')])
 do_GET=handle_request
 do_POST=handle_request
 do_PUT=handle_request
 do_OPTIONS=handle_request


def free_port():
 with socket.socket() as listener:listener.bind(('127.0.0.1',0));return listener.getsockname()[1]


def config(core_port,port):
 c=json.loads((ROOT/'profile.example.json').read_text());c.update(enabled=True,namespace='owned-qualifier',caddyImage='docker.io/library/caddy@sha256:'+'a'*64,gatewayOrigin='https://'+TECHNICAL,coreUpstream='http://127.0.0.1:'+str(core_port),listenPort=port,tlsSecretName='owned-tls',ingressClassName='traefik',ingressControllerNamespace='kube-system',ingressControllerPodLabels={'app.kubernetes.io/name':'traefik'},coreNamespace='owned-qualifier',corePodLabels={'app.kubernetes.io/name':'owned-peer'})
 for surface,key in render.SURFACES.items():c[key]='https://'+HOSTS[surface]
 return c


def request(port,method,target,body=None,headers=None,chunked=False):
 connection=http.client.HTTPConnection('127.0.0.1',port,timeout=5)
 connection.request(method,target,body,{'Host':TECHNICAL,**(headers or {})},encode_chunked=chunked)
 response=connection.getresponse();result=(response.status,response.getheaders(),response.read());connection.close();return result


def qualify(binary,readonly_container=False):
 check(os.geteuid()!=0,'qualification_must_run_as_nonroot')
 check(hashlib.sha256(binary.read_bytes()).hexdigest()==BINARY_SHA256,'official_binary_pin_mismatch')
 version=subprocess.check_output([str(binary),'version'],text=True).strip();check(version.startswith('v2.11.7 '),'version_mismatch')
 peer=ThreadingHTTPServer(('127.0.0.1',0),Peer);peer.daemon_threads=True;thread=threading.Thread(target=peer.serve_forever,daemon=True);thread.start()
 results=[]
 with tempfile.TemporaryDirectory(prefix='webdock-caddy-gateway-') as temporary:
  root=Path(temporary);port=free_port();c=config(peer.server_port,port);text=render.render(c)['Caddyfile']
  # Same generated handlers, bound only to loopback for this local qualifier.
  text=text.replace(':'+str(port)+' {', ':'+str(port)+' {\n  bind 127.0.0.1',1)
  text=text.replace('root /tmp/caddy-data','root '+str(root/'caddy-data'))
  profile=root/'Caddyfile';profile.write_text(text);profile.chmod(0o444 if readonly_container else 0o600)
  subprocess.run([str(binary),'validate','--config',str(profile),'--adapter','caddyfile'],capture_output=True,text=True,check=True)
  container_id=None;owner=secrets.token_hex(8);cidfile=root/'container.id'
  command=[str(binary),'run','--config',str(profile),'--adapter','caddyfile']
  if readonly_container:
   subprocess.run(['docker','image','inspect',QUALIFICATION_BASE],capture_output=True,check=True)
   command=['docker','run','--rm','--pull=never','--cidfile',str(cidfile),'--label','webdock.dev/gateway-qualification='+owner,'--network=host','--read-only','--user','10001:10001','--cap-drop=ALL','--security-opt=no-new-privileges','--memory=128m','--memory-swap=128m','--cpus=0.5','--pids-limit=64','--tmpfs','/tmp:rw,noexec,nosuid,nodev,size=8m,mode=1777','--mount','type=bind,src='+str(binary)+',dst=/usr/local/bin/caddy,readonly','--mount','type=bind,src='+str(profile)+',dst=/etc/caddy/Caddyfile,readonly','--entrypoint','/usr/local/bin/caddy',QUALIFICATION_BASE,'run','--config','/etc/caddy/Caddyfile','--adapter','caddyfile']
  log=(root/'caddy.log').open('wb');process=subprocess.Popen(command,stdout=log,stderr=log,env={**os.environ,'HOME':str(root),'XDG_CONFIG_HOME':str(root/'config'),'XDG_DATA_HOME':str(root/'data')})
  try:
   deadline=time.monotonic()+5
   while time.monotonic()<deadline:
    check(process.poll() is None,'caddy_failed_to_start')
    try:
     if request(port,'GET','/healthz')[::2]==(200,b'ok'):break
    except OSError:pass
    time.sleep(0.02)
   else:raise RuntimeError('caddy_not_listening')
   runtime={'kind':'nonroot pinned binary','uid':os.geteuid(),'readOnlyRootFilesystem':False}
   if readonly_container:
    container_id=cidfile.read_text().strip();check(len(container_id)==64,'container_identity_missing')
    observed=json.loads(subprocess.check_output(['docker','inspect',container_id]))[0]
    check(observed['Config']['Labels'].get('webdock.dev/gateway-qualification')==owner and observed['Config']['User']=='10001:10001' and observed['HostConfig']['ReadonlyRootfs'] and observed['HostConfig']['CapDrop']==['ALL'],'container_security_profile_changed')
    check(not observed['HostConfig']['Privileged'] and any(value in ['no-new-privileges','no-new-privileges=true'] for value in observed['HostConfig']['SecurityOpt']) and observed['HostConfig']['Memory']==128*1024*1024 and observed['HostConfig']['MemorySwap']==128*1024*1024 and observed['HostConfig']['PidsLimit']==64,'container_runtime_bounds_changed')
    runtime={'kind':'pinned official Caddy binary mounted into cached local qualification-only Alpine base','uid':10001,'readOnlyRootFilesystem':True,'capabilitiesDropped':['ALL'],'noNewPrivileges':True,'memoryBytes':observed['HostConfig']['Memory'],'pidsLimit':observed['HostConfig']['PidsLimit'],'temporaryFilesystemBytes':8*1024*1024,'baseImageID':QUALIFICATION_BASE,'network':'host network for local loopback-only test, not production Pod networking'}
   # Direct strict peer rejects the same forged proxy input; Core is not relaxed.
   direct=http.client.HTTPConnection('127.0.0.1',peer.server_port,timeout=5);direct.request('GET','/console-config.json',headers={'Host':HOSTS['console'],'Forwarded':'for=evil'});response=direct.getresponse();check(response.status==400,'strict_peer_was_relaxed');response.read();direct.close();results.append('strict-peer-forwarded-rejection')
   forged={'Forwarded':'for=evil;host=evil;proto=http','X-Forwarded-Host':'evil','X-Forwarded-Proto':'http','X-Forwarded-For':'127.0.0.1','X-Vercel-Forwarded-For':'evil','X-Vercel-Other':'must-drop','X-Real-IP':'evil','Via':'evil','Proxy-Authorization':'Basic OWNED_TEST_ONLY','Connection':'X-Poison','X-Poison':'must-drop'}
   for surface,host in HOSTS.items():
    status,_,body=request(port,'GET','/__'+surface+'/console-config.json',headers=forged);check(status==200,'forged_proxy_headers_reached_core_'+surface)
    check(body.startswith(b'{'),'non_json_bootstrap_'+surface+'_'+repr(body[:100])+'_peer_records_'+str(len(Peer.records)))
    value=json.loads(body);check(value['apiOrigin']=='https://'+host,'canonical_host_not_fixed_'+surface)
    if surface in ['console','studio']:check(value.get('browserFacade') is True,'facade_bootstrap_host_wrong')
    results.append('canonical-'+surface+'-bootstrap-and-forwarded-sanitization')
   raw_query='state=A%2fb+%252e&state=again&empty=&bare&x=%3F%23'
   payload=bytes(range(256))*1024
   headers={**forged,'Origin':'https://'+HOSTS['console'],'Cookie':'session=OWNED_TEST_ONLY; preference=a%2Fb','Authorization':'Bearer OWNED_TEST_ONLY','Last-Event-ID':'owned-cursor','Idempotency-Key':'owned-id','Content-Type':'application/octet-stream'}
   status,_,body=request(port,'POST','/__console/api/console/echo?'+raw_query,payload,headers);check(status==200,'body_proxy_failed')
   echo=json.loads(body);check(echo['target']=='/api/console/echo?'+raw_query and echo['bodySHA256']==hashlib.sha256(payload).hexdigest() and echo['bytes']==len(payload),'query_or_body_changed')
   for header in ['origin','cookie','authorization','last-event-id','idempotency-key','content-type']:check(echo['headers'][header]==headers[next(key for key in headers if key.lower()==header)],'end_to_end_header_changed_'+header)
   results.append('raw-query-binary-body-cookie-authorization-origin-cursor-preserved')
   status,_,body=request(port,'POST','/__console/api/console/echo',[b'chunk-one',b'\x00chunk-two'],{'Origin':'https://'+HOSTS['console']},chunked=True);check(status==200 and json.loads(body)['bodySHA256']==hashlib.sha256(b'chunk-one\x00chunk-two').hexdigest(),'chunked_body_changed');results.append('chunked-body-preserved-with-valid-new-hop-framing')
   status,_,_=request(port,'POST','/__console/api/console/echo',b'x',{'Origin':'https://evil.example.test'});check(status==403,'origin_was_rewritten_or_authorization_relaxed');results.append('foreign-origin-remains-denied')
   status,headers,body=request(port,'GET','/__console/api/sso/callback?'+raw_query);check(status==307 and body==b'callback-body','callback_response_changed')
   check([value for key,value in headers if key.lower()=='location']==['https://auth.example.test/api/auth/oauth2/authorize?state=A%2fb+%252e&x=1&x=2'],'location_changed')
   check([value for key,value in headers if key.lower()=='set-cookie']==['one=first; Path=/; Secure; HttpOnly; SameSite=Lax','two=second; Path=/; Secure; HttpOnly; SameSite=Strict'],'multiple_cookies_changed');results.append('callback-location-status-and-two-set-cookies-preserved')
   for path in ['/api/console/echo','/__console/not-core','/__consolex/api/console/echo','/__account/api/console/echo','/__workspace/api/hosting/git/worker/claim','/__studio/api/hosting/git/worker/kubernetes-claim','/config/']:
    before=len(Peer.records);status,_,_=request(port,'GET',path);check(status==404 and len(Peer.records)==before,'unknown_or_private_route_forwarded_'+path)
   results.append('unknown-prefix-private-worker-and-admin-paths-never-forwarded')
   for path in ['/__console//api/console/echo','/__console/api/console/../auth/x','/__console/api/console/%2e%2e/x','/__console/api/console/%252e%252e/x','/__console/api/console/%2fsecret','/__console/api/console/\\secret']:
    before=len(Peer.records);status,_,_=request(port,'GET',path);check(status==400 and len(Peer.records)==before,'unsafe_uri_was_normalized_'+path)
   results.append('raw-traversal-encoded-separators-and-double-encoding-denied-before-normalization')
   status,_,_=request(port,'GET','/__console/console-config.json',headers={'Host':'evil.example.test'});check(status==404,'unexpected_incoming_host_accepted');results.append('technical-gateway-host-is-fixed')
   connection=http.client.HTTPConnection('127.0.0.1',port,timeout=5);start=time.monotonic();connection.request('GET','/__console/api/console/events',headers={'Host':TECHNICAL,'Last-Event-ID':'owned-cursor'});response=connection.getresponse();check(response.status==200,'sse_status')
   first=response.readline();first_at=time.monotonic()-start;rest=response.read();elapsed=time.monotonic()-start;connection.close()
   check(first==b'id: 1\n' and first_at<0.5 and b'id: 2\n' in rest and elapsed>=0.7,'sse_buffered_or_rewritten');results.append('sse-first-event-flushed-before-second-event')
   return {'version':version,'binarySHA256':BINARY_SHA256,'officialArchiveSHA256':ARCHIVE_SHA256,'nonrootUID':os.geteuid(),'bindScope':'loopback only','runtime':runtime,'CaddyProfileSHA256':hashlib.sha256(text.encode()).hexdigest(),'checks':results,'sseFirstEventSeconds':round(first_at,4),'sseSecondEventSeconds':round(elapsed,4),'cleanup':'exact owned process terminated, controlled peer closed, temporary config/log/state removed','limits':['actual Core/Ingress/Vercel was not deployed/contacted','official Caddy container image digest and RUN admission/networking remain unqualified; read-only constraints were tested only when the explicit local container option is selected','Vercel external proxy/body/SSE/cache behavior remains a separate owned preview gate']}
  except Exception:
   log.flush();Path('/tmp/webdock-caddy-qualification-error.log').write_bytes((root/'caddy.log').read_bytes());Path('/tmp/webdock-caddy-qualification-error.log').chmod(0o600);raise
  finally:
   if readonly_container and cidfile.exists():
    exact=cidfile.read_text().strip();observed=subprocess.run(['docker','inspect',exact],capture_output=True,text=True)
    if observed.returncode==0:
     check(json.loads(observed.stdout)[0]['Config']['Labels'].get('webdock.dev/gateway-qualification')==owner,'cleanup_container_owner_changed')
     subprocess.run(['docker','rm','--force',exact],capture_output=True,check=True)
   if process.poll() is None:process.terminate()
   try:process.wait(timeout=5)
   except subprocess.TimeoutExpired:process.kill();process.wait(timeout=5)
   log.close();peer.shutdown();peer.server_close();thread.join(timeout=2)


def main():
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--caddy',type=Path,required=True);parser.add_argument('--evidence',type=Path);parser.add_argument('--readonly-container',action='store_true');args=parser.parse_args()
 result=qualify(args.caddy.resolve(),args.readonly_container)
 if args.evidence:
  with args.evidence.open('x') as stream:json.dump(result,stream,indent=2);stream.write('\n')
 print(json.dumps(result,indent=2))


if __name__=='__main__':main()
