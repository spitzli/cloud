#!/usr/bin/env python3
"""Fresh bounded TLS source-GET fixture on the explicitly owned BUILD host.

Run locally as the operator. State, credentials and diagnostics stay in an
explicit private directory. No mutable image pulls, source Jobs or RUN changes.
"""
import argparse
import base64
import datetime as dt
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import stat
import subprocess
import sys
import urllib.parse

REPO = Path(__file__).resolve().parents[2]
HOST = '167.86.75.144'
PORT = 19443
MINIO_ID = 'sha256:74d077c5981eeaaf88688deca6dfece7e21d3e1627264d1c2183750380683833'
MINIO_BINARY_SHA256 = '7c8161b2f67871fb0f69bd26c130dcb51e35e29dcea678f31bba76b1686c9bb1'
MC = 'quay.io/minio/mc@sha256:a7fe349ef4bd8521fb8497f55c6042871b2ae640607cf99d9bede5e9bdf11727'
SSH = ['ssh', '-o', 'BatchMode=yes', '-o', 'IdentityAgent=none', '-o', 'IdentitiesOnly=yes', '-i', str(Path.home()/'.ssh/id_ed25519'), 'root@'+HOST]
NETWORK = '/root/webdock-network-profile-v1/deploy/build-cluster'


def private_root(path):
    if not path.is_absolute() or path == Path('/'): raise ValueError('an explicit absolute private state-root is required')
    for parent in [*reversed(path.parents), path]:
        if parent.is_symlink(): raise ValueError('state-root symlink ancestor')
    path.mkdir(mode=0o700, parents=False, exist_ok=True)
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise ValueError('state-root must be an owned private directory')
    return path


def objects(record, admin, certificate, key, ca):
    ns, nonce = record['namespace'], record['nonce']
    def metadata(name): return {'name': name, 'namespace': ns, 'labels': {'webdock.dev/fixture': nonce}}
    return [
        {'apiVersion':'v1','kind':'Namespace','metadata':{'name':ns,'labels':{'webdock.dev/fixture':nonce,'webdock.dev/role':'temporary-staging-infrastructure','pod-security.kubernetes.io/enforce':'privileged'}}},
        {'apiVersion':'v1','kind':'ServiceAccount','metadata':metadata('minio'),'automountServiceAccountToken':False},
        {'apiVersion':'v1','kind':'Secret','metadata':metadata('minio-root'),'type':'Opaque','immutable':True,'stringData':{'MINIO_ROOT_USER':admin['accessKeyId'],'MINIO_ROOT_PASSWORD':admin['secretAccessKey']}},
        {'apiVersion':'v1','kind':'Secret','metadata':metadata('minio-tls'),'type':'kubernetes.io/tls','immutable':True,'data':{'tls.crt':base64.b64encode(certificate+ca).decode(),'tls.key':base64.b64encode(key).decode()}},
        {'apiVersion':'v1','kind':'Pod','metadata':metadata('minio'),'spec':{
            'hostNetwork':True,'dnsPolicy':'Default','nodeSelector':{'kubernetes.io/hostname':'webdock-build-eu-01'},'serviceAccountName':'minio','automountServiceAccountToken':False,'restartPolicy':'Always','terminationGracePeriodSeconds':10,
            'securityContext':{'runAsNonRoot':True,'runAsUser':10001,'runAsGroup':10001,'fsGroup':10001,'seccompProfile':{'type':'RuntimeDefault'}},
            'containers':[{'name':'minio','image':record['image'],'imagePullPolicy':'Never','args':['server','/data','--address',HOST+':'+str(PORT),'--console-address','127.0.0.1:19001','--certs-dir','/certs'],
                'envFrom':[{'secretRef':{'name':'minio-root'}}], 'securityContext':{'privileged':False,'allowPrivilegeEscalation':False,'readOnlyRootFilesystem':True,'capabilities':{'drop':['ALL']}},
                'resources':{'requests':{'cpu':'250m','memory':'512Mi','ephemeral-storage':'32Mi'},'limits':{'cpu':'1','memory':'1536Mi','ephemeral-storage':'64Mi'}},
                'volumeMounts':[{'name':'data','mountPath':'/data'},{'name':'tmp','mountPath':'/tmp'},{'name':'tls','mountPath':'/certs','readOnly':True}],
                'readinessProbe':{'httpGet':{'host':HOST,'port':PORT,'scheme':'HTTPS','path':'/minio/health/ready'},'initialDelaySeconds':2,'periodSeconds':2}}],
            'volumes':[{'name':'data','emptyDir':{'medium':'Memory','sizeLimit':'1Gi'}},{'name':'tmp','emptyDir':{'medium':'Memory','sizeLimit':'32Mi'}},{'name':'tls','secret':{'secretName':'minio-tls','defaultMode':0o440,'items':[{'key':'tls.crt','path':'public.crt'},{'key':'tls.key','path':'private.key'}]}}]}}
    ]


def iam_policies(bucket):
    arn = 'arn:aws:s3:::'+bucket
    source = {'Version':'2012-10-17','Statement':[{'Effect':'Allow','Action':['s3:GetObject'],'Resource':[arn+'/worker-staging/*']},{'Effect':'Allow','Action':['s3:GetBucketLocation'],'Resource':[arn]}]}
    operator = {'Version':'2012-10-17','Statement':[{'Effect':'Allow','Action':['s3:GetObject','s3:PutObject','s3:DeleteObject','s3:AbortMultipartUpload','s3:ListMultipartUploadParts'],'Resource':[arn+'/worker-staging/*']},{'Effect':'Allow','Action':['s3:GetBucketLocation'],'Resource':[arn]},{'Effect':'Allow','Action':['s3:ListBucket'],'Resource':[arn],'Condition':{'StringLike':{'s3:prefix':['worker-staging','worker-staging/*']}}}]}
    return operator, source


def source_egress(nonce):
    if not re.fullmatch('[a-f0-9]{20}',nonce): raise ValueError('invalid fixture nonce')
    return {'apiVersion':'networking.k8s.io/v1','kind':'NetworkPolicy','metadata':{'name':'owned-source-get-'+nonce,'namespace':'webdock-build-qualification','labels':{'webdock.dev/fixture':nonce}},'spec':{'podSelector':{'matchLabels':{'webdock.dev/component':'build-job'}},'policyTypes':['Egress'],'egress':[{'to':[{'ipBlock':{'cidr':HOST+'/32'}}],'ports':[{'protocol':'TCP','port':PORT}]}]}}


def upload_egress(nonce):
    policy=source_egress(nonce)
    policy['metadata']['name']='webdock-upload-qualification'
    policy['spec']['egress'][0]['ports'][0]['port']=19444
    return policy


def transfer_policy(ca):
    return {'allowedOrigins':['https://167.86.75.144:19443','https://167.86.75.144:19444'],'maxSourceBytes':128*1024*1024+50000*1024+10240,'maxResultBytes':512*1024*1024+188416,'timeoutSeconds':60,'certificateAuthorityPEM':ca}


def dispatcher_config(ca, token):
    return {'apiVersion':'v1','kind':'Config','clusters':[{'name':'owned-build','cluster':{'server':'https://127.0.0.1:16443','certificate-authority-data':base64.b64encode(ca).decode()}}], 'users':[{'name':'owned-build-dispatcher','user':{'token':token}}], 'contexts':[{'name':'owned-build-live','context':{'cluster':'owned-build','user':'owned-build-dispatcher','namespace':'webdock-build-qualification'}}],'current-context':'owned-build-live'}


def namespace_delete(record, observed):
    metadata=observed['metadata']
    if (metadata['uid'] != record['namespaceUID'] or metadata['name'] != record['namespace'] or metadata.get('labels',{}).get('webdock.dev/fixture') != record['nonce']):
        raise ValueError('fixture namespace identity changed; refusing deletion')
    return {'apiVersion':'v1','kind':'DeleteOptions','propagationPolicy':'Foreground','preconditions':{'uid':record['namespaceUID']}}


class Fixture:
    def __init__(self, root): self.root=private_root(root)

    def write(self,name,value):
        data=value if isinstance(value,bytes) else json.dumps(value,indent=2).encode()
        temporary=self.root/(name+'.tmp-'+secrets.token_hex(4))
        with temporary.open('xb') as output:
            os.chmod(temporary,0o600);output.write(data);output.flush();os.fsync(output.fileno())
        temporary.replace(self.root/name)

    def read(self,name):
        descriptor=os.open(self.root/name,os.O_RDONLY|os.O_NOFOLLOW)
        with os.fdopen(descriptor,'rb') as stream:
            info=os.fstat(stream.fileno())
            if not stat.S_ISREG(info.st_mode) or info.st_uid!=os.getuid() or info.st_nlink!=1 or info.st_mode&0o077 or info.st_size>2*1024*1024:
                raise ValueError('unsafe private fixture file')
            return json.load(stream)

    def run(self,args,data=None,env=None,allowed_failure=False,timeout=180):
        result=subprocess.run(args,input=data,capture_output=True,env=env,timeout=timeout)
        if result.returncode and not allowed_failure:
            self.write('last-operation-error.bin',(result.stdout+b'\n'+result.stderr)[-1024*1024:])
            raise RuntimeError('fixture operation failed; bounded private diagnostics retained')
        return None if result.returncode else result.stdout

    def remote(self,command,data=None,**kwargs): return self.run([*SSH,command],data,**kwargs)
    def kubectl(self,*args,data=None,**kwargs): return self.remote(shlex.join(['/usr/local/bin/k3s','kubectl','--kubeconfig=/etc/rancher/k3s/k3s.yaml',*args]),data,**kwargs)
    def apply(self,value): return self.kubectl('apply','-f','-',data=json.dumps(value).encode())

    def certificates(self,record):
        if (self.root/'ca.key').exists(): return
        def execute(*args): self.run(['openssl',*args])
        execute('req','-new','-newkey','rsa:3072','-x509','-sha256','-nodes','-days','2','-subj','/CN=Webdock fresh source '+record['nonce'],'-addext','basicConstraints=critical,CA:TRUE,pathlen:0','-addext','keyUsage=critical,keyCertSign,cRLSign','-keyout',str(self.root/'ca.key'),'-out',str(self.root/'ca.crt'))
        execute('req','-new','-newkey','rsa:2048','-sha256','-nodes','-subj','/CN='+HOST,'-keyout',str(self.root/'server.key'),'-out',str(self.root/'server.csr'))
        self.write('server.ext',b'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=IP:167.86.75.144,IP:127.0.0.1,IP:10.83.0.1\n')
        execute('x509','-req','-sha256','-days','2','-in',str(self.root/'server.csr'),'-CA',str(self.root/'ca.crt'),'-CAkey',str(self.root/'ca.key'),'-CAcreateserial','-extfile',str(self.root/'server.ext'),'-out',str(self.root/'server.crt'))
        execute('verify','-CAfile',str(self.root/'ca.crt'),'-verify_ip',HOST,str(self.root/'server.crt'))
        for path in self.root.iterdir():
            if path.is_file():path.chmod(0o600)

    def import_image(self,record):
        self.run(['docker','image','tag',MINIO_ID,record['localImage']])
        saved=subprocess.Popen(['docker','save',record['localImage']],stdout=subprocess.PIPE,stderr=subprocess.DEVNULL)
        uploaded=subprocess.run([*SSH,'k3s ctr images import -'],stdin=saved.stdout,capture_output=True,timeout=600)
        saved.stdout.close()
        if uploaded.returncode or saved.wait():
            self.write('image-import-error.bin',uploaded.stderr[-1024*1024:]);raise RuntimeError('owned immutable image import failed')
        lines=self.remote('k3s ctr images ls').decode().splitlines()
        matches=[line.split()[2] for line in lines if line.split()[0]==record['imageTag']]
        if len(matches)!=1 or not re.fullmatch('sha256:[a-f0-9]{64}',matches[0]): raise ValueError('imported image identity missing')
        record['image']=record['imageTag'].split(':')[0]+'@'+matches[0]
        self.remote(shlex.join(['k3s','ctr','images','tag',record['imageTag'],record['image']]))
        self.write('ownership.json',record)

    def mc(self,record,identity='admin'):
        creds=self.read(identity+'.json')
        config=self.root/('mc-'+identity);ca=config/'certs/CAs';ca.mkdir(mode=0o700,parents=True,exist_ok=True)
        (ca/'fixture.crt').write_bytes((self.root/'ca.crt').read_bytes())
        encoded=urllib.parse.quote(creds['accessKeyId'],safe='')+':'+urllib.parse.quote(creds['secretAccessKey'],safe='')
        env={**os.environ,'MC_HOST_fixture':'https://'+encoded+'@'+HOST+':'+str(PORT)}
        def invoke(*args,data=None,allowed_failure=False):
            return self.run(['docker','run','--rm','--pull=never','--network','host','--user',str(os.getuid())+':'+str(os.getgid()),'-i','--env','MC_HOST_fixture','--env','MC_CONFIG_DIR=/cfg','-v',str(config)+':/cfg',MC,*args],data,env,allowed_failure)
        return invoke

    def prepare(self,owner):
        if not re.fullmatch('[a-z0-9][a-z0-9-]{0,79}',owner):raise ValueError('invalid fixture owner')
        self.remote('python3 -B '+NETWORK+'/trusted-system-network.py --verify --on-build-host')
        if (self.root/'ownership.json').exists():
            record=self.read('ownership.json')
            if record['cleaned'] or record['ready'] or record['owner']!=owner:raise ValueError('do not revive cleaned/ready/foreign authority; use a fresh state-root')
        else:
            image=self.run(['docker','image','inspect','webdock-dev-minio:local','--format','{{.Id}}']).decode().strip()
            if image!=MINIO_ID:raise ValueError('cached MinIO image changed; explicit pin review required')
            version=self.run(['docker','run','--rm','--pull=never','--network=none',MINIO_ID,'--version']).decode()
            binary=self.run(['docker','run','--rm','--pull=never','--network=none','--entrypoint','sha256sum',MINIO_ID,'/usr/local/bin/minio']).decode().split()[0]
            if binary!=MINIO_BINARY_SHA256 or '9e49d5e7a648f00e26f2246f4dc28e6b07f8c84a' not in version:raise ValueError('cached MinIO source/binary version mismatch')
            self.run(['docker','image','inspect',MC])
            nonce=secrets.token_hex(10)
            record={'version':1,'owner':owner,'nonce':nonce,'host':HOST,'port':PORT,'namespace':'wd-stage-fixture-'+nonce,'namespaceUID':None,'podUID':None,'bucket':'wd-stage-'+nonce,'policy':'wd-stage-'+nonce,'imageTag':'docker.io/library/webdock-fixture-minio-'+nonce+':owned','localImage':'webdock-fixture-minio-'+nonce+':owned','sourceImageID':image,'sourceDockerfileSHA256':hashlib.sha256((REPO/'deploy/dev/minio/Dockerfile').read_bytes()).hexdigest(),'minioBinarySHA256':binary,'minioVersion':version,'mcImage':MC,'createdAt':dt.datetime.now(dt.timezone.utc).isoformat(),'ready':False,'cleaned':False}
            self.write('ownership.json',record)
        self.certificates(record)
        if 'image' not in record:self.import_image(record)
        opened=self.remote(shlex.join(['python3','-B',NETWORK+'/qualification-ingress.py','--open','--on-build-host','--owner',owner,'--ttl-seconds','3600']))
        record['ingressLease']=json.loads(opened);self.write('ownership.json',record)
        for name,prefix in [('admin','wdroot'),('credentials','wdop'),('source-read-credentials','wdget')]:
            if not (self.root/(name+'.json')).exists():self.write(name+'.json',{'accessKeyId':prefix+record['nonce'][:16],'secretAccessKey':secrets.token_hex(32)})
        docs=objects(record,self.read('admin.json'),(self.root/'server.crt').read_bytes(),(self.root/'server.key').read_bytes(),(self.root/'ca.crt').read_bytes())
        if record['namespaceUID'] is None:
            self.kubectl('create','-f','-',data=json.dumps(docs[0]).encode())
            current=json.loads(self.kubectl('get','namespace',record['namespace'],'-o','json'))
            record['namespaceUID']=current['metadata']['uid'];self.write('ownership.json',record)
        else:namespace_delete(record,json.loads(self.kubectl('get','namespace',record['namespace'],'-o','json')))
        self.apply({'apiVersion':'v1','kind':'List','items':docs[1:]})
        self.kubectl('wait','-n',record['namespace'],'--for=condition=Ready','pod/minio','--timeout=120s',timeout=150)
        pod=json.loads(self.kubectl('get','pod','minio','-n',record['namespace'],'-o','json'))
        record['podUID']=pod['metadata']['uid'];self.write('ownership.json',record)
        admin=self.mc(record)
        admin('mb','--ignore-existing','fixture/'+record['bucket'])
        for name,policy in zip(['credentials','source-read-credentials'],iam_policies(record['bucket'])):
            credentials=self.read(name+'.json');policy_name=record['policy']+('-get' if name.startswith('source') else '-operator')
            admin('admin','user','add','fixture',credentials['accessKeyId'],credentials['secretAccessKey'])
            admin('admin','policy','create','fixture',policy_name,'/dev/stdin',data=json.dumps(policy).encode())
            admin('admin','policy','attach','fixture',policy_name,'--user',credentials['accessKeyId'])
            self.write(name+'-policy.json',policy)
        scoped=self.mc(record,'credentials');read_only=self.mc(record,'source-read-credentials')
        if scoped('admin','info','fixture',allowed_failure=True) is not None:raise ValueError('operator scoped IAM unexpectedly administers MinIO')
        if scoped('pipe','fixture/'+record['bucket']+'/outside-prefix',data=b'deny',allowed_failure=True) is not None:raise ValueError('scoped IAM escaped prefix')
        target='fixture/'+record['bucket']+'/worker-staging/source-get-proof'
        body=('fresh-source-'+record['nonce']).encode();scoped('pipe',target,data=body)
        if read_only('pipe',target,data=b'deny',allowed_failure=True) is not None:raise ValueError('source GET identity unexpectedly wrote')
        shared=json.loads(read_only('share','download','--json','--expire','15m',target).decode().splitlines()[-1])
        self.write('source-get-proof-private.json',shared)
        url=shared.get('share') or shared.get('url')
        if not isinstance(url,str):raise ValueError('mc did not return a presigned GET URL')
        def curl(*args):return self.run(['curl','--noproxy','*','--silent','--show-error','--max-time','15','--cacert',str(self.root/'ca.crt'),*args])
        if curl('--fail',url)!=body:raise ValueError('presigned source GET proof differs')
        denied=curl('--request','PUT','--data-binary','must-deny','--output','/dev/null','--write-out','%{http_code}',url)
        if denied!=b'403':raise ValueError('presigned GET authority accepted a PUT')
        scoped('rm',target)
        self.install_egress(record,source_egress(record['nonce']),'egressPolicy')
        self.install_egress(record,upload_egress(record['nonce']),'uploadEgressPolicy')
        endpoint='https://'+HOST+':'+str(PORT)
        self.write('s3-config.json',{'id':'owned-stage-'+record['nonce'][:12],'endpoint':endpoint,'region':'us-east-1','bucket':record['bucket'],'namespace':'worker-staging'})
        self.write('transfer-policy.json',transfer_policy((self.root/'ca.crt').read_text()))
        record.update({'ready':True,'readyAt':dt.datetime.now(dt.timezone.utc).isoformat(),'caSHA256':hashlib.sha256((self.root/'ca.crt').read_bytes()).hexdigest(),'sourceGetProof':{'TLSVerified':True,'signedGet':True,'signedPutDenied':True,'sourceIAMWriteDenied':True,'operatorAdminDenied':True,'outsidePrefixDenied':True},'dataBudget':'1Gi actual memory tmpfs; Pod memory cap1536Mi','sourcePodCredentials':'none; only per-attempt signed GET URL/CA','fixtureSourceSHA256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest()})
        self.write('ownership.json',record)
        self.status()

    def install_egress(self,record,policy,key):
        metadata=policy['metadata']
        current=self.kubectl('get','networkpolicy',metadata['name'],'-n',metadata['namespace'],'-o','json',allowed_failure=True)
        if current is not None:
            observed=json.loads(current)
            if (observed['metadata'].get('labels',{}).get('webdock.dev/fixture')!=record['nonce'] or record.get(key,{}).get('uid')!=observed['metadata']['uid']):
                raise ValueError('refusing to adopt a preexisting/foreign data-plane policy')
            return
        self.kubectl('create','-f','-',data=json.dumps(policy).encode())
        observed=json.loads(self.kubectl('get','networkpolicy',metadata['name'],'-n',metadata['namespace'],'-o','json'))
        record[key]={'namespace':metadata['namespace'],'name':metadata['name'],'uid':observed['metadata']['uid']}
        self.write('ownership.json',record)

    def dispatcher(self,owner):
        record=self.read('ownership.json')
        if owner!=record['owner'] or not record['ready'] or record['cleaned']:raise ValueError('requires this live fixture owner')
        if (self.root/'dispatcher.kubeconfig.json').exists():raise ValueError('dispatcher token already issued; do not silently replace authority')
        identity='system:serviceaccount:webdock-build-qualification:webdock-build-dispatcher'
        checks=[('create','jobs','webdock-build-qualification',True),('patch','jobs','webdock-build-qualification',True),('create','secrets','webdock-build-qualification',True),('list','secrets','webdock-build-qualification',False),('create','pods/exec','webdock-build-qualification',False),('list','nodes','webdock-build-qualification',False),('create','rolebindings','webdock-build-qualification',False),('create','configmaps','webdock-build-storage-requests',True),('get','configmaps','webdock-build-storage-receipts',True),('create','secrets','kube-system',False)]
        results=[]
        for verb,resource,namespace,expected in checks:
            result=self.kubectl('auth','can-i',verb,resource,'-n',namespace,'--as='+identity,allowed_failure=True)
            allowed=result is not None and result.strip()==b'yes'
            if allowed!=expected:raise ValueError('dispatcher least-privilege binding mismatch: '+verb+' '+resource+' '+namespace)
            results.append({'verb':verb,'resource':resource,'namespace':namespace,'allowed':allowed})
        ca_encoded=self.kubectl('config','view','--raw','-o','jsonpath={.clusters[0].cluster.certificate-authority-data}').strip()
        ca=base64.b64decode(ca_encoded,validate=True)
        if not ca.startswith(b'-----BEGIN CERTIFICATE-----'):raise ValueError('expected only a public build-cluster CA')
        account=json.loads(self.kubectl('get','serviceaccount','webdock-build-dispatcher','-n','webdock-build-qualification','-o','json'))
        token=self.kubectl('create','token','webdock-build-dispatcher','-n','webdock-build-qualification','--duration=1h').decode().strip()
        encoded=token.split('.')[1];claims=json.loads(base64.urlsafe_b64decode(encoded+'='*((4-len(encoded)%4)%4)))
        if claims['sub']!=identity or not 0<claims['exp']-claims['iat']<=3600:raise ValueError('unexpected scoped token identity/lifetime')
        self.write('dispatcher.kubeconfig.json',dispatcher_config(ca,token));self.write('build-ca.crt',ca)
        metadata={'identity':identity,'serviceAccountUID':account['metadata']['uid'],'issuedAt':claims['iat'],'expiresAt':claims['exp'],'context':'owned-build-live','server':'https://127.0.0.1:16443','buildCASHA256':hashlib.sha256(ca).hexdigest(),'authorizationChecks':results,'cleanup':'delete local kubeconfig; bound to existing scoped ServiceAccount and expires within1h; do not delete shared ServiceAccount'}
        self.write('dispatcher-authority.json',metadata)
        record['dispatcherAuthority']={k:v for k,v in metadata.items() if k!='authorizationChecks'};self.write('ownership.json',record)
        print(json.dumps({'kubeconfig':str(self.root/'dispatcher.kubeconfig.json'),'publicCA':str(self.root/'build-ca.crt'),'metadata':str(self.root/'dispatcher-authority.json'),'buildCASHA256':metadata['buildCASHA256'],'expiresAt':metadata['expiresAt'],'context':metadata['context'],'server':metadata['server']},indent=2))

    def status(self):
        record=self.read('ownership.json')
        print(json.dumps({'owner':record['owner'],'nonce':record['nonce'],'namespace':record['namespace'],'namespaceUID':record['namespaceUID'],'ready':record['ready'],'cleaned':record['cleaned'],'endpoint':'https://'+HOST+':'+str(PORT),'paths':{name:str(self.root/name) for name in ['ownership.json','s3-config.json','credentials.json','source-read-credentials.json','ca.crt','server.crt','server.key','transfer-policy.json']}},indent=2))

    def cleanup(self,owner):
        record=self.read('ownership.json')
        if owner!=record['owner']:raise ValueError('fixture belongs to another owner')
        if record['cleaned']:self.status();return
        for key in ['egressPolicy','uploadEgressPolicy']:
            policy=record.get(key)
            if policy:
                data=self.kubectl('get','networkpolicy',policy['name'],'-n',policy['namespace'],'-o','json',allowed_failure=True)
                if data:
                    value=json.loads(data)
                    if value['metadata']['uid']!=policy['uid'] or value['metadata'].get('labels',{}).get('webdock.dev/fixture')!=record['nonce']:raise ValueError('source egress policy replaced; refusing deletion')
                    delete={'apiVersion':'v1','kind':'DeleteOptions','preconditions':{'uid':policy['uid']}}
                    self.kubectl('delete','--raw=/apis/networking.k8s.io/v1/namespaces/'+policy['namespace']+'/networkpolicies/'+policy['name'],'-f','-',data=json.dumps(delete).encode())
        data=self.kubectl('get','namespace',record['namespace'],'-o','json',allowed_failure=True)
        if data:
            delete=namespace_delete(record,json.loads(data))
            self.kubectl('delete','--raw=/api/v1/namespaces/'+record['namespace'],'-f','-',data=json.dumps(delete).encode())
            self.kubectl('wait','--for=delete','namespace/'+record['namespace'],'--timeout=120s',timeout=150)
        self.remote(shlex.join(['python3','-B',NETWORK+'/qualification-ingress.py','--close','--on-build-host','--owner',owner]))
        for name in [record.get('image'),record['imageTag']]:
            if name:self.remote(shlex.join(['k3s','ctr','images','rm',name]),allowed_failure=True)
        self.run(['docker','image','rm',record['localImage']],allowed_failure=True)
        for name in ['admin.json','credentials.json','source-read-credentials.json','source-get-proof-private.json','ca.key','server.key','server.csr','server.crt','server.ext','ca.srl','dispatcher.kubeconfig.json']:
            (self.root/name).unlink(missing_ok=True)
        for name in ['mc-admin','mc-credentials','mc-source-read-credentials']:
            directory=self.root/name
            if directory.exists():
                if directory.is_symlink():raise ValueError('refusing linked MC authority cache')
                # Older mc runs used container-root ownership; mount only this exact
                # private cache and remove its history without network or image pulls.
                self.run(['docker','run','--rm','--pull=never','--network=none','--read-only','--user','0:0','--entrypoint','/bin/sh','--mount','type=bind,src='+str(directory)+',dst=/cfg',MINIO_ID,'-ec','find /cfg -mindepth 1 -delete'])
                directory.rmdir()
        record.update({'cleaned':True,'ready':False,'cleanedAt':dt.datetime.now(dt.timezone.utc).isoformat()});self.write('ownership.json',record)
        self.status()


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action',choices=['prepare','status','cleanup','dispatcher'])
    parser.add_argument('--state-root',type=Path,required=True)
    parser.add_argument('--on-owned-build-host',action='store_true',required=True)
    parser.add_argument('--owner')
    args=parser.parse_args()
    os.umask(0o077)
    fixture=Fixture(args.state_root)
    try:
        if args.action=='prepare':
            if not args.owner:parser.error('--owner is required')
            fixture.prepare(args.owner)
        elif args.action=='dispatcher':
            if not args.owner:parser.error('--owner is required')
            fixture.dispatcher(args.owner)
        elif args.action=='cleanup':
            if not args.owner:parser.error('--owner is required')
            fixture.cleanup(args.owner)
        else:fixture.status()
    except Exception as error:
        fixture.write('private-error.json',{'type':type(error).__name__,'message':str(error)})
        print('Owned staging operation incomplete; private diagnostics: '+str(args.state_root),file=sys.stderr)
        return 1
    return 0


if __name__=='__main__':sys.exit(main())
