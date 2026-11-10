#!/usr/bin/env python3
"""Render operator-owned deployment resources from a private, explicit inventory.

The output includes Kubernetes Secrets: keep input/output outside Git, mode 0600.
SQLite claims must already reference the approved project volume; never copy a live DB.
"""
import argparse, json, os, pathlib, subprocess, re, ssl, ipaddress
from urllib.parse import urlsplit
p=argparse.ArgumentParser(); p.add_argument('--inventory',required=True); p.add_argument('--private-dir',required=True); p.add_argument('--gateway-image',required=True); p.add_argument('--runtime-image',required=True); p.add_argument('--tls-image',required=True); p.add_argument('--runtime-commit',required=True); p.add_argument('--managed-runtimes-config'); p.add_argument('--output',required=True); a=p.parse_args()
os.umask(0o077); private=pathlib.Path(a.private_dir); inventory=json.loads(pathlib.Path(a.inventory).read_text()); namespace='spitzli-databases'; resources=[]
managed=None
if a.managed_runtimes_config:
 try:
  managed=json.loads(pathlib.Path(a.managed_runtimes_config).read_text())
  assert set(managed)=={'authOrigin','capacity','clusters'}
  assert type(managed['capacity']) is int and 1<=managed['capacity']<=64
  api=urlsplit(managed['authOrigin'])
  assert api.scheme=='https' and api.hostname and not api.username and not api.password and not api.query and not api.fragment and api.path==''
  assert isinstance(managed['clusters'],list) and 1<=len(managed['clusters'])<=16
  for cluster in managed['clusters']:
   assert set(cluster)=={'addressIPv4','port','endPort'}
   address=ipaddress.IPv4Address(cluster['addressIPv4'])
   assert address.is_global and not address.is_multicast
   assert type(cluster['port']) is int and type(cluster['endPort']) is int and 32000<=cluster['port']<=cluster['endPort']<=32199
 except Exception:
  raise ValueError('Invalid managed database runtime configuration') from None
database_ingress=[]
def database_target(row):
 if 'databaseTLSCA' not in row and 'databaseTarget' not in row:return None
 # Never include inventory values in validation failures: they may contain secrets.
 def require(valid):
  if not valid:raise ValueError('Invalid private database target configuration')
 target=row.get('databaseTarget');ca_path=row.get('databaseTLSCA')
 require(row.get('engine')=='postgresql' and isinstance(target,dict) and isinstance(ca_path,str) and bool(ca_path))
 dns=lambda value:isinstance(value,str) and len(value)<=63 and re.fullmatch(r'[a-z0-9](?:[a-z0-9-]*[a-z0-9])?',value)
 external=set(target)=={'host','port','addressIPv4'}
 host=target.get('host')
 if external:
  require(isinstance(host,str) and len(host)<=253 and '.' in host and all(dns(part) for part in host.split('.')))
  require(type(target['port']) is int and 1<=target['port']<=65535)
  require(isinstance(target['addressIPv4'],str))
  try:address=ipaddress.IPv4Address(target['addressIPv4'])
  except Exception:raise ValueError('Invalid private database target address') from None
  require(address.is_global and not address.is_multicast)
 else:
  require(set(target)=={'namespace','podLabels','host'})
  require(dns(target['namespace']))
  labels=target['podLabels'];require(isinstance(labels,dict) and bool(labels))
  for key,value in labels.items():
   require(isinstance(key,str) and isinstance(value,str))
   parts=key.split('/');require(len(parts)<=2)
   if len(parts)==2:require(len(parts[0])<=253 and all(dns(part) for part in parts[0].split('.')))
   label=lambda part:len(part)<=63 and re.fullmatch(r'[A-Za-z0-9](?:[A-Za-z0-9_.-]*[A-Za-z0-9])?',part)
   require(label(parts[-1]) and label(value))
  suffix='.'+target['namespace']+'.svc.cluster.local'
  require(isinstance(host,str) and host.endswith(suffix) and dns(host[:-len(suffix)]))
 connections=row.get('connections');require(isinstance(connections,list) and len(connections)==1)
 connection=connections[0];require(isinstance(connection,dict))
 params=connection.get('params');require(isinstance(params,dict))
 require(params.get('driver') in ('postgres','postgresql') and params.get('host')==host and params.get('port')==target.get('port',5432)
         and params.get('ssl_mode')=='verify-full' and params.get('ssl_ca')=='/database-trust/ca.crt')
 try:
  ca=pathlib.Path(ca_path).read_text()
  require(re.fullmatch(r'\s*(?:-----BEGIN CERTIFICATE-----\s+[A-Za-z0-9+/=\s]+-----END CERTIFICATE-----\s*)+',ca))
  ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT).load_verify_locations(cadata=ca)
 except Exception:raise ValueError('Invalid database CA certificate file') from None
 return target,ca
# Validate every optional target before rendering any resources or certificates.
targets=[database_target(row) for row in inventory]
def add(kind,name,spec=None,**extra):
 item={'apiVersion':'apps/v1' if kind=='Deployment' else 'networking.k8s.io/v1' if kind in ['NetworkPolicy','Ingress'] else 'v1','kind':kind,'metadata':{'name':name,'namespace':namespace,'labels':{'app.kubernetes.io/part-of':'webdock-database-browser'}}}
 if kind=='Namespace':item['metadata'].pop('namespace')
 if spec is not None:item['spec']=spec
 item.update(extra);resources.append(item);return item
security={'runAsNonRoot':True,'runAsUser':65532,'runAsGroup':65532,'fsGroup':65532,'fsGroupChangePolicy':'OnRootMismatch','seccompProfile':{'type':'RuntimeDefault'}}
container_security={'allowPrivilegeEscalation':False,'readOnlyRootFilesystem':True,'capabilities':{'drop':['ALL']}}
def limits(cpu,memory):return {'requests':{'cpu':'50m','memory':'64Mi','ephemeral-storage':'32Mi'},'limits':{'cpu':cpu,'memory':memory,'ephemeral-storage':'128Mi'}}
ns=add('Namespace',namespace);ns['metadata']['labels'].update({'pod-security.kubernetes.io/enforce':'restricted','pod-security.kubernetes.io/audit':'restricted'})
add('ResourceQuota','database-browser-budget',{'hard':{'requests.cpu':'1','limits.cpu':'4','requests.memory':'1Gi','limits.memory':'3Gi','requests.ephemeral-storage':'512Mi','limits.ephemeral-storage':'2Gi','requests.storage':'2Gi','persistentvolumeclaims':'8','pods':'8','services':'8','count/secrets':'10','count/configmaps':'10','services.nodeports':'0','services.loadbalancers':'0'}})
add('ConfigMap','database-runtime-ca',data={'ca.crt':(private/'ca.crt').read_text()})
add('ConfigMap','database-runtime-tls',data={'Caddyfile':'{\n admin off\n auto_https off\n}\nhttps://:8443 {\n tls /tls/tls.crt /tls/tls.key\n reverse_proxy 127.0.0.1:8080\n}\n'})
add('NetworkPolicy','default-deny',{'podSelector':{},'policyTypes':['Ingress','Egress']})
add('NetworkPolicy','dns',{'podSelector':{},'policyTypes':['Egress'],'egress':[{'to':[{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':'kube-system'}},'podSelector':{'matchLabels':{'k8s-app':'kube-dns'}}}], 'ports':[{'protocol':p,'port':53} for p in ['UDP','TCP']]}]})
public={'ipBlock':{'cidr':'0.0.0.0/0','except':['10.0.0.0/8','172.16.0.0/12','192.168.0.0/16','127.0.0.0/8','169.254.0.0/16']}}
add('NetworkPolicy','gateway-network',{'podSelector':{'matchLabels':{'app':'database-gateway'}},'policyTypes':['Ingress','Egress'],'ingress':[{'from':[{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':'kube-system'}},'podSelector':{'matchLabels':{'app.kubernetes.io/name':'traefik'}}}], 'ports':[{'port':3137}]}], 'egress':[{'to':[public],'ports':[{'port':443}]},{'to':[{'podSelector':{'matchLabels':{'component':'database-runtime'}}}],'ports':[{'port':8443}]},{'to':[{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':'kube-system'}},'podSelector':{'matchLabels':{'app.kubernetes.io/name':'traefik'}}}],'ports':[{'protocol':'TCP','port':8443}]}]})
for row,target_config in zip(inventory,targets):
 assert row['namespace']==namespace and re.fullmatch(r'[a-z0-9-]{1,50}',row['slug'])
 name='db-'+row['slug'];host=f'{name}.{namespace}.svc.cluster.local';key=private/(name+'.key');crt=private/(name+'.crt')
 if not crt.exists():
  subprocess.run(['openssl','req','-new','-newkey','rsa:2048','-nodes','-keyout',str(key),'-out',str(private/(name+'.csr')),'-subj','/CN='+host],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
  ext=private/(name+'.ext');ext.write_text('subjectAltName=DNS:'+host+'\nextendedKeyUsage=serverAuth\n')
  subprocess.run(['openssl','x509','-req','-in',str(private/(name+'.csr')),'-CA',str(private/'ca.crt'),'-CAkey',str(private/'ca.key'),'-CAcreateserial','-out',str(crt),'-days','365','-sha256','-extfile',str(ext)],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 add('Secret',name,type='Opaque',stringData={'connections.json':json.dumps(row['connections']),'config.json':json.dumps(row['config']),'proxy-secret':row['proxySecret'],'tls.crt':crt.read_text(),'tls.key':key.read_text()})
 add('PersistentVolumeClaim',name+'-state',{'accessModes':['ReadWriteOnce'],'resources':{'requests':{'storage':'128Mi'}},'storageClassName':'local-path'})
 volumes=[{'name':'state','persistentVolumeClaim':{'claimName':name+'-state'}},{'name':'credentials','secret':{'secretName':name,'defaultMode':288}},{'name':'tls-config','configMap':{'name':'database-runtime-tls'}},{'name':'tmp','emptyDir':{'sizeLimit':'128Mi'}}]
 mounts=[{'name':'state','mountPath':'/data'},{'name':'tmp','mountPath':'/tmp'}]
 if target_config:
  target,ca=target_config
  add('ConfigMap',name+'-database-ca',data={'ca.crt':ca})
  volumes.append({'name':'database-trust','configMap':{'name':name+'-database-ca'}})
  mounts.append({'name':'database-trust','mountPath':'/database-trust','readOnly':True})
 if row.get('pvc'):volumes.append({'name':'project-data','persistentVolumeClaim':{'claimName':row['pvc']}});mounts.append({'name':'project-data','mountPath':'/project-data'})
 pod={'automountServiceAccountToken':False,'securityContext':security,'volumes':volumes,'initContainers':[{'name':'initialize','image':'busybox:1.37','command':['sh','-c','cp /credentials/connections.json /data/connections.json; if [ ! -f /data/config.json ]; then cp /credentials/config.json /data/config.json; fi; chmod 600 /data/connections.json /data/config.json'],'securityContext':container_security,'resources':limits('100m','64Mi'),'volumeMounts':[{'name':'state','mountPath':'/data'},{'name':'credentials','mountPath':'/credentials','readOnly':True}]}], 'containers':[
 {'name':'tabularis','image':a.runtime_image,'imagePullPolicy':'Never','args':['--public-url',row['runtimeOrigin'],'--allowed-origin',row['runtimeOrigin']], 'env':[{'name':'TABULARIS_WEB_PROXY_SECRET','valueFrom':{'secretKeyRef':{'name':name,'key':'proxy-secret'}}}], 'securityContext':container_security,'resources':limits('750m','512Mi'),'volumeMounts':mounts},
 {'name':'tls','image':a.tls_image,'imagePullPolicy':'Never','command':['caddy','run','--config','/etc/caddy/Caddyfile','--adapter','caddyfile'],'securityContext':container_security,'resources':limits('100m','128Mi'),'ports':[{'containerPort':8443}],'readinessProbe':{'tcpSocket':{'port':8443},'initialDelaySeconds':3},'volumeMounts':[{'name':'credentials','mountPath':'/tls','readOnly':True},{'name':'tls-config','mountPath':'/etc/caddy','readOnly':True},{'name':'tmp','mountPath':'/tmp'}]}
 ]}
 probe={'exec':{'command':['bash','-c','exec 3<>/dev/tcp/127.0.0.1/8080']},'timeoutSeconds':3}
 pod['containers'][0].update({'readinessProbe':probe,'livenessProbe':{**probe,'periodSeconds':15},'startupProbe':{**probe,'failureThreshold':30,'periodSeconds':5}})
 pod['containers'][1].update({'livenessProbe':{'tcpSocket':{'port':8443}},'startupProbe':{'tcpSocket':{'port':8443},'failureThreshold':30,'periodSeconds':5}})
 add('Deployment',name,{'replicas':1,'strategy':{'type':'Recreate'},'selector':{'matchLabels':{'app':name}},'template':{'metadata':{'labels':{'app':name,'component':'database-runtime'}},'spec':pod}})
 add('Service',name,{'selector':{'app':name},'ports':[{'port':8443,'targetPort':8443}]})
 policy={'podSelector':{'matchLabels':{'app':name}},'policyTypes':['Ingress','Egress'],'ingress':[{'from':[{'podSelector':{'matchLabels':{'app':'database-gateway'}}}],'ports':[{'port':8443}]}],'egress':[]}
 if row['engine']=='postgresql':policy['egress']=[{'to':[public],'ports':[{'port':5432}]}]
 if target_config and 'addressIPv4' in target:
  policy['egress']=[{'to':[{'ipBlock':{'cidr':target['addressIPv4']+'/32'}}],'ports':[{'protocol':'TCP','port':target['port']}]}]
 if target_config and 'namespace' in target:
  policy['egress']=[{'to':[{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':target['namespace']}},'podSelector':{'matchLabels':target['podLabels']}}],'ports':[{'protocol':'TCP','port':5432}]}]
  database_ingress.append({'apiVersion':'networking.k8s.io/v1','kind':'NetworkPolicy','metadata':{'name':name+'-browser','namespace':target['namespace'],'labels':{'app.kubernetes.io/part-of':'webdock-database-browser'}},'spec':{'podSelector':{'matchLabels':target['podLabels']},'policyTypes':['Ingress'],'ingress':[{'from':[{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':namespace}},'podSelector':{'matchLabels':{'app':name}}}],'ports':[{'protocol':'TCP','port':5432}]}]}})
 add('NetworkPolicy',name,policy)
add('Secret','database-gateway',type='Opaque',stringData={'SPITZLI_DATABASE_GATEWAY_SECRET':(private/'gateway-secret').read_text()})
add('Deployment','database-gateway',{'replicas':1,'strategy':{'type':'Recreate'},'selector':{'matchLabels':{'app':'database-gateway'}},'template':{'metadata':{'labels':{'app':'database-gateway'}},'spec':{'automountServiceAccountToken':False,'securityContext':security,'containers':[{'name':'gateway','image':a.gateway_image,'imagePullPolicy':'Never','securityContext':container_security,'resources':limits('500m','256Mi'),'envFrom':[{'secretRef':{'name':'database-gateway'}}],'env':[{'name':k,'value':v} for k,v in {'SPITZLI_AUTH_ORIGIN':'https://auth.webdock.dev','SPITZLI_DATABASE_GATEWAY_ORIGIN':'https://database.webdock.dev','SPITZLI_DATABASE_BROWSER_ORIGINS':'https://studio.webdock.dev,https://console.webdock.dev','SPITZLI_DATABASE_RUNTIME_ORIGINS':','.join(r['runtimeOrigin'] for r in inventory),'TABULARIS_EXPECTED_COMMIT':a.runtime_commit,'NODE_EXTRA_CA_CERTS':'/trust/ca.crt'}.items()],'ports':[{'containerPort':3137}],'readinessProbe':{'httpGet':{'path':'/healthz','port':3137}},'livenessProbe':{'httpGet':{'path':'/healthz','port':3137}},'startupProbe':{'httpGet':{'path':'/healthz','port':3137},'failureThreshold':30,'periodSeconds':5},'volumeMounts':[{'name':'trust','mountPath':'/trust','readOnly':True}]}],'volumes':[{'name':'trust','configMap':{'name':'database-runtime-ca'}}]}}})
if managed:
 gateway=next(item for item in resources if item['kind']=='Deployment' and item['metadata']['name']=='database-gateway')
 pod=gateway['spec']['template']['spec'];container=pod['containers'][0]
 for variable in container['env']:
  if variable['name']=='SPITZLI_AUTH_ORIGIN':variable['value']=managed['authOrigin']
 container['env'] += [{'name':'SPITZLI_DATABASE_MANAGED_RUNTIMES','value':'true'},{'name':'SPITZLI_DATABASE_RUNTIME_CAPACITY','value':str(managed['capacity'])}]
 container['volumeMounts'].append({'name':'managed-runtime-state','mountPath':'/tmp'})
 pod['volumes'].append({'name':'managed-runtime-state','emptyDir':{'medium':'Memory','sizeLimit':str(managed['capacity']*32)+'Mi'}})
 container['resources']['requests'].update({'cpu':'100m','memory':'128Mi'})
 container['resources']['limits'].update({'cpu':'2','memory':str(256+managed['capacity']*256)+'Mi'})
 quota=next(item for item in resources if item['kind']=='ResourceQuota')
 quota['spec']['hard']['limits.memory']=str(3072+managed['capacity']*256)+'Mi'
 quota['spec']['hard']['limits.cpu']='6'
 network=next(item for item in resources if item['kind']=='NetworkPolicy' and item['metadata']['name']=='gateway-network')
 for cluster in managed['clusters']:
  network['spec']['egress'].append({'to':[{'ipBlock':{'cidr':cluster['addressIPv4']+'/32'}}], 'ports':[{'protocol':'TCP','port':cluster['port'],'endPort':cluster['endPort']}]})
add('Service','database-gateway',{'selector':{'app':'database-gateway'},'ports':[{'port':3137,'targetPort':3137}]})
ing=add('Ingress','database-gateway',{'ingressClassName':'traefik','rules':[{'host':'database.webdock.dev','http':{'paths':[{'path':'/','pathType':'Prefix','backend':{'service':{'name':'database-gateway','port':{'number':3137}}}}]}}],'tls':[{'hosts':['database.webdock.dev']}]});ing['metadata']['annotations']={'traefik.ingress.kubernetes.io/router.entrypoints':'websecure','traefik.ingress.kubernetes.io/router.tls':'true','traefik.ingress.kubernetes.io/router.tls.certresolver':'webdock'}
pathlib.Path(a.output).write_text(json.dumps({'apiVersion':'v1','kind':'List','items':resources}));print(f'Rendered {len(resources)} resources; secret-bearing output retained locally.')
if database_ingress:
 pathlib.Path(a.output+'.database-ingress.json').write_text(json.dumps({'apiVersion':'v1','kind':'List','items':database_ingress}))
