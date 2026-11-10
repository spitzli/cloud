"""Fixed PostgreSQL profiles and fenced execution; no tenant SQL, images, paths or YAML."""
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import re
import subprocess
import uuid


def identifier(value):
    if not isinstance(value,str) or not re.fullmatch(r'[1-9][0-9]{0,18}',value) or int(value)>9223372036854775807:raise ValueError('Invalid database identity')
    return value


def role_names(database_id):
    identifier(database_id)
    return tuple(prefix+database_id for prefix in ['wd_owner_','wd_migrate_','wd_app_'])


def validate_packet(p,*,check_lease=True):
    try:
        if set(p)-{'previousPodUID','migration'}!={'version','operationID','generation','leaseToken','leaseUntil','clusterID','action','phase','revision','backend','database','members'}:raise ValueError('Invalid database packet')
        if type(p['version']) is not int or p['version']!=1:raise ValueError('Unsupported database protocol')
        for key in ['operationID','clusterID']:identifier(p[key])
        for key in ['generation','revision']:
            if type(p[key]) is not int or not 1<=p[key]<=2147483646:raise ValueError('Invalid database revision')
        if not re.fullmatch(r'[a-f0-9]{64}',p['leaseToken']):raise ValueError('Invalid database lease')
        lease_until=datetime.fromisoformat(p['leaseUntil'].replace('Z','+00:00'))
        if lease_until.tzinfo is None:raise ValueError('Invalid database lease')
        if check_lease and not 0<(lease_until-datetime.now(timezone.utc)).total_seconds()<=300:raise ValueError('Expired database lease')
        if p['action'] not in ['create','prepareMigration','migrate','pause','resume','restart','rotateCredentials','delete','purge','resize']:raise ValueError('Unsupported database action')
        if p['phase'] not in ['apply','configure','observe','uncertain']:raise ValueError('Invalid execution phase')
        b=p['backend'];identifier(b['id'])
        if set(b)!={'id','kind','namespace','node','poolID','revision','cnpgUID','resources','host','directPort','pooledPort'} or b['namespace']!='wd-db-'+b['id']:raise ValueError('Foreign database namespace')
        if b['kind'] not in ['shared-pool','dedicated'] or type(b['revision']) is not int or b['revision']<1:raise ValueError('Invalid installation kind')
        if not re.fullmatch(r'[a-z0-9][a-z0-9.-]{0,252}',b['node']) or str(uuid.UUID(b['poolID']))!=b['poolID']:raise ValueError('Invalid storage placement')
        if not re.fullmatch(r'db-'+b['id']+r'\.(?:[a-z0-9-]+\.)+[a-z]{2,63}',b['host']):raise ValueError('Invalid database hostname')
        if any(type(b[k]) is not int or not 32000<=b[k]<=32199 for k in ['directPort','pooledPort']) or b['directPort']==b['pooledPort']:raise ValueError('Invalid database ports')
        if b['cnpgUID'] is not None and (not isinstance(b['cnpgUID'],str) or not 1<=len(b['cnpgUID'])<=128):raise ValueError('Invalid CNPG identity')
        r=b['resources']
        if set(r)!={'cpuMillicores','memoryBytes','volumeBytes'}:raise ValueError('Invalid database resources')
        for key,minimum,maximum in [('cpuMillicores',100,64000),('memoryBytes',134217728,1099511627776),('volumeBytes',67108864,1099511627776)]:
            if type(r[key]) is not int or not minimum<=r[key]<=maximum:raise ValueError('Invalid database resource size')
        d=p['database']
        if d is not None:
            if set(d)!={'id','kind','name','revision','credentialRevision','connectionLimit','credentials'}:raise ValueError('Invalid logical database')
            identifier(d['id'])
            if d['name']!='db_'+d['id'] or d['kind']!=('shared' if b['kind']=='shared-pool' else 'dedicated'):raise ValueError('Invalid logical database owner')
            if d['revision']!=p['revision'] or type(d['revision']) is not int or type(d['credentialRevision']) is not int or d['credentialRevision']<1:raise ValueError('Invalid credential revision')
            if type(d['connectionLimit']) is not int or not 2<=d['connectionLimit']<=100:raise ValueError('Invalid connection budget')
            if set(d['credentials'])!={'migrationPassword','applicationPassword'} or any(not re.fullmatch(r'[A-Za-z0-9_-]{43}',v) for v in d['credentials'].values()):raise ValueError('Invalid database credentials')
            if b['kind']=='shared-pool' and p['action'] in ['restart']:raise ValueError('Shared customer cannot restart the pool')
        if not isinstance(p['members'],list) or len(p['members'])>100:raise ValueError('Invalid database membership')
        seen=set()
        for member in p['members']:
            if set(member)!={'id','projectID','databaseName','enabled'}:raise ValueError('Invalid database member')
            identifier(member['id']);identifier(member['projectID'])
            if member['id'] in seen or member['databaseName']!='db_'+member['id'] or type(member['enabled']) is not bool:raise ValueError('Invalid database member')
            seen.add(member['id'])
        if d and d['id'] not in seen:raise ValueError('Missing database membership')
    except (KeyError,TypeError,AttributeError,OverflowError):raise ValueError('Invalid database packet') from None
    return p


def auxiliary_databases(packet,config):
    mapping=config.get('platformAuxiliaryDatabases',{})
    if not isinstance(mapping,dict) or len(mapping)>10:raise ValueError('Invalid auxiliary database map')
    names=set();selected=[]
    for parent,entries in mapping.items():
        identifier(parent)
        if not isinstance(entries,list) or not 1<=len(entries)<=10:raise ValueError('Invalid auxiliary databases')
        for entry in entries:
            if not isinstance(entry,dict) or set(entry)!={'name','roles','projectID'}:raise ValueError('Invalid auxiliary database')
            name=entry['name'];roles=entry['roles'];identifier(entry['projectID'])
            if not isinstance(name,str) or not re.fullmatch(r'webdock_[a-z][a-z0-9_]{0,53}',name) or name in names:raise ValueError('Invalid auxiliary database name')
            if not isinstance(roles,list) or not 1<=len(roles)<=10 or any(not isinstance(r,str) or not re.fullmatch(r'webdock_[a-z][a-z0-9_]{0,53}',r) for r in roles) or len(set(roles))!=len(roles):raise ValueError('Invalid auxiliary database roles')
            names.add(name)
            if packet and any(m['id']==parent and m['enabled'] for m in packet['members']):
                if packet['backend']['kind']!='dedicated':raise ValueError('Auxiliary databases require a dedicated installation')
                selected.append(entry)
    return selected


def hba(p,config=None):
    rows=[]
    platform=(config or {}).get("platformDatabases",{})
    for member in p['members']:
        if member['enabled']:
            _,migration,app=role_names(member['id'])
            extra=platform.get(member['id'],[])
            if extra and p['backend']['kind']!='dedicated':raise ValueError('Platform service roles require a dedicated installation')
            users=','.join([migration,app,*extra])
            rows.append(f"hostssl {member['databaseName']} {users} all scram-sha-256")
    for entry in auxiliary_databases(p,config or {}):
        rows.append(f"hostssl {entry['name']} {','.join(entry['roles'])} all scram-sha-256")
    return rows+['host all all all reject']


def backend_manifests(p,config):
    validate_packet(p,check_lease=False);b=p['backend'];r=b['resources'];ns=b['namespace']
    labels={'app.kubernetes.io/managed-by':'webdock','webdock.dev/database-installation':b['id']}
    def meta(name):return {'name':name,'namespace':ns,'labels':labels}
    resources={'cpu':str(r['cpuMillicores'])+'m','memory':str(r['memoryBytes']),'ephemeral-storage':'268435456'}
    return [
      {'apiVersion':'v1','kind':'Namespace','metadata':{'name':ns,'labels':{**labels,'webdock.dev/workload-class':'database','pod-security.kubernetes.io/enforce':'restricted'}}},
      {'apiVersion':'networking.k8s.io/v1','kind':'NetworkPolicy','metadata':meta('default-deny'),'spec':{'podSelector':{},'policyTypes':['Ingress','Egress']}},
      {'apiVersion':'networking.k8s.io/v1','kind':'NetworkPolicy','metadata':meta('database-infrastructure'),'spec':{'podSelector':{},'policyTypes':['Ingress','Egress'],
        'ingress':([{'from':[{'namespaceSelector':{'matchLabels':{'webdock.dev/project-id':project}}} for project in sorted({m['projectID'] for m in p['members']}|{e['projectID'] for e in auxiliary_databases(p,config)})],'ports':[{'protocol':'TCP','port':5432}]}] if p['members'] else []) + [{'from':[{'podSelector':{}}]},{'from':[{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':'cnpg-system'}}}],'ports':[{'protocol':'TCP','port':8000}]},
                   {'from':[{'ipBlock':{'cidr':'0.0.0.0/0','except':config['podCIDRs']}}],'ports':[{'protocol':'TCP','port':5432}]}],
        'egress':[{'to':[{'podSelector':{}}]},{'to':[{'namespaceSelector':{'matchLabels':{'kubernetes.io/metadata.name':'kube-system'}}}],'ports':[{'protocol':'UDP','port':53},{'protocol':'TCP','port':53}]},
                  {'to':[{'ipBlock':{'cidr':cidr}} for cidr in config['apiCIDRs']],'ports':[{'protocol':'TCP','port':443},{'protocol':'TCP','port':6443}]}]}},
      {'apiVersion':'v1','kind':'ResourceQuota','metadata':meta('database-budget'),'spec':{'hard':{'requests.cpu':str(r['cpuMillicores']+200)+'m','limits.cpu':str(r['cpuMillicores']+200)+'m','requests.memory':str(r['memoryBytes']+536870912),'limits.memory':str(r['memoryBytes']+536870912),'requests.storage':str(r['volumeBytes']),'persistentvolumeclaims':'1','pods':'4'}}},
      {'apiVersion':'cert-manager.io/v1','kind':'Certificate','metadata':meta('server-tls'),'spec':{'secretName':'server-tls','dnsNames':[b['host'],'postgres-rw','postgres-rw.'+ns,'postgres-rw.'+ns+'.svc','postgres-rw.'+ns+'.svc.cluster.local'],'issuerRef':{'name':config['issuer'],'kind':'ClusterIssuer'},'privateKey':{'rotationPolicy':'Always'}}},
      {'apiVersion':'postgresql.cnpg.io/v1','kind':'Cluster','metadata':meta('postgres'),'spec':{
        'instances':1,'smartShutdownTimeout':10,'imageName':config['image'],'postgresUID':26,'postgresGID':26,'enableSuperuserAccess':False,
        'resources':{'requests':resources,'limits':resources},
        'storage':{'size':str(r['volumeBytes']),'storageClass':'','pvcTemplate':{'volumeName':'wd-db-'+b['id'],'accessModes':['ReadWriteOnce']}},
        'certificates':{'serverTLSSecret':'server-tls','serverCASecret':'server-ca'},
        'bootstrap':{'initdb':{'postInitSQL':['REVOKE ALL ON DATABASE postgres, template0, template1 FROM PUBLIC']}},
        'postgresql':{'enableAlterSystem':False,'parameters':{'password_encryption':'scram-sha-256','shared_buffers':str(max(32,r['memoryBytes']//4194304))+'MB','max_connections':'100','max_wal_size':'256MB','min_wal_size':'64MB','log_statement':'none'},'pg_hba':hba(p,config)}}},
      {'apiVersion':'postgresql.cnpg.io/v1','kind':'Pooler','metadata':meta('pooler'),'spec':{'cluster':{'name':'postgres'},'instances':1,'type':'rw',
        'pgbouncer':{'poolMode':'transaction','parameters':{'max_client_conn':'80','default_pool_size':'2' if any(m['id'] in config.get('platformDatabases',{}) for m in p['members']) else '5','max_db_connections':'10','max_user_connections':'10','server_connect_timeout':'10','client_tls_sslmode':'require','server_tls_sslmode':'verify-full'}},
        'template':{'spec':{'nodeSelector':{'kubernetes.io/hostname':b['node']},'initContainers':[{'name':'bootstrap-controller','resources':{'requests':{'cpu':'100m','memory':'268435456','ephemeral-storage':'134217728'},'limits':{'cpu':'100m','memory':'268435456','ephemeral-storage':'268435456'}}}],'containers':[{'name':'pgbouncer','image':config['poolerImage'],'resources':{'requests':{'cpu':'100m','memory':'134217728','ephemeral-storage':'134217728'},'limits':{'cpu':'100m','memory':'134217728','ephemeral-storage':'268435456'}}}]}}}},
    ]


def scram_secret(password):
    import base64,hashlib,hmac
    salt=os.urandom(16)
    salted=hashlib.pbkdf2_hmac('sha256',password.encode(),salt,4096)
    stored=hashlib.sha256(hmac.digest(salted,b'Client Key','sha256')).digest()
    server=hmac.digest(salted,b'Server Key','sha256')
    encode=lambda value:base64.b64encode(value).decode()
    return 'SCRAM-SHA-256$4096:'+encode(salt)+'$'+encode(stored)+':'+encode(server)


def sql_configuration(p):
    validate_packet(p,check_lease=False);d=p['database']
    if not d:raise ValueError('A logical database is required')
    owner,migration,app=role_names(d['id']);name=d['name'];marker='webdock:database:'+d['id']
    roles=[]
    for role in [owner,migration,app]:
        roles.append(f"""DO $webdock$ BEGIN
 IF NOT EXISTS(SELECT 1 FROM pg_catalog.pg_roles WHERE rolname='{role}') THEN
  CREATE ROLE "{role}" NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS;
  COMMENT ON ROLE "{role}" IS '{marker}';
 ELSIF (SELECT pg_catalog.shobj_description(oid,'pg_authid') FROM pg_catalog.pg_roles WHERE rolname='{role}') IS DISTINCT FROM '{marker}' THEN
  RAISE EXCEPTION 'Database role ownership conflict';
 END IF;
END $webdock$;""")
    return {'roles':'\n'.join(roles),
      'create':f'CREATE DATABASE "{name}" OWNER "{owner}";',
      'mark':f'COMMENT ON DATABASE "{name}" IS \'{marker}\';',
      'database':f'REVOKE ALL ON DATABASE "{name}" FROM PUBLIC; GRANT CONNECT ON DATABASE "{name}" TO "{migration}", "{app}";',
      'schema':f'''REVOKE ALL ON SCHEMA public FROM PUBLIC;
CREATE SCHEMA IF NOT EXISTS app AUTHORIZATION "{migration}";
GRANT USAGE ON SCHEMA app TO "{app}";
ALTER DEFAULT PRIVILEGES FOR ROLE "{migration}" IN SCHEMA app GRANT SELECT,INSERT,UPDATE,DELETE ON TABLES TO "{app}";
ALTER DEFAULT PRIVILEGES FOR ROLE "{migration}" IN SCHEMA app GRANT USAGE,SELECT ON SEQUENCES TO "{app}";
ALTER DEFAULT PRIVILEGES FOR ROLE "{migration}" IN SCHEMA app REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC;
ALTER ROLE "{migration}" IN DATABASE "{name}" SET search_path=app,pg_catalog;
ALTER ROLE "{app}" IN DATABASE "{name}" SET search_path=app,pg_catalog;''',
      'enable':f'''SET log_statement='none'; SET log_min_duration_statement=-1;
ALTER ROLE "{migration}" NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS LOGIN CONNECTION LIMIT {d['connectionLimit']} PASSWORD '{scram_secret(d['credentials']['migrationPassword'])}';
ALTER ROLE "{app}" NOSUPERUSER NOCREATEDB NOCREATEROLE NOREPLICATION NOBYPASSRLS LOGIN CONNECTION LIMIT {d['connectionLimit']} PASSWORD '{scram_secret(d['credentials']['applicationPassword'])}';''',
      'disable':f'''ALTER ROLE "{migration}" NOLOGIN; ALTER ROLE "{app}" NOLOGIN;
SELECT pg_catalog.pg_terminate_backend(pid) FROM pg_catalog.pg_stat_activity WHERE usename IN ('{migration}','{app}') AND pid<>pg_catalog.pg_backend_pid();'''}


def configuration():
    path=Path(os.environ.get('SPITZLI_DATABASE_CONFIG','/etc/webdock-postgres.json'))
    info=path.stat()
    if path.is_symlink() or info.st_uid!=os.getuid() or info.st_mode&0o077:raise ValueError('Private database configuration required')
    config=json.loads(path.read_text())
    required={'image','poolerImage','issuer','apiCIDRs','podCIDRs','storageSocket','caFile'}
    if not required<=set(config)<=required|{'platformDatabases','platformAuxiliaryDatabases','additionalStoragePools'}:raise ValueError('Invalid database configuration')
    auxiliary_databases(None,config)
    platform=config.get('platformDatabases',{})
    if not isinstance(platform,dict) or len(platform)>10:raise ValueError('Invalid platform database map')
    for identity,roles in platform.items():
        identifier(identity)
        if not isinstance(roles,list) or not 1<=len(roles)<=32 or any(not isinstance(r,str) or not re.fullmatch(r'webdock_[a-z][a-z0-9_]{0,53}',r) for r in roles) or len(set(roles))!=len(roles):raise ValueError('Invalid platform database roles')
    for key in ['image','poolerImage']:
        if not re.fullmatch(r'[A-Za-z0-9_.:/-]+@sha256:[a-f0-9]{64}',config[key]):raise ValueError('Pinned database images required')
    if not re.fullmatch(r'.+:18\.[0-9][A-Za-z0-9_.-]*@sha256:[a-f0-9]{64}',config['image']):raise ValueError('PostgreSQL major tag and digest are both required')
    if not re.fullmatch(r'[a-z0-9][a-z0-9-]{0,62}',config['issuer']):raise ValueError('Invalid database certificate issuer')
    import ipaddress
    for key in ['apiCIDRs','podCIDRs']:
        if not isinstance(config[key],list) or not 1<=len(config[key])<=20:raise ValueError('Invalid database network boundary')
        for cidr in config[key]:ipaddress.ip_network(cidr,strict=True)
    storage_routes(config)
    ca=Path(config['caFile']).read_bytes()
    if len(ca)>262144 or b'BEGIN CERTIFICATE' not in ca or b'PRIVATE KEY' in ca:raise ValueError('A public CA certificate bundle is required')
    import ssl
    ssl.create_default_context(cafile=config['caFile'])
    return config


def storage_routes(config):
    extra=config.get('additionalStoragePools',[])
    if not isinstance(extra,list) or len(extra)>7:raise ValueError('At most eight operator-owned database pools are supported')
    routes=[{'storageSocket':config['storageSocket']}]+extra
    sockets=set();nodes=set();pools=set()
    for index,route in enumerate(routes):
        if not isinstance(route,dict) or set(route)!=({'storageSocket','node','poolID'} if index else {'storageSocket'}):raise ValueError('Invalid database pool route')
        path=route['storageSocket']
        if not isinstance(path,str) or len(path)>103 or not re.fullmatch(r'/run/webdock-postgres-storage/(?:[A-Za-z0-9_-]+/)*[A-Za-z0-9_.-]+\.sock',path) or not Path(path).resolve().is_relative_to('/run/webdock-postgres-storage') or path in sockets:raise ValueError('Unsafe or duplicate database storage socket')
        sockets.add(path)
        if index:
            if not isinstance(route['node'],str) or not re.fullmatch(r'[a-z0-9][a-z0-9.-]{0,252}',route['node']) or str(uuid.UUID(route['poolID']))!=route['poolID']:raise ValueError('Invalid database pool placement')
            if route['node'] in nodes or route['poolID'] in pools:raise ValueError('Duplicate database pool placement')
            nodes.add(route['node']);pools.add(route['poolID'])
    return routes


def storage_pools(config,nodes):
    from storage_client import request
    result=[];seen_nodes=set();seen_pools=set();allocations=set()
    routes=storage_routes(config)
    for index,route in enumerate(routes):
        try:pool=request('database.capability',nodes,socket_path=route['storageSocket'])
        except (FileNotFoundError,ConnectionError,TimeoutError):continue
        # An unavailable independent pool is omitted; malformed identity is never ignored.
        if index==0 and isinstance(pool,dict) and any(pool.get('node')==extra['node'] or pool.get('poolID')==extra['poolID'] for extra in routes[1:]):raise ValueError('Primary pool conflicts with an additional route')
        if not isinstance(pool,dict) or pool.get('node') not in nodes or str(uuid.UUID(pool['poolID']))!=pool['poolID']:raise ValueError('Invalid database pool observation')
        if any(pool.get(key)!=route[key] for key in ['node','poolID'] if key in route):raise ValueError('Database pool route identity changed')
        if pool['node'] in seen_nodes or pool['poolID'] in seen_pools:raise ValueError('Database pools must have unique nodes and identities')
        seen_nodes.add(pool['node']);seen_pools.add(pool['poolID'])
        for key in ['capacityBytes','availableBytes','allocatedBytes']:
            if type(pool.get(key)) is not int or not 0<=pool[key]<=9007199254740991:raise ValueError('Invalid database pool capacity')
        if type(pool.get('ready')) is not bool or not isinstance(pool.get('allocations'),list) or len(pool['allocations'])>100:raise ValueError('Invalid database pool readiness')
        for allocation in pool['allocations']:
            identifier(allocation['installationID'])
            if allocation['installationID'] in allocations:raise ValueError('Database installation exists in multiple pools')
            allocations.add(allocation['installationID'])
        result.append((route['storageSocket'],pool))
    return result


def storage_request(config,action,value):
    from storage_client import request
    if action not in ['database.ensure','database.state','database.retained','database.purge']:raise ValueError('Unsupported database storage action')
    from database_storage import validate
    validate(value)
    routes=storage_routes(config)
    selected=routes[0]['storageSocket']
    for route in routes[1:]:
        node_matches=route['node']==value['node'];pool_matches=route['poolID']==value['poolID']
        if node_matches!=pool_matches:raise ValueError('Database node and pool conflict with an approved route')
        if node_matches:selected=route['storageSocket']
    # Never query unrelated pools. The selected private helper validates the exact
    # cluster/node/pool/installation before every operation, including legacy fallback.
    return request(action,value,socket_path=selected)


def kubectl(*args,source=None,timeout=25):
    config=os.environ.get('SPITZLI_DATABASE_KUBECONFIG')
    command=['kubectl','--kubeconfig',config] if config else ['k3s','kubectl']
    result=subprocess.run(command+list(args),input=source,capture_output=True,text=True,timeout=timeout)
    if result.returncode or len(result.stdout)>4000000:raise RuntimeError('Database Kubernetes operation failed')
    return result.stdout.strip()


def get(kind,name,namespace=None):
    text=kubectl('get',kind,name,*(['-n',namespace] if namespace else []),'--ignore-not-found','-o','json')
    return json.loads(text) if text else None


def owned(obj,p):
    if obj['metadata'].get('labels',{}).get('webdock.dev/database-installation')!=p['backend']['id']:raise ValueError('Foreign database object')
    if obj.get('kind')=='Cluster' and p['backend']['cnpgUID'] and obj['metadata']['uid']!=p['backend']['cnpgUID']:raise ValueError('Database installation UID changed')
    return obj


def apply(obj,p,guard):
    existing=get(obj['kind'],obj['metadata']['name'],obj['metadata'].get('namespace'))
    if existing:owned(existing,p)
    guard()
    kubectl('apply','--server-side','--field-manager=spitzli-postgres','-f','-',source=json.dumps(obj))


def sql(p,statement,database='postgres'):
    # Statements are generated by this module; neither identifiers nor SQL come from customers.
    # Cold bounded PostgreSQL profiles can need longer than Kubernetes metadata reads.
    return kubectl('exec','-i','-n',p['backend']['namespace'],'postgres-1','-c','postgres','--',
                   'psql','-X','-U','postgres','-d',database,'-v','ON_ERROR_STOP=1','-At',source=statement,timeout=60)


def database_exists(p):
    d=p['database']
    marker=sql(p,"SELECT COALESCE(pg_catalog.shobj_description(oid,'pg_database'),'unowned') FROM pg_catalog.pg_database WHERE datname='"+d['name']+"'")
    if marker and marker!='webdock:database:'+d['id']:raise ValueError('Unowned logical database requires reconciliation')
    return bool(marker)


def endpoint_services(p,guard):
    b=p['backend'];ns=b['namespace']
    for suffix,target,port in [('direct','postgres-rw',b['directPort']),('pooled','pooler',b['pooledPort'])]:
        service=get('service',target,ns)
        if not service:return False
        for existing in json.loads(kubectl('get','services','-A','-o','json'))['items']:
            if any(v.get('nodePort')==port for v in existing['spec'].get('ports',[])) and (existing['metadata']['namespace']!=ns or existing['metadata']['name']!='public-'+suffix):raise ValueError('Database NodePort collision')
        apply({'apiVersion':'v1','kind':'Service','metadata':{'name':'public-'+suffix,'namespace':ns,'labels':{'webdock.dev/database-installation':b['id']}},
            'spec':{'type':'NodePort','selector':service['spec']['selector'],'ports':[{'name':'postgres','port':5432,'targetPort':5432,'nodePort':port,'protocol':'TCP'}]}},p,guard)
    return True


def verify_endpoint_tls(host,port,ca_file):
    import socket,ssl,struct
    try:
        context=ssl.create_default_context(cafile=ca_file)
        with socket.create_connection((host,port),timeout=3) as connection:
            connection.sendall(struct.pack('!II',8,80877103))
            if connection.recv(1)!=b'S':return False
            with context.wrap_socket(connection,server_hostname=host) as secured:
                return bool(secured.getpeercert())
    except (OSError,ssl.SSLError):return False


def verify_database_logins(p):
    d=p['database'];b=p['backend']
    if not d:return True
    pooled=get('service','pooler',b['namespace'])
    if not pooled:return False
    checks=[('wd_migrate_'+d['id'],d['credentials']['migrationPassword'],'127.0.0.1'),
            ('wd_app_'+d['id'],d['credentials']['applicationPassword'],pooled['spec']['clusterIP'])]
    for role,password,address in checks:
        connection=f"host={b['host']} hostaddr={address} port=5432 dbname={d['name']} user={role} sslmode=verify-full sslrootcert=/controller/certificates/server-ca.crt connect_timeout=3"
        # The secret is stdin only, never an exec argument, generated SQL, or diagnostic.
        try:
            result=kubectl('exec','-i','-n',b['namespace'],'postgres-1','-c','postgres','--',
                'sh','-c','IFS= read -r PGPASSWORD; export PGPASSWORD; exec psql "$@"','--',
                connection,'-X','-At','-c','SELECT 1',source=password+'\n')
        except RuntimeError:return False
        if result!='1':return False
    return True


def endpoints_ready(p):
    b=p['backend']
    for name,port in [('public-direct',b['directPort']),('public-pooled',b['pooledPort'])]:
        service=get('service',name,b['namespace'])
        if not service:return False
        owned(service,p)
        if service['spec'].get('type')!='NodePort' or not any(v.get('nodePort')==port for v in service['spec'].get('ports',[])):return False
    return True


def runtime_ready(p,cluster,pod,config):
    if not pod or cluster.get('status',{}).get('readyInstances',0)!=1:return False
    if not any(v['type']=='Ready' and v['status']=='True' for v in pod.get('status',{}).get('conditions',[])):return False
    if not any(v.get('uid')==cluster['metadata']['uid'] for v in pod['metadata'].get('ownerReferences',[])):return False
    container=next((c for c in pod['spec']['containers'] if c['name']=='postgres'),None)
    if not container or container['image']!=config['image']:return False
    from observer import quantity
    for kind in ['requests','limits']:
        resources=container.get('resources',{}).get(kind,{})
        if quantity(resources.get('cpu','0'),cpu=True)!=p['backend']['resources']['cpuMillicores'] or quantity(resources.get('memory','0'))!=p['backend']['resources']['memoryBytes']:return False
    return True


def execute_database(p,checkpoint):
    """One bounded stage per agent turn; None means observe again, never success on apply."""
    validate_packet(p)
    if p['phase']=='uncertain':raise ValueError('Interrupted mutation requires operator reconciliation')
    config=configuration();b=p['backend'];ns=b['namespace'];d=p['database']
    affected={m['id'] for m in p['members']}
    if d:affected.add(d['id'])
    if affected.intersection(set(config.get('platformDatabases',{}))|set(config.get('platformAuxiliaryDatabases',{}))) and p['action'] in ['pause','restart','resize','delete','purge','rotateCredentials','migrate']:
        raise ValueError('Platform database requires reviewed offline maintenance')
    if p['action']=='migrate':
        from database_migrations import execute_migration
        return execute_migration(p,checkpoint,config)
    if p['action']=='purge' and b['kind']=='dedicated':return purge_installation(p,checkpoint,config)
    current=get('cluster','postgres',ns)
    if current:owned(current,p)
    def guard():
        # Phase checkpoints renew the server lease; the claimed deadline is only an entry fence.
        checkpoint()
    if p['phase']=='apply':
        guard()
        if not current:
            if b['cnpgUID'] or b['revision']!=1:raise ValueError('Missing database installation; refusing empty replacement')
            if p['action'] not in ['create','prepareMigration']:raise ValueError('Installation must already exist')
            volume=storage_request(config,'database.ensure',{'version':1,'installationID':b['id'],'clusterID':p['clusterID'],'revision':b['revision'],'node':b['node'],'poolID':b['poolID'],'volumeBytes':b['resources']['volumeBytes'],'leaseUntil':p['leaseUntil']})
            manifests=backend_manifests(p,config)
            apply(manifests[0],p,guard);apply(volume['persistentVolume'],p,guard)
            import base64
            certificate=Path(config['caFile']).read_bytes()
            if len(certificate)>262144 or b'BEGIN CERTIFICATE' not in certificate:raise ValueError('Database server CA bundle required')
            apply({'apiVersion':'v1','kind':'Secret','metadata':{'name':'server-ca','namespace':ns,'labels':{'webdock.dev/database-installation':b['id']}},'type':'Opaque','data':{'ca.crt':base64.b64encode(certificate).decode()}},p,guard)
            for obj in manifests[1:]:apply(obj,p,guard)
            current=get('cluster','postgres',ns);owned(current,p)
        if p['action']=='resume' and b['kind']=='dedicated':
            guard();kubectl('annotate','cluster','postgres','-n',ns,'cnpg.io/hibernation=off','--overwrite')
        before=get('pod','postgres-1',ns)['metadata']['uid'] if p['action']=='restart' else None
        checkpoint(phase='applied',cnpgUID=current['metadata']['uid'],caCertificate=Path(config['caFile']).read_text(),**({'previousPodUID':before} if before else {}))
        return None
    if not current:raise ValueError('Database installation disappeared')
    uid=current['metadata']['uid']
    if p['phase']=='configure':
        if current.get('status',{}).get('readyInstances',0)!=1 and p['action']!='restart':return None
        checkpoint(phase='configuring',cnpgUID=uid)
        if p['action'] in ['create','rotateCredentials','resume'] and d:
            statements=sql_configuration(p)
            guard();sql(p,statements['roles'])
            if not database_exists(p):
                if p['action']!='create':raise ValueError('Missing database; refusing empty replacement')
                guard();sql(p,statements['create']);guard();sql(p,statements['mark'])
            guard();sql(p,statements['database']);guard();sql(p,statements['schema'],d['name'])
            guard();kubectl('patch','cluster','postgres','-n',ns,'--type=merge','-p',json.dumps({'spec':{'postgresql':{'pg_hba':hba(p,config)}}}))
            guard();sql(p,statements['enable'])
        elif p['action'] in ['pause','delete'] and d:
            if not database_exists(p):raise ValueError('Database identity missing')
            guard();sql(p,sql_configuration(p)['disable'])
            if b['kind']=='dedicated':
                guard();kubectl('annotate','cluster','postgres','-n',ns,'cnpg.io/hibernation=on','--overwrite')
        elif p['action']=='restart' and b['kind']=='dedicated':
            pod=get('pod','postgres-1',ns)
            if not p.get('previousPodUID') or not pod or pod['metadata']['uid']!=p['previousPodUID']:raise ValueError('Instance changed before restart')
            delete_owned(pod,p,guard,cnpg_owner=True)
        elif p['action']=='resize' and d:
            if b['kind']=='dedicated':
                manifests=backend_manifests(p,config)
                quota=next(o for o in manifests if o['kind']=='ResourceQuota')
                desired=next(o for o in manifests if o['kind']=='Cluster')['spec']['resources']
                apply(quota,p,guard);guard()
                kubectl('patch','cluster','postgres','-n',ns,'--type=merge','-p',json.dumps({'spec':{'resources':desired}}))
            _,migration,app=role_names(d['id']);guard()
            sql(p,f'ALTER ROLE "{migration}" CONNECTION LIMIT {d["connectionLimit"]}; ALTER ROLE "{app}" CONNECTION LIMIT {d["connectionLimit"]};')
        elif p['action']=='purge' and d:
            if b['kind']=='dedicated':raise ValueError('Dedicated physical purge requires verified teardown')
            if database_exists(p):
                guard();sql(p,sql_configuration(p)['disable']);guard();sql(p,'DROP DATABASE "'+d['name']+'";')
            for role in role_names(d['id']):
                guard()
                references=sql(p,"SELECT count(*) FROM pg_catalog.pg_shdepend WHERE refclassid='pg_authid'::regclass AND refobjid=(SELECT oid FROM pg_catalog.pg_roles WHERE rolname='"+role+"')")
                if references=='0':guard();sql(p,'DROP ROLE IF EXISTS "'+role+'";')
        elif p['action'] not in ['create','prepareMigration']:raise ValueError('Unsupported database maintenance stage')
        if p['action'] not in ['pause','delete','purge'] and not endpoint_services(p,guard):raise ValueError('Database endpoint services are not reconciled')
        checkpoint(phase='observing',cnpgUID=uid)
        return None
    if p['phase']!='observe':raise ValueError('Unexpected database stage')
    if p['action'] in ['pause','delete']:
        state='paused' if p['action']=='pause' else 'retained'
        if b['kind']=='dedicated':
            if volume_users(ns):return None
            checkpoint()
            storage_request(config,'database.retained',{'version':1,'installationID':b['id'],'clusterID':p['clusterID'],'revision':b['revision'],'node':b['node'],'poolID':b['poolID'],'volumeBytes':b['resources']['volumeBytes'],'leaseUntil':p['leaseUntil']})
        elif d:
            _,migration,app=role_names(d['id'])
            if sql(p,f"SELECT count(*) FROM pg_catalog.pg_roles WHERE rolname IN ('{migration}','{app}') AND rolcanlogin")!='0':return None
        return {'installationID':b['id'],'databaseID':d['id'] if d else None,'cnpgUID':uid,'revision':p['revision'],'state':state,'endpointReady':b['kind']=='shared-pool'}
    if p['action']=='purge':
        if database_exists(p):return None
        return {'installationID':b['id'],'databaseID':d['id'],'cnpgUID':uid,'revision':p['revision'],'state':'purged','endpointReady':b['kind']=='shared-pool'}
    pod=get('pod','postgres-1',ns)
    if not runtime_ready(p,current,pod,config):return None
    if p['action']=='restart' and (not p.get('previousPodUID') or pod['metadata']['uid']==p['previousPodUID']):return None
    if d and (not database_exists(p) or not verify_database_logins(p)):return None
    pooler=get('deployment','pooler',ns)
    certificate=get('certificate','server-tls',ns)
    if not pooler or pooler.get('status',{}).get('readyReplicas',0)!=1 or not certificate or not any(c['type']=='Ready' and c['status']=='True' for c in certificate.get('status',{}).get('conditions',[])):return None
    if not endpoints_ready(p):return None
    if not all(verify_endpoint_tls(b['host'],port,config['caFile']) for port in [b['directPort'],b['pooledPort']]):return None
    return {'installationID':b['id'],'databaseID':d['id'] if d else None,'cnpgUID':uid,'revision':p['revision'],'state':'ready','endpointReady':True}


def database_capability(nodes):
    """Advertise only verified operator-owned DB pools and a healthy pinned operator."""
    try:
        config=configuration()
        operator=get('deployment','cnpg-controller-manager','cnpg-system')
        if not operator or operator.get('status',{}).get('availableReplicas',0)<1:return None
        for name in ['webdock-database-pods','webdock-database-services']:
            policy=get('validatingadmissionpolicy',name);binding=get('validatingadmissionpolicybinding',name)
            if not policy or not binding or policy['metadata'].get('labels',{}).get('webdock.dev/database-boundary-version')!='1' or 'Deny' not in binding['spec'].get('validationActions',[]) or policy.get('status',{}).get('typeChecking',{}).get('expressionWarnings'):return None
        node_map={n['metadata']['name']:n for n in json.loads(kubectl('get','nodes','-o','json'))['items']}
        pools=storage_pools(config,nodes)
        if not pools:return None
        pods=json.loads(kubectl('get','pods','-A','-o','json'))['items']
        services=json.loads(kubectl('get','services','-A','-o','json'))['items']
        from observer import quantity
        observed=[]
        for _,pool in pools:
            if not pool['ready']:continue
            node=node_map.get(pool['node'])
            if not node or node.get('spec',{}).get('unschedulable') or node.get('spec',{}).get('taints'):continue
            conditions={c['type']:c['status'] for c in node['status'].get('conditions',[])}
            if conditions.get('Ready')!='True' or any(conditions.get(k)=='True' for k in ['MemoryPressure','DiskPressure','PIDPressure']):continue
            cpu=memory=0;by_installation={}
            for pod in pods:
                if pod.get('status',{}).get('phase') in ['Succeeded','Failed']:continue
                if pod.get('spec',{}).get('nodeName') not in [None,pool['node']]:continue
                # Preserve conservative accounting, including unplaced and init work.
                for container in pod['spec'].get('containers',[])+pod['spec'].get('initContainers',[]):
                    resources=container.get('resources',{}).get('requests',{})
                    requested_cpu=quantity(resources.get('cpu','0'),cpu=True);requested_memory=quantity(resources.get('memory','0'))
                    cpu+=requested_cpu;memory+=requested_memory
                    match=re.fullmatch(r'wd-db-([1-9][0-9]{0,18})',pod['metadata']['namespace'])
                    if match:
                        allocation=by_installation.setdefault(match[1],{'installationID':match[1],'cpuMillicores':0,'memoryBytes':0})
                        allocation['cpuMillicores']+=requested_cpu;allocation['memoryBytes']+=requested_memory
            pool.update(requestedCpuMillicores=cpu,requestedMemoryBytes=memory,observedDatabases=list(by_installation.values()))
            observed.append(pool)
        if not observed:return None
        ports=[p['nodePort'] for service in services for p in service['spec'].get('ports',[]) if p.get('nodePort')]
        return {'databaseVersion':1,'databasePools':observed,'databaseNodePorts':ports}
    except (OSError,ValueError,KeyError,RuntimeError,subprocess.SubprocessError):return None


def volume_users(namespace):
    return [pod for pod in json.loads(kubectl('get','pods','-n',namespace,'-o','json'))['items']
            if pod.get('status',{}).get('phase') not in ['Succeeded','Failed']
            and any(v.get('persistentVolumeClaim',{}).get('claimName')=='postgres-1' for v in pod['spec'].get('volumes',[]))]


def delete_owned(obj,p,guard,cnpg_owner=False):
    if cnpg_owner:
        if obj['metadata'].get('namespace')!=p['backend']['namespace'] or not any(o.get('uid')==p['backend']['cnpgUID'] for o in obj['metadata'].get('ownerReferences',[])):raise ValueError('Foreign CNPG-owned object')
    else:owned(obj,p)
    guard()
    group='/api/v1' if obj['apiVersion']=='v1' else '/apis/'+obj['apiVersion']
    plural={'Job':'jobs','Secret':'secrets','ConfigMap':'configmaps','NetworkPolicy':'networkpolicies','Pod':'pods','Cluster':'clusters','Pooler':'poolers','Service':'services','PersistentVolume':'persistentvolumes','PersistentVolumeClaim':'persistentvolumeclaims','Namespace':'namespaces'}[obj['kind']]
    namespace=obj['metadata'].get('namespace')
    endpoint=group+('/namespaces/'+namespace if namespace else '')+'/'+plural+'/'+obj['metadata']['name']
    options={'apiVersion':'v1','kind':'DeleteOptions','propagationPolicy':'Foreground','preconditions':{'uid':obj['metadata']['uid'],'resourceVersion':obj['metadata']['resourceVersion']}}
    kubectl('delete','--raw',endpoint,'-f','-',source=json.dumps(options))


def purge_installation(p,checkpoint,config):
    b=p['backend'];ns=b['namespace'];uid=b['cnpgUID']
    if not uid:raise ValueError('A confirmed installation is required for physical purge')
    def guard():checkpoint()
    volume={'version':1,'installationID':b['id'],'clusterID':p['clusterID'],'revision':b['revision'],'node':b['node'],'poolID':b['poolID'],'volumeBytes':b['resources']['volumeBytes'],'leaseUntil':p['leaseUntil']}
    namespace=get('namespace',ns)
    if namespace:owned(namespace,p)
    if p['phase']=='apply':
        cluster=get('cluster','postgres',ns)
        if cluster:
            owned(cluster,p)
            if cluster['metadata'].get('annotations',{}).get('cnpg.io/hibernation')!='on' or volume_users(ns):raise ValueError('Stop and retain the database before physical purge')
        for kind,name in [('Service','public-direct'),('Service','public-pooled'),('Pooler','pooler'),('Cluster','postgres')]:
            obj=get(kind,name,ns)
            if obj:delete_owned(obj,p,guard)
        checkpoint(phase='applied',cnpgUID=uid);return None
    if p['phase']=='configure':
        if get('cluster','postgres',ns) or get('pooler','pooler',ns) or volume_users(ns):return None
        pv=get('persistentvolume','wd-db-'+b['id']);pvc=get('persistentvolumeclaim','postgres-1',ns)
        if pv:
            owned(pv,p)
            ref=pv['spec'].get('claimRef',{})
            if ref.get('namespace')!=ns or ref.get('name')!='postgres-1':raise ValueError('Database PV binding changed')
        if pvc:
            if pvc['spec'].get('volumeName')!='wd-db-'+b['id']:raise ValueError('Database PVC binding changed')
            if not any(o.get('uid')==uid for o in pvc['metadata'].get('ownerReferences',[])):raise ValueError('Database PVC owner changed')
            delete_owned(pvc,p,guard,cnpg_owner=True);return None
        if pv:delete_owned(pv,p,guard);return None
        checkpoint(phase='configuring',cnpgUID=uid);guard()
        storage_request(config,'database.purge',volume)
        if namespace:delete_owned(namespace,p,guard)
        checkpoint(phase='observing',cnpgUID=uid);return None
    if p['phase']!='observe':raise ValueError('Uncertain physical purge requires reconciliation')
    if namespace or get('persistentvolume','wd-db-'+b['id']):return None
    if storage_request(config,'database.state',volume)['state']!='purged':return None
    return {'installationID':b['id'],'databaseID':p['database']['id'],'cnpgUID':uid,'revision':p['revision'],'state':'purged','endpointReady':False}


def observe_databases(config):
    settings=configuration();result=[]
    if set(config)!={'installations'} or not isinstance(config['installations'],list) or len(config['installations'])>100:raise ValueError('Invalid database inventory request')
    for b in config['installations']:
        identifier(b['id'])
        if b['namespace']!='wd-db-'+b['id'] or len(b['members'])>100:raise ValueError('Invalid database inventory identity')
        for member in b['members']:identifier(member)
        cluster=get('cluster','postgres',b['namespace'])
        if not cluster or cluster['metadata']['uid']!=b['cnpgUID'] or cluster['metadata'].get('labels',{}).get('webdock.dev/database-installation')!=b['id']:continue
        ready=cluster.get('status',{}).get('readyInstances',0)==1
        values=[];used=None
        if ready:
            try:
                packet={'backend':b}
                names=','.join("'db_"+member+"'" for member in b['members']) or "''"
                rows=sql(packet,"SELECT COALESCE(pg_catalog.json_agg(pg_catalog.json_build_object('id',substring(datname from 4),'logicalBytes',pg_catalog.pg_database_size(oid))), '[]'::json) FROM pg_catalog.pg_database WHERE datname IN ("+names+")")
                values=json.loads(rows)
                used=int(kubectl('exec','-n',b['namespace'],'postgres-1','-c','postgres','--','df','--output=used','-B1','/var/lib/postgresql/data').splitlines()[-1].strip())
                ready=all(verify_endpoint_tls(b['host'],port,settings['caFile']) for port in [b['directPort'],b['pooledPort']])
            except (ValueError,RuntimeError,subprocess.SubprocessError):ready=False
        if b['state'] in ['paused','retained']:ready=True
        result.append({'installationID':b['id'],'cnpgUID':b['cnpgUID'],'ready':ready,'filesystemBytes':used,'databases':values})
    return {'observedAt':datetime.now(timezone.utc).isoformat(timespec='milliseconds').replace('+00:00','Z'),'installations':result}
