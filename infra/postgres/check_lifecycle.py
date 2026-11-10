"""Run inside the disposable postgres-pilot node; no production node is accepted."""
import argparse
from datetime import datetime,timedelta,timezone
import json
import os
from pathlib import Path
import secrets
import socket
import sys
import time
sys.path.insert(0,'/tmp')
import databases


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--installation-id',required=True);parser.add_argument('--resume-stage',choices=['create','pause','resume','restart','resize','rotateCredentials','delete','purge']);args=parser.parse_args()
    if os.environ.get('SPITZLI_DISPOSABLE_POSTGRES_TEST')!='yes' or socket.gethostname()!='postgres-pilot':raise SystemExit('Disposable test node required')
    identity=databases.identifier(args.installation_id);pool=json.loads(Path('/etc/webdock-postgres-storage.json').read_text());ns='wd-db-'+identity
    if databases.get('namespace',ns) and not args.resume_stage:raise SystemExit('Use a fresh synthetic installation ID; inspect any interrupted run separately')
    node=databases.get('node','postgres-pilot');address=next(a['address'] for a in node['status']['addresses'] if a['type']=='InternalIP')
    host='db-'+identity+'.example.invalid'
    with open('/etc/hosts','a') as stream:stream.write('\n'+address+' '+host+'\n')
    p={'version':1,'operationID':identity+'01','generation':1,'leaseToken':'a'*64,'leaseUntil':'','clusterID':'456','action':'create','phase':'apply','revision':1,
       'backend':{'id':identity,'kind':'dedicated','namespace':ns,'node':'postgres-pilot','poolID':pool['poolID'],'revision':1,'cnpgUID':None,'resources':{'cpuMillicores':500,'memoryBytes':1073741824,'volumeBytes':10737418240},'host':host,'directPort':32002,'pooledPort':32003},
       'database':{'id':identity+'10','kind':'dedicated','name':'db_'+identity+'10','revision':1,'credentialRevision':1,'connectionLimit':5,'credentials':{'migrationPassword':secrets.token_urlsafe(32),'applicationPassword':secrets.token_urlsafe(32)}},
       'members':[{'id':identity+'10','projectID':'456','databaseName':'db_'+identity+'10','enabled':True}]}
    path=Path('/tmp/database-lifecycle-'+identity+'.json')
    if args.resume_stage:
        p=json.loads(path.read_text())
        if p['action']!=args.resume_stage or p['phase']!='observe':raise SystemExit('Only observation of the inspected completed stage may be resumed')
        current=databases.get('cluster','postgres',ns)
        if current:databases.owned(current,p)
        elif args.resume_stage!='purge':raise SystemExit('Installation disappeared')
    def checkpoint(**value):
        if not value:return
        p['backend']['cnpgUID']=value['cnpgUID']
        if 'previousPodUID' in value:p['previousPodUID']=value['previousPodUID']
        if value['phase']=='applied':p['phase']='configure'
        elif value['phase']=='observing':p['phase']='observe'
    def run(action):
        phase=p['phase'] if args.resume_stage==action else 'apply'
        p['action']=action;p['phase']=phase;p['operationID']=identity+str(p['revision']).zfill(2)
        end=time.monotonic()+240
        while time.monotonic()<end:
            p['leaseUntil']=(datetime.now(timezone.utc)+timedelta(minutes=4)).isoformat()
            path.write_text(json.dumps(p));path.chmod(0o600)
            result=databases.execute_database(p,checkpoint)
            if result:
                print(json.dumps({'action':action,'proof':result}),flush=True)
                p['revision']+=1;p['database']['revision']=p['revision'];return result
            time.sleep(1)
        raise RuntimeError('Lifecycle stage did not converge: '+action)
    actions=['create','pause','resume','restart','resize','rotateCredentials','delete','purge']
    for action in actions:
        if args.resume_stage and actions.index(action)<actions.index(args.resume_stage):continue
        if action=='resize' and args.resume_stage!=action:
            p['backend']['resources'].update(cpuMillicores=750,memoryBytes=1342177280);p['backend']['revision']+=1
        if action=='rotateCredentials' and args.resume_stage!=action:
            p['database']['credentials']={'migrationPassword':secrets.token_urlsafe(32),'applicationPassword':secrets.token_urlsafe(32)};p['database']['credentialRevision']+=1
        run(action)
        if action=='create':databases.sql(p,'CREATE TABLE app.lifecycle(value text); INSERT INTO app.lifecycle VALUES (\'retained-sentinel\');',p['database']['name'])
        if action in ['resume','restart','resize','rotateCredentials']:
            assert databases.sql(p,'SELECT value FROM app.lifecycle',p['database']['name'])=='retained-sentinel'
    assert databases.get('namespace',ns) is None
    print(json.dumps({'dedicatedLifecycle':True,'persistentData':True,'pauseResume':True,'restart':True,'resize':True,'rotation':True,'retainedThenPurged':True}),flush=True)


if __name__=='__main__':main()
