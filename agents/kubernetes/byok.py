"""Bounded inventory and UID-fenced operations on existing customer workloads."""
import json
import math
import os
import re
import subprocess
import time
from datetime import datetime, timezone
from decimal import Decimal
from observer import quantity
KINDS={'Node':'nodes','Namespace':'namespaces','Deployment':'deployments.apps','StatefulSet':'statefulsets.apps','DaemonSet':'daemonsets.apps','Pod':'pods','Job':'jobs.batch','CronJob':'cronjobs.batch','Service':'services','Ingress':'ingresses.networking.k8s.io','PersistentVolumeClaim':'persistentvolumeclaims','StorageClass':'storageclasses.storage.k8s.io'}
SYSTEM={'kube-system','kube-public','kube-node-lease','spitzli-agent'}
def allowed_namespace(namespace,allowed):
 return namespace in allowed and namespace not in SYSTEM and bool(re.fullmatch(r'[a-z0-9]([-a-z0-9]*[a-z0-9])?',namespace))
def kubectl(*args,timeout=20):
 command=[os.environ.get('WEBDOCK_KUBECTL','kubectl')]
 if os.environ.get('WEBDOCK_KUBECTL_MODE')=='k3s':command+=['kubectl']
 if os.environ.get('WEBDOCK_KUBECTL_CACHE_DIR'):command+=['--cache-dir',os.environ['WEBDOCK_KUBECTL_CACHE_DIR']]
 if os.environ.get('WEBDOCK_KUBECONFIG'):command+=['--kubeconfig',os.environ['WEBDOCK_KUBECONFIG']]
 result=subprocess.run(command+list(args),capture_output=True,text=True,timeout=timeout,check=True)
 if len(result.stdout)>2_000_000:raise ValueError('response_too_large')
 return result.stdout

def obj(kind,name,namespace=None):
 args=['get',KINDS[kind],name,'-o','json']
 if namespace:args+=['-n',namespace]
 return json.loads(kubectl(*args))

def cluster_uid():return obj('Namespace','kube-system')['metadata']['uid']

def summarize(item,kind):
 m=item['metadata'];s=item.get('status',{});spec=item.get('spec',{})
 result={'uid':m['uid'],'resourceVersion':m['resourceVersion'],'kind':kind,'name':m['name'],'namespace':m.get('namespace',''),'status':'unknown'}
 if kind=='Node':
  result.update(status='ready' if any(c['type']=='Ready' and c['status']=='True' for c in s.get('conditions',[])) else 'failed',cpuMillicores=quantity(s.get('allocatable',{}).get('cpu','0'),True),memoryBytes=quantity(s.get('allocatable',{}).get('memory','0')))
 elif kind in ('Deployment','StatefulSet','DaemonSet'):
  replicas=s.get('desiredNumberScheduled',0) if kind=='DaemonSet' else spec.get('replicas',1)
  ready=s.get('numberReady',0) if kind=='DaemonSet' else s.get('readyReplicas',0)
  result.update(replicas=replicas,ready=ready,status='stopped' if replicas==0 else 'ready' if ready==replicas else 'pending')
 elif kind=='Pod':
  statuses=s.get('containerStatuses',[])
  result.update(status='ready' if s.get('phase')=='Succeeded' or (s.get('phase')=='Running' and statuses and all(c.get('ready') for c in statuses)) else 'failed' if s.get('phase')=='Failed' else 'pending',restarts=sum(c.get('restartCount',0) for c in statuses))
 elif kind=='CronJob':result.update(suspended=bool(spec.get('suspend')),status='stopped' if spec.get('suspend') else 'ready')
 elif kind=='Job':result['status']='ready' if s.get('succeeded') else 'failed' if s.get('failed') else 'pending'
 elif kind=='PersistentVolumeClaim':result.update(storageBytes=quantity(spec.get('resources',{}).get('requests',{}).get('storage','0')),status='ready' if s.get('phase')=='Bound' else 'pending')
 else:result['status']='ready'
 containers=spec.get('containers',spec.get('template',{}).get('spec',{}).get('containers',[]))
 if containers:
  result['cpuMillicores']=sum(quantity(c.get('resources',{}).get('requests',{}).get('cpu','0'),True) for c in containers)
  result['memoryBytes']=sum(quantity(c.get('resources',{}).get('requests',{}).get('memory','0')) for c in containers)
 managers=[str(f.get('manager','')).lower() for f in m.get('managedFields',[])]
 labels=m.get('labels',{})
 result['controlled']=any(any(x in manager for x in ('argocd','flux','terraform')) for manager in managers) or any(k.startswith(('argocd.argoproj.io/','kustomize.toolkit.fluxcd.io/','helm.toolkit.fluxcd.io/')) for k in labels)
 return result

def scan(namespaces):
 result={'clusterUID':cluster_uid(),'observedAt':datetime.now(timezone.utc).isoformat(timespec='milliseconds').replace('+00:00','Z'),'complete':True,'unavailable':[],'resources':[]}
 deadline=time.monotonic()+45
 for kind,plural in KINDS.items():
  scopes=[None] if kind in ('Node','Namespace','StorageClass') else [n for n in namespaces if allowed_namespace(n,namespaces)]
  for namespace in scopes:
   cursor=''
   try:
    while True:
     if time.monotonic()>deadline or len(result['resources'])>=2000:raise ValueError('scan_limit')
     args=['get',plural,'-o','json','--chunk-size=100','--limit=100']
     # kubectl get follows chunks internally; raw API gives bounded continuation pages.
     group='/api/v1' if kind in ('Node','Namespace','Pod','Service','PersistentVolumeClaim') else '/apis/apps/v1' if kind in ('Deployment','StatefulSet','DaemonSet') else '/apis/batch/v1' if kind in ('Job','CronJob') else '/apis/networking.k8s.io/v1' if kind=='Ingress' else '/apis/storage.k8s.io/v1'
     from urllib.parse import urlencode
     endpoint=group+('/namespaces/'+namespace if namespace else '')+'/'+plural.split('.')[0]+'?'+urlencode({'limit':100,**({'continue':cursor} if cursor else {})})
     page=json.loads(kubectl('get','--raw',endpoint,timeout=max(1,min(20,deadline-time.monotonic()))))
     for item in page.get('items',[]):
      if kind=='Namespace' and item['metadata']['name'] not in namespaces:continue
      result['resources'].append(summarize(item,kind))
      if len(result['resources'])>=2000:raise ValueError('scan_limit')
     cursor=page.get('metadata',{}).get('continue','')
     if not cursor:break
   except Exception:
    result['complete']=False
    if kind not in result['unavailable']:result['unavailable'].append(kind)
 # Metrics are optional; never turn missing metrics into measured zeros.
 for ns in [n for n in namespaces if allowed_namespace(n,namespaces)]:
  try:
   if time.monotonic()>=deadline:
    if 'metrics' not in result['unavailable']:result['unavailable'].append('metrics')
    break
   metrics=json.loads(kubectl('get','--raw','/apis/metrics.k8s.io/v1beta1/namespaces/'+ns+'/pods',timeout=max(1,min(20,deadline-time.monotonic()))))
   by_name={x['metadata']['name']:x for x in metrics.get('items',[])}
   for resource in result['resources']:
    if resource['kind']=='Pod' and resource['namespace']==ns and resource['name'] in by_name:
     containers=by_name[resource['name']].get('containers',[])
     # CPU metrics commonly use fractional millicores: round up for display only.
     resource['usageCPU']=sum(math.ceil(float(c['usage']['cpu'].removesuffix('n'))/1e6) if c['usage']['cpu'].endswith('n') else quantity(c['usage']['cpu'],True) for c in containers)
     resource['usageMemory']=sum(quantity(c['usage']['memory']) for c in containers)
  except Exception:
   if 'metrics' not in result['unavailable']:result['unavailable'].append('metrics')
 return result

def mutation_patch(current,action,replicas,operation):
 m=current['metadata'];patch=[{'op':'test','path':'/metadata/uid','value':m['uid']},{'op':'test','path':'/metadata/resourceVersion','value':m['resourceVersion']}]
 if action in ('scale','stop','start'):
  if type(replicas) is not int or not 0<=replicas<=100:raise ValueError('invalid_replicas')
  patch.append({'op':'add','path':'/spec/replicas','value':replicas})
 elif action in ('suspend','resume'):patch.append({'op':'add','path':'/spec/suspend','value':action=='suspend'})
 elif action=='restart':
  meta=current['spec']['template'].get('metadata',{})
  if not meta.get('annotations'):patch.append({'op':'add','path':'/spec/template/metadata/annotations','value':{'webdock.dev/restart':operation}})
  else:patch.append({'op':'add','path':'/spec/template/metadata/annotations/webdock.dev~1restart','value':operation})
 else:raise ValueError('unsupported_action')
 return patch

def rollout_ready(current,kind,operation):
 spec=current.get('spec',{});status=current.get('status',{})
 n=status.get('desiredNumberScheduled',0) if kind=='DaemonSet' else spec.get('replicas',1)
 updated=status.get('updatedNumberScheduled',0) if kind=='DaemonSet' else status.get('updatedReplicas',0)
 ready=status.get('numberReady',0) if kind=='DaemonSet' else status.get('readyReplicas',0)
 if status.get('observedGeneration',0)<current['metadata'].get('generation',0) or updated!=n or ready!=n:return False
 if spec.get('template',{}).get('metadata',{}).get('annotations',{}).get('webdock.dev/restart')!=operation:return False
 if kind=='Deployment':return status.get('replicas',0)==n and status.get('availableReplicas',0)==n
 if kind=='StatefulSet':return status.get('replicas',0)==n and status.get('currentRevision')==status.get('updateRevision')
 return status.get('numberAvailable',0)==n

def safe_scaling(current,replicas):
 spec=current.get('spec',{})
 return not (replicas<spec.get('replicas',1) and spec.get('persistentVolumeClaimRetentionPolicy',{}).get('whenScaled')=='Delete')

def execute(packet,allowed):
 target=packet['resource'];kind=target['kind'];ns=target['namespace'];action=packet['action']
 def valid_lease():
  if datetime.now(timezone.utc)>=datetime.fromisoformat(packet['leaseUntil'].replace('Z','+00:00')):raise ValueError('lease_expired')
 valid_lease()
 if not allowed_namespace(ns,allowed) or cluster_uid()!=packet['clusterUID']:raise ValueError('ownership_conflict')
 current=obj(kind,target['name'],ns)
 if current['metadata']['uid']!=target['uid'] or current['metadata']['resourceVersion']!=target['resourceVersion']:raise ValueError('ownership_conflict')
 if action=='logs':
  text=kubectl('-n',ns,'logs',KINDS[kind].split('.')[0]+'/'+target['name'],'--tail=100','--limit-bytes=12000','--all-containers=true','--prefix=true')
  return {'uid':target['uid'],'action':action,'observed':True,'logs':text[:12000]}
 if summarize(current,kind)['controlled']:raise ValueError('controller_conflict')
 if action in ('scale','stop','start'):
  if kind=='StatefulSet' and not safe_scaling(current,packet['replicas']):raise ValueError('controller_conflict')
  hpas=json.loads(kubectl('-n',ns,'get','hpa','-o','json'))
  if any(h.get('spec',{}).get('scaleTargetRef',{}).get('name')==target['name'] and h['spec']['scaleTargetRef'].get('kind')==kind for h in hpas.get('items',[])):raise ValueError('controller_conflict')
 if action=='restart' and kind=='Deployment' and current.get('spec',{}).get('paused'):raise ValueError('controller_conflict')
 if action=='restart' and kind in ('StatefulSet','DaemonSet'):
  strategy=current.get('spec',{}).get('updateStrategy',{})
  if strategy.get('type','RollingUpdate')!='RollingUpdate' or strategy.get('rollingUpdate',{}).get('partition',0):raise ValueError('controller_conflict')
 operations=mutation_patch(current,action,packet.get('replicas'),packet['operationID'])
 valid_lease()
 args=['-n',ns,'patch',KINDS[kind],target['name'],'--type=json','-p',json.dumps(operations)]
 if action in ('scale','stop','start'):args+=['--subresource=scale']
 kubectl(*args)
 deadline=time.monotonic()+100
 while time.monotonic()<deadline:
  valid_lease();current=obj(kind,target['name'],ns)
  if current['metadata']['uid']!=target['uid']:raise ValueError('ownership_conflict')
  spec=current.get('spec',{});status=current.get('status',{});ready=False
  if action in ('suspend','resume'):ready=bool(spec.get('suspend'))==(action=='suspend')
  elif action in ('scale','stop','start'):
   n=packet['replicas'];ready=spec.get('replicas')==n and status.get('observedGeneration',0)>=current['metadata'].get('generation',0) and status.get('replicas',0)==n and status.get('readyReplicas',0)==n
  else:
   n=status.get('desiredNumberScheduled',0) if kind=='DaemonSet' else spec.get('replicas',1)
   updated=status.get('updatedNumberScheduled',0) if kind=='DaemonSet' else status.get('updatedReplicas',0)
   count=status.get('numberReady',0) if kind=='DaemonSet' else status.get('readyReplicas',0)
   ready=rollout_ready(current,kind,packet['operationID'])
  if ready:return {'uid':target['uid'],'action':action,'observed':True}
  time.sleep(2)
 raise ValueError('rollout_timeout')
