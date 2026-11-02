#!/usr/bin/env python3
"""Private operator-only Loki gateway artifacts, not a customer tenant gateway."""
import ipaddress
NAMESPACE='webdock-observability'
HOST='obs-loki-auth.webdock-observability.svc.cluster.local'
IMAGE='docker.io/library/caddy:2.11.7@sha256:36c24ad91a7ff7bc4c87fe22bfa8df3c4b18da0773d73fc12ee2c52b98c0bb7e'
def caddyfile(reader_hash,collector_hash):
 return '''{
 admin off
 auto_https off
 persist_config off
}
https://'''+HOST+''':8443 {
 tls /tls/tls.crt /tls/tls.key
 request_body {
  max_size 1MB
 }
 @write {
  method POST
  path /loki/api/v1/push
 }
 handle @write {
  basic_auth {
   collector '''+collector_hash+'''
  }
  reverse_proxy obs-loki:3100 {
   header_up X-Scope-OrgID platform
   header_up -Authorization
  }
 }
 @read {
  method GET
  path /loki/api/v1/query /loki/api/v1/query_range /loki/api/v1/labels /loki/api/v1/label/*/values /loki/api/v1/series /loki/api/v1/index/stats
 }
 handle @read {
  basic_auth {
   reader '''+reader_hash+'''
  }
  reverse_proxy obs-loki:3100 {
   header_up X-Scope-OrgID platform
   header_up -Authorization
  }
 }
 respond 404
}
'''
def dns():
 return {'to':[{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':'kube-system'}},'podSelector':{'matchLabels':{'k8s-app':'kube-dns'}}}],'ports':[{'protocol':'UDP','port':53},{'protocol':'TCP','port':53}]}
def policies(addresses):
 if not addresses or len(addresses)>64:raise ValueError('Bounded exact Wasabi endpoint addresses required')
 for value in addresses:
  address=ipaddress.ip_address(value)
  if address.version!=4 or not address.is_global:raise ValueError('Only public Wasabi IPv4 endpoints')
 def policy(name,spec):return {'apiVersion':'networking.k8s.io/v1','kind':'NetworkPolicy','metadata':{'name':name,'namespace':NAMESPACE},'spec':spec}
 return [policy('obs-default-deny',{'podSelector':{},'policyTypes':['Ingress','Egress'],'ingress':[],'egress':[]}),policy('obs-infrastructure-private',{'podSelector':{'matchExpressions':[{'key':'app.kubernetes.io/name','operator':'NotIn','values':['loki']}]},'policyTypes':['Ingress','Egress'],'ingress':[{'from':[{'podSelector':{}}]}],'egress':[{'to':[{'podSelector':{}}]},dns(),{'to':[{'ipBlock':{'cidr':'10.0.4.2/32'}}],'ports':[{'protocol':'TCP','port':6443}]}]}),policy('obs-loki-gateway-only',{'podSelector':{'matchLabels':{'app.kubernetes.io/name':'loki'}},'policyTypes':['Ingress','Egress'],'ingress':[{'from':[{'podSelector':{'matchLabels':{'app.kubernetes.io/name':'obs-loki-auth'}}}],'ports':[{'protocol':'TCP','port':3100}]}],'egress':[dns(),{'to':[{'ipBlock':{'cidr':value+'/32'}} for value in addresses],'ports':[{'protocol':'TCP','port':443}]}]})]
def workloads():
 labels={'app.kubernetes.io/name':'obs-loki-auth'}
 result=[{'apiVersion':'apps/v1','kind':'Deployment','metadata':{'name':'obs-loki-auth','namespace':NAMESPACE},'spec':{'replicas':1,'selector':{'matchLabels':labels},'template':{'metadata':{'labels':labels},'spec':{'automountServiceAccountToken':False,'enableServiceLinks':False,'securityContext':{'runAsNonRoot':True,'runAsUser':65532,'runAsGroup':65532,'fsGroup':65532,'seccompProfile':{'type':'RuntimeDefault'}},'containers':[{'name':'gateway','image':IMAGE,'command':['caddy'],'args':['run','--config','/gateway/Caddyfile','--adapter','caddyfile'],'ports':[{'name':'https','containerPort':8443}],'securityContext':{'allowPrivilegeEscalation':False,'readOnlyRootFilesystem':True,'capabilities':{'drop':['ALL']}},'resources':{'requests':{'cpu':'25m','memory':'64Mi','ephemeral-storage':'16Mi'},'limits':{'cpu':'250m','memory':'128Mi','ephemeral-storage':'32Mi'}},'readinessProbe':{'tcpSocket':{'port':'https'},'periodSeconds':5},'volumeMounts':[{'name':'gateway','mountPath':'/gateway','readOnly':True},{'name':'tls','mountPath':'/tls','readOnly':True},{'name':'tmp','mountPath':'/tmp'},{'name':'data','mountPath':'/data'},{'name':'config','mountPath':'/config'}]}],'volumes':[{'name':'gateway','secret':{'secretName':'obs-loki-gateway','defaultMode':0o440}},{'name':'tls','secret':{'secretName':'obs-loki-gateway-tls','defaultMode':0o440}},{'name':'tmp','emptyDir':{'medium':'Memory','sizeLimit':'8Mi'}},{'name':'data','emptyDir':{'medium':'Memory','sizeLimit':'8Mi'}},{'name':'config','emptyDir':{'medium':'Memory','sizeLimit':'8Mi'}}]}}}}, {'apiVersion':'v1','kind':'Service','metadata':{'name':'obs-loki-auth','namespace':NAMESPACE},'spec':{'type':'ClusterIP','selector':labels,'ports':[{'name':'https','port':443,'targetPort':'https'}]}}]

 pod=result[0]['spec']['template']['spec'];main=pod['containers'][0]
 # Official Caddy has cap_net_bind_service on its file; dropALL rejects that exec.
 # Nonroot copy drops the unnecessary xattr for8443 while preserving exact bytes.
 main['command']=['/work/caddy'];main['resources']['requests']['memory']='128Mi';main['resources']['limits']['memory']='256Mi'
 main['volumeMounts'].append({'name':'binary','mountPath':'/work','readOnly':True})
 pod['volumes'].append({'name':'binary','emptyDir':{'medium':'Memory','sizeLimit':'64Mi'}})
 pod['initContainers']=[{'name':'prepare-binary','image':IMAGE,'command':['/bin/sh'],'args':['-ec','cp /usr/bin/caddy /work/caddy; chmod 0555 /work/caddy; cmp /usr/bin/caddy /work/caddy'],'securityContext':{'allowPrivilegeEscalation':False,'readOnlyRootFilesystem':True,'capabilities':{'drop':['ALL']}},'resources':{'requests':{'cpu':'25m','memory':'64Mi','ephemeral-storage':'16Mi'},'limits':{'cpu':'250m','memory':'128Mi','ephemeral-storage':'32Mi'}},'volumeMounts':[{'name':'binary','mountPath':'/work'}]}]
 return result
