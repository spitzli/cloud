#!/usr/bin/env python3
"""Typed Webdock executor. Only operator-authorized, leased application operations are accepted."""
import copy
import base64
from datetime import datetime, timezone
import fcntl
import json
import os
import re
import subprocess
import sys
import time
import storage_client as storage
import registry
import service_bindings

class ExecutionError(Exception):
    pass


def lease(packet):
    if datetime.fromisoformat(packet['leaseUntil'].replace('Z','+00:00')) <= datetime.now(timezone.utc):
        raise ExecutionError('execution_failed')
    if packet.get('pluginProvisioningGuard') is True:
        state=packet.get('_guard')
        if not isinstance(state,dict) or state.get('clusterID')!=packet['clusterID']:
            raise ExecutionError('execution_failed')
        from connect import agent_request
        result=agent_request(state['endpoint'],state,'guard',{'id':packet['operationID'],'generation':packet['generation']})
        if not isinstance(result,dict) or result.get('valid') is not True:
            raise ExecutionError('execution_failed')


def validate_packet(p):
    try:
        for key in ['appID','projectID','clusterID','operationID']:
            if not isinstance(p[key],str) or not re.fullmatch(r'[1-9][0-9]{0,18}',p[key]):
                raise ValueError()
        if p['namespace'] != 'wd-'+p['projectID'] or p['action'] not in ['apply','delete','logs','purge-storage']:
            raise ValueError()
        if type(p['revision']) is not int or p['revision']<1 or type(p['generation']) is not int or p['generation']<1:
            raise ValueError()
        s=p['spec']
        if not re.fullmatch(r'[A-Za-z0-9.:-]+/[A-Za-z0-9_./-]+@sha256:[a-f0-9]{64}',s['image']):
            raise ValueError()
        for key,low,high in [('cpuMillicores',10,64000),('memoryBytes',16777216,1099511627776),('ephemeralBytes',1048576,1099511627776),('replicas',0,20),('port',1024,65535)]:
            if type(s[key]) is not int or not low<=s[key]<=high:raise ValueError()
        if not re.fullmatch(r'/[A-Za-z0-9/_-]*',s['healthPath']) or len(s['healthPath'])>160:raise ValueError()
        if not isinstance(s['args'],list) or len(s['args'])>20 or any(not isinstance(a,str) or len(a)>512 for a in s['args']):raise ValueError()
        size=s.get('volumeBytes',0)
        if type(size) is not int or not (size==0 or 67108864<=size<=1099511627776):raise ValueError()
        if size and (s['replicas']>1 or p.get('storageVersion')!=1):raise ValueError()
        if p['action']=='purge-storage' and not size:raise ValueError()
        for key in ['apps','cpuMillicores','memoryBytes','ephemeralBytes']:
            if type(p['quota'][key]) is not int or p['quota'][key]<0:raise ValueError()
        if 'environment' in p or 'environmentVersion' in p:
            env=p['environment']
            if p['environmentVersion']!=1 or not isinstance(env,dict) or len(env)>32:raise ValueError()
            total=0
            for key,value in env.items():
                if not re.fullmatch(r'[A-Za-z_][A-Za-z0-9_]{0,127}',key) or key in ['__proto__','constructor','prototype']:raise ValueError()
                if not isinstance(value,str) or '\0' in value or len(value.encode('utf-8'))>4096:raise ValueError()
                total+=len(key.encode('utf-8'))+len(value.encode('utf-8'))
            if total>12000 or len(json.dumps(env,ensure_ascii=False,separators=(',',':')).encode('utf-8'))>16000:raise ValueError()
        service_bindings.validate(p)
        lease(p)
    except (KeyError,ValueError,TypeError):
        raise ExecutionError('execution_failed') from None


def labels(p, app=True):
    result={'security.webdock.dev/managed-by':'webdock','webdock.dev/project-id':p['projectID'],'webdock.dev/cluster-id':p['clusterID']}
    if app:result['webdock.dev/app-id']=p['appID']
    return result


def manifests(p):
    s=p['spec'];name='app-'+p['appID'];owner=labels(p)
    annotation={'webdock.dev/revision':str(p['revision']),'webdock.dev/operation':p['operationID']}
    resource={'cpu':str(s['cpuMillicores'])+'m','memory':str(s['memoryBytes']),'ephemeral-storage':str(s['ephemeralBytes'])}
    probe={'httpGet':{'path':s['healthPath'],'port':s['port']},'periodSeconds':3,'timeoutSeconds':2,'failureThreshold':3}
    container={'name':'app','image':s['image'],'imagePullPolicy':'Never','args':s['args'],'ports':[{'containerPort':s['port']}],
      'securityContext':{'allowPrivilegeEscalation':False,'readOnlyRootFilesystem':True,'capabilities':{'drop':['ALL']}},
      'resources':{'requests':resource,'limits':resource},'readinessProbe':probe,'livenessProbe':probe,'startupProbe':{**probe,'failureThreshold':20},
      'volumeMounts':[{'name':'tmp','mountPath':'/tmp'}]}
    if p.get('environment'):
        container['env']=[{'name':key,'valueFrom':{'secretKeyRef':{'name':environment_secret_name(p),'key':key}}} for key in p['environment']]
    pod={'automountServiceAccountToken':False,'serviceAccountName':'default','terminationGracePeriodSeconds':15,
      'securityContext':{'runAsNonRoot':True,'runAsUser':65532,'runAsGroup':65532,'fsGroup':65532,'seccompProfile':{'type':'RuntimeDefault'}},
      'containers':[container],'volumes':[{'name':'tmp','emptyDir':{'sizeLimit':str(s['ephemeralBytes'])}}]}
    pull=registry.pull_configuration(p)
    if pull:
        container['imagePullPolicy']='IfNotPresent'
        pod['imagePullSecrets']=[pull]
    if s.get('volumeBytes'):
        pod['volumes'].append({'name':'data','persistentVolumeClaim':{'claimName':name+'-data'}})
        container['volumeMounts'].append({'name':'data','mountPath':'/data'})
    service_bindings.mount(p,pod)
    meta={'name':name,'namespace':p['namespace'],'labels':owner,'annotations':annotation}
    deployment={'apiVersion':'apps/v1','kind':'Deployment','metadata':meta,'spec':{'replicas':s['replicas'],'strategy':{'type':'Recreate'},'revisionHistoryLimit':2,'selector':{'matchLabels':{'webdock.dev/app-id':p['appID']}},'template':{'metadata':{'labels':{**owner,'security.webdock.dev/ingress':'true'},'annotations':annotation},'spec':pod}}}
    service={'apiVersion':'v1','kind':'Service','metadata':meta,'spec':{'type':'ClusterIP','selector':{'webdock.dev/app-id':p['appID']},'ports':[{'name':'http','port':s['port'],'targetPort':s['port']}]}}
    return deployment,service


def environment_secret_name(p):
    return 'app-'+p['appID']+'-env-'+str(p['revision'])


def environment_secret(p):
    if not p.get('environment'):return None
    return {'apiVersion':'v1','kind':'Secret','metadata':{'name':environment_secret_name(p),'namespace':p['namespace'],'labels':labels(p),'annotations':{'webdock.dev/revision':str(p['revision'])}},'type':'Opaque','immutable':True,'data':{key:base64.b64encode(value.encode()).decode() for key,value in p['environment'].items()}}


def cleanup_environment(p,keep=()):
    if isinstance(keep,str):keep={keep}
    result=json.loads(kubectl('-n',p['namespace'],'get','secrets','-l','webdock.dev/app-id='+p['appID'],'-o','json'))
    for secret in result['items']:
        name=secret['metadata']['name']
        if name in keep:continue
        if name=='app-'+p['appID']+'-tls' and secret.get('type')=='kubernetes.io/tls' and secret['metadata'].get('annotations',{}).get('cert-manager.io/certificate-name')==name and re.fullmatch(r'[1-9][0-9]{0,18}',secret['metadata'].get('labels',{}).get('webdock.dev/provisioning-id','')):
            check_owner(secret,p,True)
            certificate=get('Certificate',name,p['namespace'])
            if not certificate:raise ExecutionError('ownership_conflict')
            check_owner(certificate,p,True)
            if certificate.get('spec',{}).get('secretName')!=name or certificate['metadata'].get('labels',{}).get('webdock.dev/provisioning-id')!=secret['metadata']['labels']['webdock.dev/provisioning-id']:raise ExecutionError('ownership_conflict')
            refs=secret['metadata'].get('ownerReferences',[])
            if refs and not any(r.get('kind')=='Certificate' and r.get('uid')==certificate['metadata'].get('uid') for r in refs):raise ExecutionError('ownership_conflict')
            continue
        if re.fullmatch('app-'+p['appID']+r'-services-[1-9][0-9]*',name):continue
        if not re.fullmatch('app-'+p['appID']+r'-env-[1-9][0-9]*',name):raise ExecutionError('ownership_conflict')
        guarded_delete('secrets',secret,p)


def cleanup_service_secrets(p,keep=()):
    if p.get('serviceBindingsVersion')!=1:return
    result=json.loads(kubectl('-n',p['namespace'],'get','secrets','-l','webdock.dev/app-id='+p['appID'],'-o','json'))
    for secret in result['items']:
        name=secret['metadata']['name']
        if re.fullmatch('app-'+p['appID']+r'-services-[1-9][0-9]*',name) and name not in keep:
            guarded_delete('secrets',secret,p)


def referenced_environment(objects):
    names=set()
    for obj in objects:
        spec=obj.get('spec',{})
        spec=spec.get('template',{}).get('spec',spec)
        for container in spec.get('containers',[]):
            for env in container.get('env',[]):
                name=env.get('valueFrom',{}).get('secretKeyRef',{}).get('name')
                if name:names.add(name)
    return names


def kubectl(*args,obj=None,timeout=20):
    command=['k3s','kubectl']
    if os.environ.get('SPITZLI_KUBECTL_CACHE_DIR'):
        command+=['--cache-dir',os.environ['SPITZLI_KUBECTL_CACHE_DIR']]
    result=subprocess.run([*command,*args],input=json.dumps(obj) if obj is not None else None,capture_output=True,text=True,timeout=timeout)
    if result.returncode:
        if any(k in result.stderr for k in ['Webdock:','violates PodSecurity','exceeded quota']):raise ExecutionError('admission_rejected')
        raise ExecutionError('execution_failed')
    if len(result.stdout)>2000000:raise ExecutionError('execution_failed')
    return result.stdout


def get(kind,name,namespace=None):
    args=['get',kind,name,'--ignore-not-found','-o','json']
    if namespace:args+=['-n',namespace]
    result=kubectl(*args)
    return json.loads(result) if result.strip() else None


def check_owner(obj,p,app=False):
    actual=obj['metadata'].get('labels',{})
    if any(actual.get(k)!=v for k,v in labels(p,app).items()):raise ExecutionError('ownership_conflict')
    if app and int(obj['metadata'].get('annotations',{}).get('webdock.dev/revision','0'))>p['revision']:raise ExecutionError('ownership_conflict')


def apply(obj,p,default_account=False):
    lease(p)
    existing=get(obj['kind'],obj['metadata']['name'],obj['metadata'].get('namespace'))
    if existing and not default_account:check_owner(existing,p,'webdock.dev/app-id' in obj['metadata'].get('labels',{}))
    kubectl('apply','--server-side','--field-manager=webdock','-f','-',obj=obj)


def prepare_namespace(p,namespace):
    owner=labels(p,False)
    existing=get('namespace',namespace)
    if existing:check_owner(existing,p)
    else:
        lease(p)
        kubectl('create','-f','-',obj={'apiVersion':'v1','kind':'Namespace','metadata':{'name':namespace,'labels':{**owner,'security.webdock.dev/ready':'false','pod-security.kubernetes.io/enforce':'restricted','pod-security.kubernetes.io/enforce-version':'v1.36'}}})
    def resource(kind,name,spec,api='v1'):
        return {'apiVersion':api,'kind':kind,'metadata':{'name':name,'namespace':namespace,'labels':owner},'spec':spec}
    quota=p['quota']
    solver=False
    if existing and existing.get('metadata',{}).get('labels',{}).get('webdock.dev/certificate-budget')=='1':
        from app_templates import certificate_budget
        solver=certificate_budget(p,existing)
    apply(resource('ResourceQuota','webdock-allocation',{'hard':{
      'requests.cpu':str(quota['cpuMillicores']+(25 if solver else 0))+'m','limits.cpu':str(quota['cpuMillicores']+(200 if solver else 0))+'m',
      'requests.memory':str(quota['memoryBytes']+(33554432 if solver else 0)),'limits.memory':str(quota['memoryBytes']+(67108864 if solver else 0)),
      'requests.ephemeral-storage':str(quota['ephemeralBytes']+(8388608 if solver else 0)),'limits.ephemeral-storage':str(quota['ephemeralBytes']+(67108864 if solver else 0)),
      'pods':str(max(10,quota['apps']*20)),'services':str(max(1,quota['apps'])+(1 if solver else 0)),
      'services.nodeports':'0','services.loadbalancers':'0','persistentvolumeclaims':str(quota.get('volumeBytes',0)//67108864),'requests.storage':str(quota.get('volumeBytes',0)),'count/jobs.batch':'0','count/secrets':str(max(20,quota['apps']*4+2)),'count/configmaps':'20'}}),p)
    apply(resource('LimitRange','webdock-defaults',{'limits':[{'type':'Container','defaultRequest':{'cpu':'100m','memory':'128Mi','ephemeral-storage':'128Mi'},'default':{'cpu':'100m','memory':'128Mi','ephemeral-storage':'128Mi'}}]}),p)
    apply({'apiVersion':'v1','kind':'ServiceAccount','metadata':{'name':'default','namespace':namespace,'labels':owner},'automountServiceAccountToken':False},p,default_account=True)
    api='networking.k8s.io/v1'
    policies=[
      ('webdock-deny',{'podSelector':{},'policyTypes':['Ingress','Egress']}),
      ('webdock-internal',{'podSelector':{},'policyTypes':['Ingress','Egress'],'ingress':[{'from':[{'podSelector':{}}]}],'egress':[{'to':[{'podSelector':{}}]}]}),
      ('webdock-dns',{'podSelector':{},'policyTypes':['Egress'],'egress':[{'to':[{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':'kube-system'}},'podSelector':{'matchLabels':{'k8s-app':'kube-dns'}}}],'ports':[{'protocol':'UDP','port':53},{'protocol':'TCP','port':53}]}]}),
      ('webdock-ingress',{'podSelector':{'matchLabels':{'security.webdock.dev/ingress':'true'}},'policyTypes':['Ingress'],'ingress':[{'from':[{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':'kube-system'}},'podSelector':{'matchLabels':{'app.kubernetes.io/name':'traefik'}}}]}]})]
    for name,spec in policies:apply(resource('NetworkPolicy',name,spec,api),p)
    lease(p);kubectl('label','namespace',namespace,'security.webdock.dev/ready=true','--overwrite')


def pods(p):
    return json.loads(kubectl('-n',p['namespace'],'get','pods','-l','webdock.dev/app-id='+p['appID'],'-o','json'))['items']


def proof(p,obj=None,deleted=False,logs=None):
    result={'appID':p['appID'],'revision':p['revision'],'namespace':p['namespace'],'uid':None if obj is None else obj['metadata']['uid'],'ready':True,'deleted':deleted,'replicas':0 if deleted else p['spec']['replicas']}
    if p.get('storageVersion')==1:result['storage']={'volumeBytes':0 if p['action']=='purge-storage' else p['spec']['volumeBytes'],'retained':deleted and p['action']!='purge-storage'}
    if p.get('serviceBindingsVersion')==1:result['serviceBindings']=[{'id':r['id'],'revision':r['revision']} for r in p['serviceBindings']]
    if logs is not None:result['logs']=logs
    return result


def bounded_logs(value):
    return value.encode("utf-8")[:6000].decode("utf-8", errors="ignore")


def guarded_delete(kind,obj,p):
    lease(p);check_owner(obj,p,True)
    group='/apis/apps/v1' if kind=='deployments' else '/api/v1'
    endpoint=group+('/' if kind=='persistentvolumes' else '/namespaces/'+p['namespace']+'/')+kind+'/'+obj['metadata']['name']
    options={'apiVersion':'v1','kind':'DeleteOptions','propagationPolicy':'Foreground','preconditions':{'uid':obj['metadata']['uid'],'resourceVersion':obj['metadata']['resourceVersion']}}
    kubectl('delete','--raw',endpoint,'-f','-',obj=options)


def prepare_storage(p, initial_allocation=False):
    lease(p)
    # Service binding and mail intent can advance revisions before first allocation.
    # Reviewed bootstrap paths opt in; known instances retain the missing-disk guard.
    config,path=storage.ensure({**p,'revision':1} if initial_allocation else p)
    name='app-'+p['appID']+'-data';pvname='wd-'+p['appID']+'-data'
    meta={'labels':labels(p),'annotations':{'webdock.dev/revision':str(p['revision'])}}
    pv={'apiVersion':'v1','kind':'PersistentVolume','metadata':{**meta,'name':pvname},'spec':{'capacity':{'storage':str(p['spec']['volumeBytes'])},'accessModes':['ReadWriteOnce'],'volumeMode':'Filesystem','persistentVolumeReclaimPolicy':'Retain','storageClassName':'','claimRef':{'namespace':p['namespace'],'name':name},'local':{'path':path},'nodeAffinity':{'required':{'nodeSelectorTerms':[{'matchExpressions':[{'key':'kubernetes.io/hostname','operator':'In','values':[config['node']]}]}]}}}}
    pvc={'apiVersion':'v1','kind':'PersistentVolumeClaim','metadata':{**meta,'name':name,'namespace':p['namespace']},'spec':{'accessModes':['ReadWriteOnce'],'volumeMode':'Filesystem','storageClassName':'','volumeName':pvname,'resources':{'requests':{'storage':str(p['spec']['volumeBytes'])}}}}
    for obj in [pv,pvc]:apply(obj,p)


def purge_storage(p):
    # Look at all pods: labels alone do not prove a claim is unused.
    claim='app-'+p['appID']+'-data';pvname='wd-'+p['appID']+'-data'
    allpods=json.loads(kubectl('get','pods','-n',p['namespace'],'-o','json'))['items'] if get('namespace',p['namespace']) else []
    if any(v.get('persistentVolumeClaim',{}).get('claimName')==claim for pod in allpods for v in pod.get('spec',{}).get('volumes',[])):
        raise ExecutionError('ownership_conflict')
    pv=get('persistentvolume',pvname)
    if pv:
        check_owner(pv,p,True)
        ref=pv['spec'].get('claimRef',{})
        if ref.get('namespace')!=p['namespace'] or ref.get('name')!=claim:raise ExecutionError('ownership_conflict')
    pvc=get('persistentvolumeclaim',claim,p['namespace'])
    if pvc:
        if pvc['spec'].get('volumeName')!=pvname:raise ExecutionError('ownership_conflict')
        guarded_delete('persistentvolumeclaims',pvc,p)
    deadline=time.monotonic()+25
    while get('persistentvolumeclaim',claim,p['namespace']):
        lease(p)
        if time.monotonic()>deadline:raise ExecutionError('execution_failed')
        time.sleep(1)
    pv=get('persistentvolume',pvname)
    if pv:guarded_delete('persistentvolumes',pv,p)
    while get('persistentvolume',pvname):
        lease(p)
        if time.monotonic()>deadline:raise ExecutionError('execution_failed')
        time.sleep(1)
    lease(p);storage.purge(p)


def execute(p):
    validate_packet(p)
    namespace=get('namespace',p['namespace'])
    if namespace:check_owner(namespace,p)
    name='app-'+p['appID'];current=get('deployment',name,p['namespace']) if namespace else None
    if current:
        check_owner(current,p,True)
        if p.get('expectedUID') and current['metadata']['uid']!=p['expectedUID']:raise ExecutionError('ownership_conflict')
    if p['action']=='purge-storage':
        if current or (namespace and pods(p)):raise ExecutionError('ownership_conflict')
        purge_storage(p)
        if namespace:prepare_namespace({**p,'quota':p['finalQuota']},p['namespace'])
        return proof(p,deleted=True)
    if p['action']=='logs':
        if not current:raise ExecutionError('ownership_conflict')
        for pod in pods(p):check_owner(pod,p,True)
        data=kubectl('-n',p['namespace'],'logs','deployment/'+name,'--tail=100','--limit-bytes=12000','--all-pods=true','--prefix=false')
        # The control plane redacts current and historical values together before
        # persistence; partial masking here would obscure overlapping old values.
        result=proof(p,current)
        # Base64 bounds JSON expansion without altering values before redaction.
        result['logsBase64']=base64.b64encode(data.encode('utf-8')[:12000].decode('utf-8',errors='ignore').encode('utf-8')).decode('ascii')
        result['logsTruncated']=len(data.encode('utf-8'))>=11996
        result['replicas']=current.get('status',{}).get('availableReplicas',0)
        result['ready']=result['replicas']==p['spec']['replicas']
        return result
    if p['action']=='delete':
        if p.get('storageVersion')==1:storage.retained(p)
        if not namespace:return proof(p,deleted=True)
        service=get('service',name,p['namespace'])
        if service:guarded_delete('services',service,p)
        if current:guarded_delete('deployments',current,p)
        deadline=time.monotonic()+55
        while time.monotonic()<deadline:
            lease(p)
            if not get('deployment',name,p['namespace']) and not get('service',name,p['namespace']) and not pods(p):
                cleanup_environment(p)
                cleanup_service_secrets(p)
                service_bindings.apply_and_verify(p,apply,get,check_owner)
                prepare_namespace({**p,'quota':p['finalQuota']},p['namespace'])
                return proof(p,deleted=True)
            time.sleep(1)
        raise ExecutionError('rollout_timeout')
    deployment,service=manifests(p)
    # A separate empty, equally restricted namespace validates the exact Pod without
    # charging a speculative extra pod against an already full application quota.
    if p['spec']['replicas']:
        validation='wdv-'+p['projectID'];prepare_namespace(p,validation)
        template=copy.deepcopy(deployment['spec']['template'])
        candidate={'apiVersion':'v1','kind':'Pod',**template}
        candidate['metadata'].update(name='check-'+p['appID'],namespace=validation)
        kubectl('create','--dry-run=server','-f','-',obj=candidate)
    prepare_namespace(p,p['namespace'])
    if p['spec'].get('volumeBytes'):prepare_storage(p)
    secret=environment_secret(p)
    active_pods=pods(p)
    for pod in active_pods:check_owner(pod,p,True)
    keep=referenced_environment(([current] if current else [])+active_pods)
    if secret:keep.add(environment_secret_name(p))
    cleanup_environment(p,keep)
    service_keep=service_bindings.references(([current] if current else [])+active_pods)
    if p.get('serviceBindings'):service_keep.add(service_bindings.secret_name(p))
    cleanup_service_secrets(p,service_keep)
    service_bindings.apply_and_verify(p,apply,get,check_owner)
    if secret:
        kubectl('apply','--server-side','--dry-run=server','--field-manager=webdock','-f','-',obj=secret)
        apply(secret,p)
    for obj in [service,deployment]:
        kubectl('apply','--server-side','--dry-run=server','--field-manager=webdock','-f','-',obj=obj)
    for obj in [service,deployment]:apply(obj,p)
    # Recreate termination and image pull precede the 60-second startup probe.
    deadline=time.monotonic()+180
    while time.monotonic()<deadline:
        lease(p);observed=get('deployment',name,p['namespace']);check_owner(observed,p,True)
        status=observed.get('status',{});items=pods(p)
        for pod in items:check_owner(pod,p,True)
        ready=(status.get('observedGeneration',0)>=observed['metadata']['generation'] and status.get('updatedReplicas',0)==p['spec']['replicas'] and status.get('availableReplicas',0)==p['spec']['replicas'] and len(items)==p['spec']['replicas'] and all(not pod['metadata'].get('deletionTimestamp') and any(c['type']=='Ready' and c['status']=='True' for c in pod.get('status',{}).get('conditions',[])) for pod in items))
        if ready:
            cleanup_environment(p,{environment_secret_name(p)} if secret else set())
            cleanup_service_secrets(p,{service_bindings.secret_name(p)} if p.get("serviceBindings") else set())
            prepare_namespace({**p,'quota':p['finalQuota']},p['namespace'])
            return proof(p,observed)
        time.sleep(1)
    raise ExecutionError('rollout_timeout')


def main(packet):
    try:
        # Serializes remote invocations even across interrupted/restarted local bridges.
        with open(os.environ.get('SPITZLI_EXECUTOR_LOCK','/run/lock/webdock-managed-executor.lock'),'w') as lock:
            fcntl.flock(lock,fcntl.LOCK_EX)
            result=execute(packet)
        print(json.dumps({'outcome':'succeeded','proof':result}))
    except Exception as error:
        code=str(error) if isinstance(error,ExecutionError) and str(error) in ['admission_rejected','ownership_conflict','rollout_timeout'] else 'execution_failed'
        print(json.dumps({'outcome':'failed','error':code}))

if __name__=='__main__':
    main(json.load(sys.stdin))
