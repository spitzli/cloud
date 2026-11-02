#!/usr/bin/env python3
"""Offline strict-host Caddy gateway renderer. Never applies or publishes anything."""
import argparse
import hashlib
import ipaddress
import json
from pathlib import Path
import re
import urllib.parse

ROOT=Path(__file__).resolve().parent
VERSION='2.11.7'
SURFACES={'console':'consoleOrigin','studio':'studioOrigin','account':'authOrigin','workspace':'apiOrigin'}
ACCESS=['/api/access','/api/tenants','/api/tenant/*','/api/sites','/api/invitations','/api/invitation/*']
BROWSER=['/console-config.json','/api/locale','/api/console/*','/api/sso/login','/api/sso/callback','/api/sso/logout','/api/users/logout','/api/users/refresh-token','/api/tenant-preview/*','/api/portal/studio']
PATHS={
 'console':BROWSER,
 'studio':BROWSER+ACCESS+['/api/mcp','/api/registry','/api/registry/*','/api/hosting','/api/hosting/*'],
 'account':['/console-config.json','/api/locale','/api/auth/*',*ACCESS,'/api/databases/gateway','/api/mail/gateway','/.well-known/openid-configuration','/.well-known/oauth-authorization-server/api/auth'],
 'workspace':['/console-config.json','/v1/*'],
}
REMOVE=['Forwarded','X-Forwarded-*','X-Vercel-*','X-Real-IP','Via','X-Original-URL','X-Original-URI','X-Rewrite-URL','Connection','Proxy-Connection','Keep-Alive','Proxy-Authenticate','Proxy-Authorization','Proxy','TE','Trailer','Transfer-Encoding','Upgrade']
UNSAFE=r'(?i)^[^?]*(?:%2[ef]|%5c|%25|%3[f]|%23|%0[0-9a-f]|%1[0-9a-f]|%7f|\\|//|/[.]{1,2}(?:/|[?]|$))'


def name(value,limit=63):
 if not isinstance(value,str) or len(value)>limit or not re.fullmatch(r'[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?',value) or any(not part or len(part)>63 or part[0]=='-' or part[-1]=='-' for part in value.split('.')):raise ValueError('a bounded DNS name is required')
 return value


def url(value,scheme):
 if not isinstance(value,str) or len(value)>512 or any(ord(char)<=32 or ord(char)>=127 for char in value) or any(char in value for char in '\\?#{}"\'`$'):raise ValueError('literal safe origin required')
 try:u=urllib.parse.urlsplit(value);port=u.port
 except ValueError:raise ValueError('invalid origin') from None
 if u.scheme!=scheme or not u.hostname or u.username is not None or u.password is not None or u.path not in ('','/') or u.query or u.fragment:raise ValueError('root HTTP(S) origin required')
 return u,port


def origin(value):
 u,port=url(value,'https');host=u.hostname.lower();name(host,253)
 try:ipaddress.ip_address(host)
 except ValueError:pass
 else:raise ValueError('canonical frontend/gateway DNS origin required')
 if '.' not in host or port not in (None,443):raise ValueError('canonical HTTPS DNS origin on443 required')
 return 'https://'+host


def upstream(value,namespace):
 u,port=url(value,'http');host=u.hostname.lower();port=port or 80
 if not 1<=port<=65535:raise ValueError('finite upstream port required')
 try:address=ipaddress.ip_address(host)
 except ValueError:
  name(host,253)
  parts=host.split('.')
  if len(parts)!=1 and not (len(parts)>=3 and parts[1]==namespace and '.'.join(parts[2:]) in ['svc','svc.cluster.local']):raise ValueError('operator-owned private Kubernetes upstream required')
  return 'http://'+host+':'+str(port),host,port,None
 networks=['10.0.0.0/8','172.16.0.0/12','192.168.0.0/16','127.0.0.0/8','fc00::/7','::1/128']
 if not any(address.version==network.version and address in network for network in map(ipaddress.ip_network,networks)):raise ValueError('metadata/public/unspecified upstream rejected')
 authority='['+str(address)+']' if address.version==6 else str(address)
 return 'http://'+authority+':'+str(port),str(address),port,str(address)+('/128' if address.version==6 else '/32')


def labels(value):
 if not isinstance(value,dict) or not 1<=len(value)<=8:raise ValueError('explicit bounded workload selector required')
 for key,item in value.items():
  if not isinstance(key,str) or not isinstance(item,str) or not re.fullmatch(r'[A-Za-z0-9./_-]{1,253}',key) or not re.fullmatch(r'[A-Za-z0-9._-]{1,63}',item):raise ValueError('invalid workload label')
 return value


def validate(c):
 if set(c)!=set(json.loads((ROOT/'profile.example.json').read_text())) or c['version']!=1 or type(c['enabled']) is not bool:raise ValueError('exact versioned gateway config required')
 if not c['enabled']:return c
 for key in ['namespace','coreNamespace','ingressControllerNamespace','ingressClassName','tlsSecretName']:name(c[key])
 if not isinstance(c['caddyImage'],str) or not re.fullmatch(r'docker.io/library/caddy@sha256:[a-f0-9]{64}',c['caddyImage']):raise ValueError('operator-verified immutable official Caddy image pin required')
 if type(c['listenPort']) is not int or not 1024<=c['listenPort']<=65535 or c['listenPort']==2019:raise ValueError('nonprivileged dedicated HTTP port required')
 origins=[origin(c[key]) for key in ['gatewayOrigin',*SURFACES.values()]]
 if len(set(origins))!=5:raise ValueError('distinct canonical surface/technical origins required')
 labels(c['ingressControllerPodLabels']);labels(c['corePodLabels']);upstream(c['coreUpstream'],c['coreNamespace'])
 return c


def caddyfile(c):
 gateway=urllib.parse.urlsplit(origin(c['gatewayOrigin'])).netloc
 core,_,_,_=upstream(c['coreUpstream'],c['coreNamespace'])
 lines=['{','  admin off','  auto_https off','  persist_config off','  storage file_system {','    root /tmp/caddy-data','  }','}',':'+str(c['listenPort'])+' {','  route {','    @wrong_host not host '+gateway,'    respond @wrong_host 404','    @unsafe expression `{http.request.uri}.matches('+json.dumps(UNSAFE)+')`','    respond @unsafe 400','    respond /healthz "ok" 200']
 blocked=['/__'+surface+'/api/hosting/git/worker*' for surface in SURFACES]
 lines+=['    @private_worker path '+' '.join(blocked),'    respond @private_worker 404']
 for surface,key in SURFACES.items():
  prefix='/__'+surface;host=urllib.parse.urlsplit(origin(c[key])).netloc
  lines+=['    @'+surface+' path '+' '.join(prefix+path for path in PATHS[surface]),'    handle @'+surface+' {','      uri strip_prefix '+prefix,'      reverse_proxy '+core+' {','        header_up Host '+host]
  lines+=['        header_up -'+header for header in REMOVE]
  lines+=['        flush_interval -1','        transport http {','          compression off','          dial_timeout 5s','          response_header_timeout 30s','        }','      }','    }']
 lines+=['    respond 404','  }','}']
 return '\n'.join(lines)+'\n'


def vercel(c,surface):
 prefix='/__'+surface;gateway=origin(c['gatewayOrigin'])
 rewrites=[];headers=[]
 for path in PATHS[surface]:
  if path.startswith('/.well-known'):continue
  source=path[:-1]+':path*' if path.endswith('*') else path
  rewrites.append({'source':source,'destination':gateway+prefix+source})
  headers.append({'source':source,'headers':[{'key':'x-vercel-enable-rewrite-caching','value':'0'}]})
 return {'$schema':'https://openapi.vercel.sh/vercel.json','rewrites':rewrites,'headers':headers}


def resources(c,text):
 namespace=c['namespace'];component={'app.kubernetes.io/name':'webdock-backend-gateway'};host=urllib.parse.urlsplit(origin(c['gatewayOrigin'])).hostname;port=c['listenPort']
 digest=hashlib.sha256(text.encode()).hexdigest()[:12];config_name='webdock-backend-gateway-'+digest
 def metadata(value):return {'name':value,'namespace':namespace,'labels':component}
 security={'runAsNonRoot':True,'runAsUser':10001,'runAsGroup':10001,'fsGroup':10001,'seccompProfile':{'type':'RuntimeDefault'}}
 probe={'httpGet':{'path':'/healthz','port':'http','httpHeaders':[{'name':'Host','value':host}]},'timeoutSeconds':2,'periodSeconds':10}
 _,_,core_port,cidr=upstream(c['coreUpstream'],c['coreNamespace'])
 peer=[{'ipBlock':{'cidr':cidr}}] if cidr else [{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':c['coreNamespace']}},'podSelector':{'matchLabels':c['corePodLabels']}}]
 return [
  {'apiVersion':'v1','kind':'ServiceAccount','metadata':metadata('webdock-backend-gateway'),'automountServiceAccountToken':False},
  {'apiVersion':'v1','kind':'ConfigMap','metadata':metadata(config_name),'immutable':True,'data':{'Caddyfile':text}},
  {'apiVersion':'apps/v1','kind':'Deployment','metadata':metadata('webdock-backend-gateway'),'spec':{'replicas':1,'revisionHistoryLimit':2,'selector':{'matchLabels':component},'template':{'metadata':{'labels':component,'annotations':{'webdock.dev/caddy-version':VERSION}},'spec':{'serviceAccountName':'webdock-backend-gateway','automountServiceAccountToken':False,'enableServiceLinks':False,'securityContext':security,'containers':[{'name':'gateway','image':c['caddyImage'],'imagePullPolicy':'IfNotPresent','command':['/usr/bin/caddy'],'args':['run','--config','/etc/caddy/Caddyfile','--adapter','caddyfile'],'ports':[{'name':'http','containerPort':port}],'securityContext':{'privileged':False,'readOnlyRootFilesystem':True,'allowPrivilegeEscalation':False,'capabilities':{'drop':['ALL']}},'resources':{'requests':{'cpu':'50m','memory':'32Mi','ephemeral-storage':'16Mi'},'limits':{'cpu':'500m','memory':'128Mi','ephemeral-storage':'32Mi'}},'readinessProbe':probe,'livenessProbe':probe,'volumeMounts':[{'name':'profile','mountPath':'/etc/caddy','readOnly':True},{'name':'tmp','mountPath':'/tmp'}]}],'volumes':[{'name':'profile','configMap':{'name':config_name,'defaultMode':0o444}},{'name':'tmp','emptyDir':{'medium':'Memory','sizeLimit':'8Mi'}}]}}}},
  {'apiVersion':'v1','kind':'Service','metadata':metadata('webdock-backend-gateway'),'spec':{'type':'ClusterIP','selector':component,'ports':[{'name':'http','port':port,'targetPort':'http'}]}},
  {'apiVersion':'networking.k8s.io/v1','kind':'Ingress','metadata':metadata('webdock-backend-gateway'),'spec':{'ingressClassName':c['ingressClassName'],'tls':[{'hosts':[host],'secretName':c['tlsSecretName']}],'rules':[{'host':host,'http':{'paths':[{'path':'/','pathType':'Prefix','backend':{'service':{'name':'webdock-backend-gateway','port':{'name':'http'}}}}]}}]}},
  {'apiVersion':'networking.k8s.io/v1','kind':'NetworkPolicy','metadata':metadata('webdock-backend-gateway'),'spec':{'podSelector':{'matchLabels':component},'policyTypes':['Ingress','Egress'],'ingress':[{'from':[{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':c['ingressControllerNamespace']}},'podSelector':{'matchLabels':c['ingressControllerPodLabels']}}],'ports':[{'protocol':'TCP','port':port}]}],'egress':[{'to':peer,'ports':[{'protocol':'TCP','port':core_port}]},{'to':[{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':'kube-system'}},'podSelector':{'matchLabels':{'k8s-app':'kube-dns'}}}],'ports':[{'protocol':'UDP','port':53},{'protocol':'TCP','port':53}]}]}}
 ]


def render(c):
 validate(c)
 if not c['enabled']:return {'enabled':False,'resources':[]}
 text=caddyfile(c)
 return {'enabled':True,'applied':False,'Caddyfile':text,'resources':resources(c,text),'vercel':{surface:vercel(c,surface) for surface in SURFACES},'qualificationGates':{'image':'operator must verify official immutable image is the qualified maintained version; binary proof is not an image/deployment proof','wellKnown':'native static authoritative public metadata or separately qualified routing needed; Vercel does not rewrite reserved well-known paths','platform':'actual Vercel request/body/query/cookies/SSE/timeouts/cache and RUN ingress/Core integration are unqualified','corePrivate':'controller worker API is excluded; its private internal canonical-host route is separate from this public gateway'}}


def alias_hosts(c,values):
 if values is None:return []
 if not isinstance(values,list) or len(values)>4:raise ValueError('bounded explicit alias origins required')
 aliases=[origin(value) for value in values]
 reserved={origin(c[key]) for key in ['gatewayOrigin',*SURFACES.values()]}
 if len(set(aliases))!=len(aliases) or reserved.intersection(aliases):raise ValueError('alias conflicts with a canonical surface')
 return [urllib.parse.urlsplit(value).netloc for value in aliases]


def native_https(c,api_public=True,api_aliases=None,navigation_aliases=None):
 """Owned native Caddy edge when a cluster has no ingress controller."""
 validate(c)
 if not c['enabled']:raise ValueError('explicit enabled profile required')
 if type(api_public) is not bool:raise ValueError('explicit API activation boolean required')
 extra=alias_hosts(c,api_aliases)
 navigation={} if navigation_aliases is None else navigation_aliases
 if not isinstance(navigation,dict) or len(navigation)>4:raise ValueError('bounded explicit service navigation aliases required')
 nav_hosts=alias_hosts(c,list(navigation))
 targets=[origin(value) for value in navigation.values()]
 if any(value not in ['https://docs.webdock.dev','https://registry.webdock.dev'] for value in targets):raise ValueError('unreviewed service navigation destination')
 if set(nav_hosts).intersection(extra):raise ValueError('navigation alias conflicts with an API alias')
 if (extra or navigation) and not api_public:raise ValueError('public aliases require explicit API activation')
 gateway=urllib.parse.urlsplit(origin(c['gatewayOrigin'])).netloc
 api=urllib.parse.urlsplit(origin(c['apiOrigin'])).netloc
 core,_,_,_=upstream(c['coreUpstream'],c['coreNamespace'])
 marker=':'+str(c['listenPort'])+' {'
 technical=caddyfile(c).split(marker,1)[1]
 if not api_public:return '{\n  admin off\n  persist_config off\n}\n'+gateway+' {'+technical
 lines=['{','  admin off','  persist_config off','  servers {','    protocols h1 h2','  }','}',gateway+' {'+technical]
 body=['  route {','    @private_worker path /api/hosting/git/worker*','    respond @private_worker 404','    @unsafe expression `{http.request.uri}.matches('+json.dumps(UNSAFE)+')`','    respond @unsafe 400','    @public path /healthz /readyz /v1/* /api/mcp /api/registry /api/registry/* /api/hosting /api/hosting/* /.well-known/oauth-protected-resource/api/mcp','    handle @public {','      reverse_proxy '+core+' {','        header_up Host '+api]
 body+=['        header_up -'+header for header in REMOVE]
 body+=['        flush_interval -1','        transport http {','          compression off','          dial_timeout 5s','          response_header_timeout 30s','        }','      }','    }','    respond 404','  }']
 for host in [api,*extra]:lines += [host+' {',*body,'}']
 for host,target in zip(nav_hosts,targets):
  lines += [host+' {','  route {','    @protocol path /api /api/* /v1 /v1/* /.well-known /.well-known/* /auth /auth/* /mcp /mcp/*','    respond @protocol 404','    @navigation method GET HEAD','    handle @navigation {','      header Cache-Control no-store','      redir '+target+'{uri} 307','    }','    respond 404','  }','}']
 return '\n'.join(lines)+'\n'


def vercel_output(c,surface,database_gateway=None,navigation_aliases=None,realtime_url=None):
 """Build Output v3: static native UI, live protected APIs, exact Studio compatibility."""
 validate(c)
 if not c['enabled'] or surface not in ['account','console']:raise ValueError('explicit frontend surface required')
 connections=[origin(c[key]) for key in SURFACES.values()]
 frames=origin(database_gateway) if database_gateway else "'none'"
 if database_gateway:connections.append(frames)
 if realtime_url:
  realtime,port=url(realtime_url,'wss');name(realtime.hostname,253)
  if not realtime.hostname.endswith('.neon.tech') or port is not None:raise ValueError('exact Neon realtime origin required')
  connections.append('https://'+realtime.hostname)
 sockets=[v.replace('https://','wss://',1) for v in connections]
 csp="default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; worker-src 'self' blob:; connect-src 'self' "+' '.join(connections+sockets)+"; frame-src "+frames+"; frame-ancestors 'none'; base-uri 'none'; object-src 'none'; form-action 'self'"
 routes=[{'src':'^/.*$','headers':{'content-security-policy':csp,'x-content-type-options':'nosniff','x-frame-options':'DENY','referrer-policy':'strict-origin-when-cross-origin','strict-transport-security':'max-age=31536000'},'continue':True}]
 for host in alias_hosts(c,navigation_aliases):
  match=[{'type':'host','value':{'eq':host}}]
  routes += [{'src':'^/(?:api|v1|\\.well-known)(?:/.*)?$','has':match,'status':404,'headers':{'cache-control':'no-store'}},{'src':'^/(.*)$','has':match,'status':307,'headers':{'Location':origin(c[SURFACES[surface]])+'/$1','cache-control':'no-store'}}]
 gateway=origin(c['gatewayOrigin'])
 def proxy(kind,host=None):
  for path in PATHS[kind]:
   if path.startswith('/.well-known'):continue
   wildcard=path.endswith('*');literal=path[:-1] if wildcard else path
   rule={'src':'^'+re.escape(literal)+('(.*)' if wildcard else '')+'$','dest':gateway+'/__'+kind+literal+('$1' if wildcard else ''),'headers':{'cache-control':'private, no-store','x-vercel-enable-rewrite-caching':'0'},'caseSensitive':True}
   if host:rule['has']=[{'type':'host','value':{'eq':host}}]
   routes.append(rule)
 if surface=='console':proxy('studio',urllib.parse.urlsplit(origin(c['studioOrigin'])).hostname)
 proxy(surface)
 routes += [{'src':'^/assets/.*$','headers':{'cache-control':'public, max-age=31536000, immutable'},'continue':True},{'handle':'filesystem'},{'src':'^/(?:api|v1|assets|\\.well-known)(?:/.*)?$','status':404},{'src':'^/.*\\.[a-zA-Z0-9]+$','status':404},{'src':'^/.*$','dest':'/index.html','headers':{'cache-control':'no-store'}}]
 return {'version':3,'framework':{'slug':'vite','version':'8.3.4'},'routes':routes}


def main():
 parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--config',type=Path,required=True);args=parser.parse_args()
 try:print(json.dumps(render(json.loads(args.config.read_text())),indent=2))
 except (ValueError,TypeError,KeyError,OSError):parser.error('invalid or incomplete operator gateway profile; no deployment performed')


if __name__=='__main__':main()
