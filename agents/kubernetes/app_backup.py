#!/usr/bin/env python3
"""Operator-only matched CMS backup/isolated restore on the existing Kubernetes host.

Uses the agent's executor lock, a database LOGIN fence and a stopped Deployment.
Failure leaves the application fenced. There is no automatic destructive retry.
The destination must be an existing separately mounted NFS/CIFS/SSHFS filesystem.
"""
import argparse
from datetime import datetime, timezone
import fcntl
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tarfile
import time
import executor as e
from database_migrations import COUNTS_SQL


def sha256(path):
    digest=hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda:stream.read(1024*1024),b''):digest.update(chunk)
    return digest.hexdigest()


def inspect_media(path):
    files={};total=0
    with tarfile.open(path,'r:') as archive:
        for item in archive:
            name=PurePosixPath(item.name)
            if name.is_absolute() or '..' in name.parts or not name.parts or name.parts[0]!='media' or not (item.isdir() or item.isfile()):raise ValueError('Unsafe media archive')
            if item.isfile():
                if str(name) in files:raise ValueError('Duplicate archive entry')
                total+=item.size
                if total>1024**4 or len(files)>=1_000_000:raise ValueError('Media archive exceeds limit')
                digest=hashlib.sha256()
                stream=archive.extractfile(item)
                for chunk in iter(lambda:stream.read(1024*1024),b''):digest.update(chunk)
                files[str(name)]=digest.hexdigest()
    return files


def verify_set(directory):
    manifest=json.loads((directory/'manifest.json').read_text())
    if manifest.get('version')!=1 or set(manifest['files'])!={'database.dump','media.tar','database-fingerprint.txt'}:raise ValueError('Invalid backup manifest')
    for name,digest in manifest['files'].items():
        if sha256(directory/name)!=digest:raise ValueError('Backup checksum mismatch')
    if inspect_media(directory/'media.tar')!=manifest['media']:raise ValueError('Media fingerprint mismatch')
    return manifest


def assert_isolated(source,target):
    if any(source[key]==target[key] for key in ['projectID','appID','databaseID','namespace']):raise ValueError('Restore requires a separate project, database, app and PVC')


def configuration(path):
    st=path.stat()
    if st.st_uid!=os.getuid() or st.st_mode & 0o077:raise ValueError('Backup config must be private and owned by the operator')
    value=json.loads(path.read_text())
    required={'projectID','clusterID','appID','databaseID','backendID','namespace','deploymentUID','pvcUID','databaseClusterUID','image','helperImage','volumeBytes'}
    if set(value)!=required:raise ValueError('Unexpected backup configuration')
    for key in ['projectID','clusterID','appID','databaseID','backendID']:
        if not re.fullmatch(r'[1-9][0-9]{0,18}',value[key]):raise ValueError('Invalid backup identity')
    for key in ['image','helperImage']:
        if not re.fullmatch(r'[a-z0-9.-]+/[A-Za-z0-9_./-]+@sha256:[a-f0-9]{64}',value[key]):raise ValueError('Backup images must be immutable')
    if value['namespace']!='wd-'+value['projectID'] or type(value['volumeBytes']) is not int or value['volumeBytes']<=0:raise ValueError('Invalid media volume')
    return value


def raw(command,output=None,input_file=None,input_data=None):
    # No stdout/stderr from database commands may reach logs or the terminal.
    result=subprocess.run(['k3s','kubectl',*command],stdout=output or subprocess.PIPE,stdin=input_file,input=input_data,stderr=subprocess.PIPE,timeout=1800)
    if result.returncode:raise ValueError('Backup command failed; source remains fenced')
    return result.stdout


def sql(c,statement,database="postgres"):
    return raw(['exec','-i','-n','wd-db-'+c['backendID'],'postgres-1','--','psql','-U','postgres','-d',database,'-X','-A','-t','-v','ON_ERROR_STOP=1'],input_data=statement.encode()).decode().strip()


def identity(c):
    deployment=e.get('deployment','app-'+c['appID'],c['namespace']);pvc=e.get('pvc','app-'+c['appID']+'-data',c['namespace'])
    if not deployment or not pvc:raise ValueError('Backup source is missing')
    for obj in [deployment,pvc]:
        if any(obj['metadata'].get('labels',{}).get(k)!=v for k,v in e.labels(c).items()):raise ValueError('Backup ownership changed')
    if deployment['metadata']['uid']!=c['deploymentUID'] or pvc['metadata']['uid']!=c['pvcUID']:raise ValueError('Backup source identity changed')
    if deployment['spec']['template']['spec']['containers'][0]['image']!=c['image']:raise ValueError('Backup image changed')
    cluster=e.get('cluster','postgres','wd-db-'+c['backendID'])
    if not cluster or cluster['metadata']['uid']!=c['databaseClusterUID'] or cluster['metadata'].get('labels',{}).get('webdock.dev/database-installation')!=c['backendID']:raise ValueError('Database installation identity changed')
    marker=sql(c,"SELECT pg_catalog.shobj_description(oid,'pg_authid') FROM pg_catalog.pg_roles WHERE rolname='wd_app_"+c['databaseID']+"';")
    if marker!='webdock:database:'+c['databaseID']:raise ValueError('Database ownership changed')
    return deployment


def scale(c,deployment,replicas):
    patch=[{'op':'test','path':'/metadata/uid','value':c['deploymentUID']},{'op':'test','path':'/metadata/resourceVersion','value':deployment['metadata']['resourceVersion']},{'op':'replace','path':'/spec/replicas','value':replicas}]
    e.kubectl('patch','deployment','app-'+c['appID'],'-n',c['namespace'],'--type=json','-p',json.dumps(patch))


def fence(c):
    deployment=identity(c)
    sql(c,'ALTER ROLE "wd_app_'+c['databaseID']+'" NOLOGIN; ALTER ROLE "wd_migrate_'+c['databaseID']+'" NOLOGIN; SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname=\'db_'+c['databaseID']+'\' AND pid<>pg_backend_pid();')
    scale(c,deployment,0)
    deadline=time.monotonic()+120
    claim='app-'+c['appID']+'-data'
    while True:
        pods=json.loads(e.kubectl('get','pods','-n',c['namespace'],'-o','json'))['items']
        if not any(v.get('persistentVolumeClaim',{}).get('claimName')==claim for pod in pods for v in pod.get('spec',{}).get('volumes',[])):return deployment['spec']['replicas']
        if time.monotonic()>deadline:raise ValueError('Media volume is still in use; source remains fenced')
        time.sleep(1)


def helper(c,readonly):
    name='cms-backup-'+c['appID'];meta={'name':name,'namespace':c['namespace'],'labels':{**e.labels(c),'webdock.dev/backup':'true'}}
    if e.get('pod',name,c['namespace']):raise ValueError('Existing backup Pod requires operator reconciliation')
    resources={'cpu':'100m','memory':'134217728','ephemeral-storage':'67108864'};probe={'exec':{'command':['/bin/true']},'periodSeconds':10}
    pod={'apiVersion':'v1','kind':'Pod','metadata':meta,'spec':{'restartPolicy':'Never','activeDeadlineSeconds':1800,'automountServiceAccountToken':False,
      'securityContext':{'runAsNonRoot':True,'runAsUser':65532,'runAsGroup':65532,'fsGroup':65532,'seccompProfile':{'type':'RuntimeDefault'}},
      'containers':[{'name':'backup','image':c['helperImage'],'command':['sleep','1800'],'resources':{'requests':resources,'limits':resources},
        'securityContext':{'readOnlyRootFilesystem':True,'allowPrivilegeEscalation':False,'capabilities':{'drop':['ALL']}},'startupProbe':probe,'livenessProbe':probe,'readinessProbe':probe,
        'volumeMounts':[{'name':'data','mountPath':'/data','readOnly':readonly},{'name':'tmp','mountPath':'/tmp'}]}],
      'volumes':[{'name':'data','persistentVolumeClaim':{'claimName':'app-'+c['appID']+'-data','readOnly':readonly}},{'name':'tmp','emptyDir':{'sizeLimit':'67108864'}}]}}
    e.kubectl('create','-f','-',obj=pod)
    e.kubectl('wait','--for=condition=Ready','pod/'+name,'-n',c['namespace'],'--timeout=120s',timeout=125)
    return name


def delete_helper(c,name):
    pod=e.get('pod',name,c['namespace'])
    if not pod or any(pod['metadata'].get('labels',{}).get(k)!=v for k,v in e.labels(c).items()) or pod['metadata'].get('labels',{}).get('webdock.dev/backup')!='true':raise ValueError('Backup Pod ownership changed')
    e.kubectl('delete','--raw','/api/v1/namespaces/'+c['namespace']+'/pods/'+name,'-f','-',obj={'apiVersion':'v1','kind':'DeleteOptions','preconditions':{'uid':pod['metadata']['uid'],'resourceVersion':pod['metadata']['resourceVersion']}})
    e.kubectl('wait','--for=delete','pod/'+name,'-n',c['namespace'],'--timeout=120s',timeout=125)


def fingerprint(c,output):
    raw(['exec','-i','-n','wd-db-'+c['backendID'],'postgres-1','--','psql','-U','postgres','-d','db_'+c['databaseID'],'-X','-A','-t','-v','ON_ERROR_STOP=1'],output=output,input_data=COUNTS_SQL.encode())


def backup(c,destination):
    mount=subprocess.run(['findmnt','-n','-o','FSTYPE','-T',str(destination)],capture_output=True,text=True,check=True).stdout.strip()
    if mount not in ['nfs','nfs4','cifs','fuse.sshfs']:raise ValueError('Use an existing separately hosted NFS/CIFS/SSHFS backup destination')
    db_size=int(sql(c,"SELECT pg_database_size('db_"+c['databaseID']+"');"))
    if shutil.disk_usage(destination).free<2*(c['volumeBytes']+db_size):raise ValueError('Insufficient separately hosted backup capacity')
    directory=destination/(datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%SZ')+'-'+c['appID']);directory.mkdir(mode=0o700)
    # Persist the original fence state before changing either role, including mixed
    # LOGIN/NOLOGIN states, so an interrupted backup can be reconciled precisely.
    role_names=['wd_app_'+c['databaseID'],'wd_migrate_'+c['databaseID']]
    roles=json.loads(sql(c,"SELECT json_object_agg(rolname,rolcanlogin) FROM pg_catalog.pg_roles WHERE rolname IN ('"+"','".join(role_names)+"');"))
    if not isinstance(roles,dict) or set(roles)!=set(role_names) or any(type(value) is not bool for value in roles.values()):raise ValueError('Both original database role states are required')
    with (directory/'fence.json').open('x') as stream:
        json.dump({'roles':roles},stream);stream.flush();os.fsync(stream.fileno())
    directory_fd=os.open(directory,os.O_DIRECTORY)
    try:os.fsync(directory_fd)
    finally:os.close(directory_fd)
    replicas=fence(c);pod=helper(c,True)
    with (directory/'database.dump').open('xb') as stream:raw(['exec','-n','wd-db-'+c['backendID'],'postgres-1','--','pg_dump','-U','postgres','-d','db_'+c['databaseID'],'--format=custom','--schema=app','--no-owner','--no-privileges'],output=stream)
    with (directory/'database-fingerprint.txt').open('xb') as stream:fingerprint(c,stream)
    with (directory/'media.tar').open('xb') as stream:raw(['exec','-n',c['namespace'],pod,'--','tar','-C','/data','-cf','-','media'],output=stream)
    manifest={'version':1,'schemaVersion':json.loads(sql(c,"SELECT coalesce(json_agg(name ORDER BY name),'[]'::json) FROM app.payload_migrations;",'db_'+c['databaseID'])),'createdAt':datetime.now(timezone.utc).isoformat(),'source':c,'schema':'app','files':{name:sha256(directory/name) for name in ['database.dump','database-fingerprint.txt','media.tar']},'media':inspect_media(directory/'media.tar')}
    (directory/'manifest.json').write_text(json.dumps(manifest,sort_keys=True))
    for path in directory.iterdir():
        with path.open('rb') as stream:os.fsync(stream.fileno())
    verify_set(directory);delete_helper(c,pod)
    # Resume only the exact deployment that was fenced, after both artifacts verify.
    deployment=identity(c)
    if deployment['spec']['replicas']!=0:raise ValueError('Application changed during backup')
    # Restore both role flags atomically only after verified backup completion.
    sql(c,'BEGIN; '+''.join('ALTER ROLE "'+name+'" '+('LOGIN' if roles[name] else 'NOLOGIN')+'; ' for name in role_names)+'COMMIT;')
    try:scale(c,deployment,replicas)
    except Exception:
        sql(c,''.join('ALTER ROLE "'+name+'" NOLOGIN; ' for name in role_names)+"SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname='db_"+c['databaseID']+"' AND pid<>pg_backend_pid();")
        raise
    return directory


def restore(c,directory):
    manifest=verify_set(directory);assert_isolated(manifest['source'],c)
    if c['image']!=manifest['source']['image']:raise ValueError('Restore must use the recorded CMS image')
    identity(c)
    if sql(c,"SELECT count(*) FROM pg_catalog.pg_class c JOIN pg_catalog.pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='app' AND c.relkind IN ('r','p','S','v','m','f');",'db_'+c['databaseID'])!='0':raise ValueError('Restore requires a fresh unmigrated database with an empty app schema')
    fence(c);pod=helper(c,False)
    # A fresh isolated volume and DB are mandatory. Never replace production media.
    if raw(['exec','-n',c['namespace'],pod,'--','find','/data/media','-mindepth','1','-print','-quit']).strip():raise ValueError('Restore volume is not empty')
    migration='wd_migrate_'+c['databaseID'];database='db_'+c['databaseID']
    sql(c,f'GRANT CREATE ON DATABASE "{database}" TO "{migration}";')
    with (directory/'database.dump').open('rb') as stream:raw(['exec','-i','-n','wd-db-'+c['backendID'],'postgres-1','--','pg_restore','-U','postgres','--role='+migration,'--dbname='+database,'--clean','--if-exists','--no-owner','--no-privileges','--exit-on-error'],input_file=stream)
    sql(c,f'REVOKE CREATE ON DATABASE "{database}" FROM "{migration}";')
    with (directory/'media.tar').open('rb') as stream:raw(['exec','-i','-n',c['namespace'],pod,'--','tar','-C','/data','--no-same-owner','-xf','-'],input_file=stream)
    # Verify actual restored bytes and rows, not only input archive hashes.
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        media=Path(tmp)/'media.tar';counts=Path(tmp)/'database-fingerprint.txt'
        with media.open('wb') as stream:raw(['exec','-n',c['namespace'],pod,'--','tar','-C','/data','-cf','-','media'],output=stream)
        with counts.open('wb') as stream:fingerprint(c,stream)
        if inspect_media(media)!=manifest['media'] or sha256(counts)!=manifest['files']['database-fingerprint.txt']:raise ValueError('Restored DB/media verification failed')
    app='wd_app_'+c['databaseID']
    sql(c,f'GRANT USAGE ON SCHEMA app TO "{app}"; GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA app TO "{app}"; GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA app TO "{app}"; ALTER DEFAULT PRIVILEGES FOR ROLE "{migration}" IN SCHEMA app GRANT SELECT,INSERT,UPDATE,DELETE ON TABLES TO "{app}"; ALTER DEFAULT PRIVILEGES FOR ROLE "{migration}" IN SCHEMA app GRANT USAGE,SELECT ON SEQUENCES TO "{app}"; ALTER DEFAULT PRIVILEGES FOR ROLE "{migration}" IN SCHEMA app REVOKE EXECUTE ON FUNCTIONS FROM PUBLIC;',database)
    delete_helper(c,pod)
    # Keep the restored instance isolated and fenced until an operator reviews it.
    return {'verified':True,'databaseID':c['databaseID'],'appID':c['appID'],'image':c['image'],'fenced':True}


def retention_plan(destination,app_id):
    sets=[]
    for path in destination.iterdir():
        if path.is_symlink() or not path.is_dir() or not re.fullmatch(r'[0-9]{8}T[0-9]{6}Z-'+re.escape(app_id),path.name):continue
        try:
            manifest=verify_set(path)
            if manifest['source']['appID']!=app_id:continue
            timestamp=datetime.fromisoformat(manifest['createdAt'])
            if timestamp.tzinfo is None:continue
            sets.append((timestamp,path))
        except (ValueError,KeyError,OSError,tarfile.TarError):continue
    keep=set();days=set();weeks=set()
    for timestamp,path in sorted(sets,reverse=True):
        day=timestamp.date();week=timestamp.isocalendar()[:2]
        if day not in days and len(days)<7:days.add(day);keep.add(path)
        if week not in weeks and len(weeks)<4:weeks.add(week);keep.add(path)
    return [path for _,path in sets if path not in keep]


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('action',choices=['backup','restore']);parser.add_argument('--config',type=Path,required=True);parser.add_argument('--directory',type=Path,required=True);parser.add_argument('--prune',action='store_true',help='After a verified backup, retain seven daily and four weekly sets for this app only')
    args=parser.parse_args();c=configuration(args.config)
    with open(os.environ.get('SPITZLI_EXECUTOR_LOCK','/run/lock/webdock-managed-executor.lock'),'a') as lock:
        fcntl.flock(lock,fcntl.LOCK_EX)
        result=backup(c,args.directory) if args.action=='backup' else restore(c,args.directory)
        if args.prune:
            if args.action!='backup':raise ValueError('Retention applies only after a successful backup')
            for expired in retention_plan(args.directory,c['appID']):shutil.rmtree(expired)
    print(json.dumps({'backup':str(result)} if isinstance(result,Path) else result))

if __name__=='__main__':main()
