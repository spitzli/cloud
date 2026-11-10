"""Operator-approved application initialization and exact-origin publication.

No commands, domains or image tags are accepted from plugins. A missing Job after
an initialization intent is an uncertain mutation, never permission to rerun it.
"""
import http.client
import ipaddress
import json
import os
from pathlib import Path
import re
import socket
import ssl
from urllib.parse import urlsplit
import executor as e

COMMAND = ['node', '/app/migrate.mjs']
FIELDS = {'version','templateID','operationID','generation','leaseToken','leaseUntil','phase','projectID','clusterID','appID','instanceID','namespace','revision','spec','quota','environment','backendID','origin','pinnedAddresses','jobUID','intentRecorded','storageState','certificateUID','ingressUID'}


def approved(template_id):
    path=Path(os.environ.get('SPITZLI_APP_TEMPLATES','/etc/webdock/app-templates.json'))
    st=path.stat()
    if st.st_uid!=0 or st.st_mode & 0o022:raise ValueError('Template approvals must be root-owned and not writable by other users')
    return json.loads(path.read_text())[template_id]


def validate(p,config):
    if set(p)!=FIELDS or p['version']!=1 or p['templateID']!='managed-cms':raise ValueError('Invalid approved template packet')
    for key in ['operationID','projectID','clusterID','appID','instanceID','backendID']:
        if not re.fullmatch(r'[1-9][0-9]{0,18}',p[key]):raise ValueError('Invalid resource identity')
    if p['namespace']!='wd-'+p['projectID'] or p['phase'] not in ['migration','publication']:raise ValueError('Invalid template scope')
    if type(p['generation']) is not int or p['generation']<1 or type(p['revision']) is not int or p['revision']<1:raise ValueError('Invalid generation')
    if not re.fullmatch(r'[A-Za-z0-9_-]{43}',p['leaseToken']):raise ValueError('Invalid lease')
    if not re.fullmatch(r'[a-z0-9.-]+/[a-zA-Z0-9_./-]+@sha256:[a-f0-9]{64}',config['image']) or p['spec']['image']!=config['image']:raise ValueError('Unapproved image')
    if not re.fullmatch(r'[a-z0-9-]+(?:\.[a-z0-9-]+)+',config['domain']):raise ValueError('Invalid approved domain')
    if p['origin']!='https://'+p['instanceID']+'.'+config['domain'] or sorted(p['pinnedAddresses'])!=sorted(config['addresses']):raise ValueError('Unapproved origin')
    if not 1<=len(config['addresses'])<=8:raise ValueError('Invalid route addresses')
    for address in config['addresses']:
        ip=ipaddress.ip_address(address)
        if ip.is_loopback or ip.is_unspecified or ip.is_multicast:raise ValueError('Invalid route destination')
    for key in ['certificateIssuer','ingressClass']:
        if not re.fullmatch(r'[a-z0-9][a-z0-9-]{0,62}',config[key]):raise ValueError('Invalid ingress configuration')
    env=p['environment']
    if not isinstance(env,dict) or len(json.dumps(env))>32000:raise ValueError('Invalid environment')
    allowed={'SPITZLI_AUTH_ISSUER','SPITZLI_SSO_APP_ORIGIN','SPITZLI_SSO_CLIENT_ID','SPITZLI_SSO_CLIENT_SECRET','SPITZLI_SSO_COOKIE_SECRET','SPITZLI_SSO_ENFORCE','CMS_INSTANCE_ID','CMS_PROJECT_ID','CMS_ORIGIN','CMS_SITE_NAME','PAYLOAD_SECRET','CMS_BRIDGE_SECRET','CMS_MIGRATING','CMS_MIGRATION_DATABASE_URL','DATABASE_CA_CERT','CMS_BOOTSTRAP_SUBJECT','CMS_BOOTSTRAP_ROLE'}
    if set(env)-allowed or env.get('CMS_INSTANCE_ID')!=p['instanceID'] or env.get('CMS_PROJECT_ID')!=p['projectID'] or env.get('CMS_ORIGIN')!=p['origin']:raise ValueError('Invalid environment scope')
    if p['phase']=='migration' and (env.get('CMS_MIGRATING')!='true' or not env.get('CMS_MIGRATION_DATABASE_URL')):raise ValueError('Missing migration credential')
    for key in ['certificateUID','ingressUID']:
        if p[key] is not None and (not isinstance(p[key],str) or not 1<=len(p[key])<=128):raise ValueError('Invalid publication UID')
    if p['jobUID'] is not None and (not isinstance(p['jobUID'],str) or not 1<=len(p['jobUID'])<=128):raise ValueError('Invalid job UID')
    if type(p['intentRecorded']) is not bool:raise ValueError('Invalid initialization intent')
    if p['storageState'] not in ['unallocated','allocating','ready','unknown']:raise ValueError('Invalid storage allocation state')
    e.validate_packet({**p,'action':'apply','storageVersion':1,'environmentVersion':1})
    issuer=urlsplit(config['authIssuer'])
    if issuer.scheme!='https' or issuer.username or issuer.password or issuer.path!='/api/auth' or issuer.query or issuer.fragment:raise ValueError('Invalid Auth issuer')
    if env.get('SPITZLI_AUTH_ISSUER')!=config['authIssuer']:raise ValueError('Unapproved Auth issuer')
    if not 1<=len(config['authAddresses'])<=8:raise ValueError('Invalid Auth addresses')
    for address in config['authAddresses']:
        ip=ipaddress.ip_address(address)
        if ip.is_loopback or ip.is_unspecified or ip.is_multicast:raise ValueError('Invalid Auth destination')
    e.lease(p)


def metadata(p,name):
    return {'name':name,'namespace':p['namespace'],'labels':{**e.labels(p),'webdock.dev/provisioning-id':p['operationID']},
            'annotations':{'webdock.dev/revision':str(p['revision']),'webdock.dev/template-id':p['templateID']}}


def owned(obj,p):
    e.check_owner(obj,p,True)
    if obj['metadata'].get('labels',{}).get('webdock.dev/provisioning-id')!=p['operationID']:raise ValueError('Template ownership changed')


def migration_manifests(p):
    name='initialize-'+p['operationID'];spec=p['spec']
    resource={'cpu':str(spec['cpuMillicores'])+'m','memory':str(spec['memoryBytes']),'ephemeral-storage':str(spec['ephemeralBytes'])}
    meta=metadata(p,name)
    secret={'apiVersion':'v1','kind':'Secret','metadata':meta,'immutable':True,'type':'Opaque','stringData':p['environment']}
    probe={'exec':{'command':['node','-e','process.exit(0)']},'periodSeconds':10,'timeoutSeconds':3,'failureThreshold':3}
    pod={'automountServiceAccountToken':False,'restartPolicy':'Never','terminationGracePeriodSeconds':10,
      'securityContext':{'runAsNonRoot':True,'runAsUser':65532,'runAsGroup':65532,'fsGroup':65532,'seccompProfile':{'type':'RuntimeDefault'}},
      'containers':[{'name':'initialize','image':spec['image'],'imagePullPolicy':'IfNotPresent','command':COMMAND,
        'securityContext':{'readOnlyRootFilesystem':True,'allowPrivilegeEscalation':False,'capabilities':{'drop':['ALL']}},
        'resources':{'requests':resource,'limits':resource},'startupProbe':probe,'readinessProbe':probe,'livenessProbe':probe,
        'env':[{'name':key,'valueFrom':{'secretKeyRef':{'name':name,'key':key}}} for key in p['environment']],
        'volumeMounts':[{'name':'tmp','mountPath':'/tmp'},{'name':'data','mountPath':'/data'}]}],
      'volumes':[{'name':'tmp','emptyDir':{'sizeLimit':str(spec['ephemeralBytes'])}},{'name':'data','persistentVolumeClaim':{'claimName':'app-'+p['appID']+'-data'}}]}
    job={'apiVersion':'batch/v1','kind':'Job','metadata':meta,'spec':{'parallelism':1,'completions':1,'backoffLimit':0,'activeDeadlineSeconds':240,
      'template':{'metadata':{'labels':meta['labels']},'spec':pod}}}
    return secret,job


def prepare_initial_storage(p,checkpoint):
    state=p['storageState']
    existing=e.get('persistentvolume','wd-'+p['appID']+'-data') or e.get('persistentvolumeclaim','app-'+p['appID']+'-data',p['namespace'])
    fresh=state=='unallocated' and not existing
    if fresh:
        # Binding bumps application revisions before the first disk exists. Record
        # allocation intent before opting into the storage service's revision-1 path.
        checkpoint(storageState='allocating')
    else:checkpoint()
    e.prepare_storage(p if fresh else {**p,'revision':max(2,p['revision'])},initial_allocation=fresh)
    checkpoint(storageState='ready')


def migrate(p,checkpoint):
    name='initialize-'+p['operationID'];job=e.get('job',name,p['namespace'])
    if job:
        owned(job,p)
        if p['jobUID'] and p['jobUID']!=job['metadata']['uid']:raise ValueError('Initialization job UID changed')
        checkpoint(jobUID=job['metadata']['uid'])
        if job.get('status',{}).get('failed') or any(c.get('type')=='Failed' and c.get('status')=='True' for c in job.get('status',{}).get('conditions',[])):
            raise ValueError('Initialization failed; database and media are retained')
        if job.get('status',{}).get('succeeded')!=1:return None
        # Persist success before removing the only execution evidence. No rerun after a crash.
        checkpoint(jobUID=job['metadata']['uid'],migrationSucceeded=True)
        secret=e.get('secret',name,p['namespace'])
        if secret:
            owned(secret,p);checkpoint();e.guarded_delete('secrets',secret,p)
        return {'phase':'migration','jobUID':job['metadata']['uid'],'instanceID':p['instanceID']}
    if p['intentRecorded'] or p['jobUID']:raise ValueError('Missing initialization job requires reconciliation')
    current=e.get('deployment','app-'+p['appID'],p['namespace'])
    if current:raise ValueError('Initialization cannot run alongside a deployed application')
    checkpoint();e.prepare_namespace(p,p['namespace'])
    prepare_initial_storage(p,checkpoint)
    # Only this reviewed initialization path permits one bounded Job. Ordinary app
    # admission remains jobs=0 and still applies Pod Security and all three probes.
    quota=e.get('resourcequota','webdock-allocation',p['namespace']);e.check_owner(quota,p)
    quota['spec']['hard']['count/jobs.batch']='1'
    checkpoint();e.apply(quota,p)
    policy={'apiVersion':'networking.k8s.io/v1','kind':'NetworkPolicy','metadata':metadata(p,name),'spec':{
      'podSelector':{'matchLabels':{'webdock.dev/provisioning-id':p['operationID']}},'policyTypes':['Egress'],'egress':[{
        'to':[{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':'wd-db-'+p['backendID']}},'podSelector':{'matchLabels':{'cnpg.io/cluster':'postgres'}}}],
        'ports':[{'protocol':'TCP','port':5432}]}]}}
    checkpoint();e.apply(policy,p)
    secret,manifest=migration_manifests(p)
    checkpoint();e.apply(secret,p)
    checkpoint(intentRecorded=True)
    # Create (never apply) the single migration Job. A lost response is observed by
    # its immutable name and ownership; absence after intent needs operator review.
    e.lease(p);e.kubectl('create','-f','-',obj=manifest)
    observed=e.get('job',name,p['namespace']);owned(observed,p)
    checkpoint(jobUID=observed['metadata']['uid'])
    return None


def publication_manifest(p,config):
    host=p['origin'][8:]
    return {'apiVersion':'networking.k8s.io/v1','kind':'Ingress','metadata':metadata(p,'app-'+p['appID']),
      'spec':{'ingressClassName':config['ingressClass'],'tls':[{'hosts':[host],'secretName':'app-'+p['appID']+'-tls'}],
        'rules':[{'host':host,'http':{'paths':[{'path':'/','pathType':'Prefix','backend':{'service':{'name':'app-'+p['appID'],'port':{'number':p['spec']['port']}}}}]}}]}}


class PublicationPending(Exception):
    pass


def tls_health(p):
    host=p['origin'][8:];observed={item[4][0] for item in socket.getaddrinfo(host,443,type=socket.SOCK_STREAM)}
    if not observed or observed!=set(p['pinnedAddresses']):raise ValueError('Published DNS differs from approved addresses')
    for address in p['pinnedAddresses']:
        conn=http.client.HTTPSConnection(host,443,timeout=10,context=ssl.create_default_context())
        try:
            # Pin the TCP destination while preserving TLS hostname verification/SNI.
            conn.sock=conn._context.wrap_socket(socket.create_connection((address,443),10),server_hostname=host)
            conn.request('GET','/api/health',headers={'Host':host,'Accept':'application/json'})
            response=conn.getresponse();body=response.read(8193)
            if len(body)>8192:raise ValueError('Published health response exceeds limit')
            if response.status in [404,408,425,429,500,502,503,504]:raise PublicationPending('HTTPS route is converging')
            if response.status!=200:raise ValueError('Published health check failed')
            payload=json.loads(body)
            if payload.get('status')!='ok' or payload.get('instance')!=p['instanceID'] or payload.get('project')!=p['projectID'] or payload.get('schema')!='app':raise ValueError('Published instance identity differs')
        finally:conn.close()


def certificate_manifest(p,config):
    meta=metadata(p,'app-'+p['appID']+'-tls')
    return {'apiVersion':'cert-manager.io/v1','kind':'Certificate','metadata':meta,'spec':{
      'secretName':meta['name'],'dnsNames':[p['origin'][8:]],
      'issuerRef':{'name':config['certificateIssuer'],'kind':'ClusterIssuer','group':'cert-manager.io'},
      'secretTemplate':{'labels':meta['labels'],'annotations':meta['annotations']},
      'privateKey':{'rotationPolicy':'Always'}}}


def certificate_budget(p,namespace):
    if not namespace or namespace.get('metadata',{}).get('labels',{}).get('webdock.dev/certificate-budget')!='1':return False
    config=approved('managed-cms')
    certificates=json.loads(e.kubectl('get','certificates','-n',namespace['metadata']['name'],'-l','webdock.dev/provisioning-id','-o','json'))['items']
    if len(certificates)!=1:raise ValueError('Certificate solver budget requires exactly one approved instance Certificate')
    cert=certificates[0];e.check_owner(cert,p,False)
    meta=cert['metadata'];owner=meta.get('labels',{});spec=cert.get('spec',{});app_id=owner.get('webdock.dev/app-id','')
    if not re.fullmatch(r'[1-9][0-9]{0,18}',app_id) or not re.fullmatch(r'[1-9][0-9]{0,18}',owner.get('webdock.dev/provisioning-id','')) or meta['name']!='app-'+app_id+'-tls' or meta.get('annotations',{}).get('webdock.dev/template-id')!='managed-cms':raise ValueError('Unowned instance Certificate')
    expected_issuer={'name':config['certificateIssuer'],'kind':'ClusterIssuer','group':'cert-manager.io'}
    hosts=spec.get('dnsNames',[])
    if spec.get('issuerRef')!=expected_issuer or spec.get('secretName')!=meta['name'] or len(hosts)!=1 or not re.fullmatch(r'[1-9][0-9]{0,18}\.'+re.escape(config['domain']),hosts[0]):raise ValueError('Certificate solver authority changed')
    if any(spec.get('secretTemplate',{}).get('labels',{}).get(key)!=value for key,value in owner.items()):raise ValueError('Certificate secret ownership changed')
    return True


def publication_owner(obj,p,key):
    if obj:
        owned(obj,p)
        if p.get(key) and obj['metadata']['uid']!=p[key]:raise ValueError('Publication resource UID changed')
    elif p.get(key):raise ValueError('Published resource disappeared; automatic recreation is not allowed')


def certificate_secret(secret,p,certificate):
    owned(secret,p)
    if secret.get('type')!='kubernetes.io/tls' or secret['metadata'].get('annotations',{}).get('cert-manager.io/certificate-name')!=certificate['metadata']['name']:raise ValueError('TLS secret is not owned by the approved Certificate')
    refs=secret['metadata'].get('ownerReferences',[])
    if refs and not any(r.get('kind')=='Certificate' and r.get('uid')==certificate['metadata']['uid'] for r in refs):raise ValueError('TLS certificate owner UID changed')


def owned_solver_route(route,p,config,certificate):
    if not certificate or route['metadata'].get('namespace')!=p['namespace'] or route['metadata'].get('labels',{}).get('acme.cert-manager.io/http01-solver')!='true':return False
    rules=route.get('spec',{}).get('rules',[])
    if len(rules)!=1 or rules[0].get('host')!=p['origin'][8:]:return False
    paths=rules[0].get('http',{}).get('paths',[])
    if len(paths)!=1 or not re.fullmatch(r'/\.well-known/acme-challenge/[A-Za-z0-9_-]{1,256}',paths[0].get('path','')):return False
    current=route
    for kind in ['Challenge','Order','CertificateRequest','Certificate']:
        refs=[r for r in current['metadata'].get('ownerReferences',[]) if r.get('controller') is True]
        if len(refs)!=1 or refs[0].get('kind')!=kind:return False
        ref=refs[0]
        if ref.get('apiVersion')!=('acme.cert-manager.io/v1' if kind in ['Challenge','Order'] else 'cert-manager.io/v1'):return False
        if kind=='Certificate':return ref.get('name')==certificate['metadata']['name'] and ref.get('uid')==certificate['metadata']['uid']
        current=e.get(kind,ref['name'],p['namespace'])
        if not current:return None  # Wait for cert-manager garbage collection; never adopt an orphan.
        if current['metadata'].get('uid')!=ref.get('uid'):return False
        if kind=='Challenge':
            spec=current.get('spec',{})
            if spec.get('dnsName')!=p['origin'][8:] or spec.get('type')!='HTTP-01' or spec.get('issuerRef')!={'name':config['certificateIssuer'],'kind':'ClusterIssuer','group':'cert-manager.io'}:return False
    return False


def publish(p,config,checkpoint):
    deployment=e.get('deployment','app-'+p['appID'],p['namespace'])
    if not deployment:raise ValueError('Application is missing')
    e.check_owner(deployment,p,True)
    if deployment.get('status',{}).get('availableReplicas')!=1 or deployment.get('status',{}).get('observedGeneration',0)<deployment['metadata']['generation']:return None
    # An exact host already attached to another resource is never adopted.
    manifest=publication_manifest(p,config)
    certificate=certificate_manifest(p,config)
    prior=e.get('Certificate',certificate['metadata']['name'],p['namespace'])
    publication_owner(prior,p,'certificateUID')
    if prior and any(prior.get('spec',{}).get(key)!=certificate['spec'][key] for key in ['dnsNames','issuerRef','secretName']):raise ValueError('Certificate scope changed')
    routes=json.loads(e.kubectl('get','ingress','-A','-o','json'))['items']
    for route in routes:
        if any(rule.get('host')==p['origin'][8:] for rule in route.get('spec',{}).get('rules',[])):
            if route['metadata']['namespace']!=p['namespace'] or route['metadata']['name']!=manifest['metadata']['name']:
                solver=owned_solver_route(route,p,config,prior)
                if solver is None:return None
                if solver:continue
                raise ValueError('Origin is already owned')
            owned(route,p)
    secret=e.get('secret',certificate['spec']['secretName'],p['namespace'])
    if secret:
        if not prior:raise ValueError('Unowned TLS secret already exists')
        certificate_secret(secret,p,prior)
    existing_ingress=e.get('ingress',manifest['metadata']['name'],p['namespace'])
    publication_owner(existing_ingress,p,'ingressUID')
    auth_addresses=config['authAddresses']
    for address in auth_addresses:ipaddress.ip_address(address)
    policy={'apiVersion':'networking.k8s.io/v1','kind':'NetworkPolicy','metadata':metadata(p,'app-'+p['appID']+'-auth'),'spec':{'podSelector':{'matchLabels':{'webdock.dev/app-id':p['appID']}},'policyTypes':['Egress'],'egress':[{'to':[{'ipBlock':{'cidr':address+('/32' if ipaddress.ip_address(address).version==4 else '/128')}} for address in auth_addresses],'ports':[{'protocol':'TCP','port':443}]}]}}
    checkpoint();e.apply(policy,p)
    checkpoint();e.apply(manifest,p)
    checkpoint();e.apply(certificate,p)
    observed=e.get('ingress',manifest['metadata']['name'],p['namespace']);owned(observed,p)
    if observed['spec']!=manifest['spec']:raise ValueError('Published route differs from approved route')
    publication_owner(observed,p,'ingressUID')
    certificate=e.get('Certificate',certificate['metadata']['name'],p['namespace'])
    publication_owner(certificate,p,'certificateUID')
    if not certificate:raise ValueError('Certificate was not observed')
    checkpoint(certificateUID=certificate['metadata']['uid'],ingressUID=observed['metadata']['uid'])
    checkpoint();e.lease(p)
    e.kubectl('label','namespace',p['namespace'],'webdock.dev/certificate-budget=1','--overwrite')
    checkpoint();e.prepare_namespace(p,p['namespace'])
    if not any(c.get('type')=='Ready' and c.get('status')=='True' and c.get('observedGeneration',0)>=certificate['metadata']['generation'] for c in certificate.get('status',{}).get('conditions',[])):return None
    secret=e.get('secret',certificate['spec']['secretName'],p['namespace'])
    if not secret:return None
    certificate_secret(secret,p,certificate)
    checkpoint()
    try:tls_health(p)
    except ssl.SSLCertVerificationError as error:raise ValueError('Published TLS hostname or chain is not verified') from error
    except (PublicationPending,OSError,http.client.HTTPException):return None
    job=e.get('job','initialize-'+p['operationID'],p['namespace'])
    if job:
        owned(job,p)
        if job.get('status',{}).get('succeeded')!=1:raise ValueError('Initialization proof changed')
        checkpoint();e.lease(p)
        e.kubectl('delete','--raw','/apis/batch/v1/namespaces/'+p['namespace']+'/jobs/'+job['metadata']['name'],'-f','-',obj={'apiVersion':'v1','kind':'DeleteOptions','propagationPolicy':'Foreground','preconditions':{'uid':job['metadata']['uid'],'resourceVersion':job['metadata']['resourceVersion']}})
    return {'phase':'publication','instanceID':p['instanceID'],'origin':p['origin'],'ingressUID':observed['metadata']['uid'],'certificateUID':certificate['metadata']['uid'],'pinnedAddresses':p['pinnedAddresses']}


def execute(p,checkpoint):
    config=approved(p['templateID']);validate(p,config)
    return migrate(p,checkpoint) if p['phase']=='migration' else publish(p,config,checkpoint)
