"""Disposable shared-to-dedicated logical migration, with a concurrent neighbor writer."""
import argparse
from datetime import datetime,timedelta,timezone
import json
import os
from pathlib import Path
import secrets
import socket
import sys
import threading
import time
sys.path.insert(0,'/tmp')
import databases as db


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--target-id',required=True);args=parser.parse_args()
    if os.environ.get('SPITZLI_DISPOSABLE_POSTGRES_TEST')!='yes' or socket.gethostname()!='postgres-pilot':raise SystemExit('Disposable node required')
    identity=db.identifier(args.target_id);namespace='wd-db-'+identity
    if db.get('namespace',namespace):raise SystemExit('Fresh target required; inspect any interrupted transfer separately')
    pool=json.loads(Path('/etc/webdock-postgres-storage.json').read_text());source=db.get('cluster','postgres','wd-db-126')
    node=db.get('node','postgres-pilot');address=next(a['address'] for a in node['status']['addresses'] if a['type']=='InternalIP')
    host='db-'+identity+'.example.invalid'
    with open('/etc/hosts','a') as file:file.write('\n'+address+' '+host+'\n')
    used={port['nodePort'] for service in json.loads(db.kubectl('get','services','-A','-o','json'))['items'] for port in service['spec'].get('ports',[]) if port.get('nodePort')}
    ports=[port for port in range(32000,32200) if port not in used][:2]
    if len(ports)!=2:raise SystemExit('No disposable NodePort pair available')
    p={'version':1,'operationID':identity+'01','generation':1,'leaseToken':'a'*64,'leaseUntil':'','clusterID':'456','action':'prepareMigration','phase':'apply','revision':2,
       'backend':{'id':identity,'kind':'dedicated','namespace':namespace,'node':'postgres-pilot','poolID':pool['poolID'],'revision':1,'cnpgUID':None,'resources':{'cpuMillicores':500,'memoryBytes':1073741824,'volumeBytes':10737418240},'host':host,'directPort':ports[0],'pooledPort':ports[1]},'database':None,'members':[]}
    def checkpoint(**value):
        if not value:return
        p['backend']['cnpgUID']=value['cnpgUID']
        if value['phase']=='applied':p['phase']='configure'
        if value['phase']=='observing':p['phase']='observe'
        if value.get('migrationJobUID'):p['migration']['jobUID']=value['migrationJobUID']
        if value.get('copySHA256'):p['migration']['copySHA256']=value['copySHA256']
    path=Path('/tmp/migration-probe-'+identity+'.json')
    def converge():
        end=time.monotonic()+240
        while time.monotonic()<end:
            p['leaseUntil']=(datetime.now(timezone.utc)+timedelta(minutes=4)).isoformat()
            path.write_text(json.dumps(p));path.chmod(0o600)
            result=db.execute_database(p,checkpoint)
            if result:return result
            time.sleep(1)
        raise RuntimeError('Migration did not converge; inspect the retained source and unique Job')
    converge()
    p['action']='migrate';p['phase']='apply';p['operationID']=identity+'02'
    p['database']={'id':'9300','kind':'dedicated','name':'db_9300','revision':2,'credentialRevision':2,'connectionLimit':4,'credentials':{'migrationPassword':secrets.token_urlsafe(32),'applicationPassword':secrets.token_urlsafe(32)}}
    p['members']=[{'id':'9300','projectID':'456','databaseName':'db_9300','enabled':True}]
    p['migration']={'id':identity+'50','source':{'id':'126','namespace':'wd-db-126','cnpgUID':source['metadata']['uid'],'host':'db-126.example.invalid','kind':'shared-pool'},'transferPassword':secrets.token_urlsafe(32),'jobUID':None,'copySHA256':None}
    source_packet={**p,'backend':{**p['backend'],'id':'126','namespace':'wd-db-126','cnpgUID':source['metadata']['uid']}}
    db.sql(source_packet,"SET ROLE wd_migrate_9300; CREATE TABLE IF NOT EXISTS app.migration_payload AS SELECT i, repeat('synthetic-transfer-',20) AS value FROM generate_series(1,5000) i;",'db_9300')
    db.sql(source_packet,'CREATE TABLE IF NOT EXISTS app.migration_canary(n integer); TRUNCATE app.migration_canary; INSERT INTO app.migration_canary VALUES(0); GRANT SELECT,UPDATE ON app.migration_canary TO wd_app_9301;','db_9301')
    neighbor=secrets.token_urlsafe(32);db.sql(source_packet,'ALTER ROLE wd_app_9301 LOGIN PASSWORD \''+db.scram_secret(neighbor)+'\';')
    stopped=threading.Event();writes=[];failures=[]
    def writer():
        while not stopped.is_set():
            try:
                connection='host=db-126.example.invalid hostaddr=127.0.0.1 dbname=db_9301 user=wd_app_9301 sslmode=verify-full sslrootcert=/controller/certificates/server-ca.crt'
                result=db.kubectl('exec','-i','-n','wd-db-126','postgres-1','-c','postgres','--','sh','-c','IFS= read -r PGPASSWORD; export PGPASSWORD; exec psql "$@"','--',connection,'-X','-At','-v','ON_ERROR_STOP=1','-c','UPDATE app.migration_canary SET n=n+1; SELECT n FROM app.migration_canary;',source=neighbor+'\n')
                writes.append(result)
            except Exception:failures.append('neighbor unavailable')
            stopped.wait(.5)
    thread=threading.Thread(target=writer);thread.start()
    try:proof=converge()
    finally:stopped.set();thread.join(timeout=30)
    assert writes and not failures,'Neighbor was interrupted'
    assert db.sql(p,'SELECT count(*) FROM app.migration_payload','db_9300')=='5000'
    assert db.sql(source_packet,"SELECT count(*) FROM pg_roles WHERE rolname IN ('wd_migrate_9300','wd_app_9300') AND rolcanlogin")=='0'
    assert db.sql(source_packet,'SELECT count(*) FROM app.migration_payload','db_9300')=='5000'
    print(json.dumps({'sharedToDedicated':True,'sourceRetainedAndFenced':True,'rows':5000,'neighborWrites':len(writes),'neighborFailures':len(failures),'proof':proof}),flush=True)


if __name__=='__main__':main()
