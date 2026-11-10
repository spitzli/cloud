"""Explicit local-only two-customer SQL isolation probe; never use a production kubeconfig."""
import argparse
import base64
from datetime import datetime,timedelta,timezone
import json
import os
from pathlib import Path
import re
import secrets
import select
import subprocess
import sys
import tempfile
sys.path.insert(0,str(Path(__file__).resolve().parents[2]/'agents/kubernetes'))
from databases import sql_configuration,hba


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kubeconfig',required=True);parser.add_argument('--installation-id',required=True)
    args=parser.parse_args()
    if os.environ.get('SPITZLI_DISPOSABLE_POSTGRES_TEST')!='yes' or not re.fullmatch(r'[1-9][0-9]*',args.installation_id):raise SystemExit('Explicit disposable test confirmation required')
    command=['kubectl','--kubeconfig',args.kubeconfig]
    config=json.loads(subprocess.check_output(command+['config','view','--minify','-o','json']))
    if not re.match(r'^https://127\.0\.0\.1:\d+$',config['clusters'][0]['cluster']['server']):raise SystemExit('Only an explicitly selected loopback test cluster is accepted')
    ns='wd-db-'+args.installation_id
    def kube(*values,source=None):
        result=subprocess.run(command+list(values),input=source,capture_output=True,text=True,timeout=30)
        if result.returncode:raise RuntimeError('Disposable Kubernetes operation failed')
        return result.stdout.strip()
    def admin(sql,database='postgres'):
        return kube('exec','-i','-n',ns,'postgres-1','-c','postgres','--','psql','-U','postgres','-d',database,'-v','ON_ERROR_STOP=1','-At',source=sql)
    uid=json.loads(kube('get','cluster','postgres','-n',ns,'-o','json'))['metadata']['uid']
    members=[{'id':i,'projectID':'456','databaseName':'db_'+i,'enabled':True} for i in ['9300','9301']]
    packets=[]
    for member in members:
        p={'version':1,'operationID':'9901','generation':1,'leaseToken':'a'*64,'leaseUntil':(datetime.now(timezone.utc)+timedelta(minutes=4)).isoformat(),'clusterID':'456','action':'create','phase':'apply','revision':1,
          'backend':{'id':args.installation_id,'kind':'shared-pool','namespace':ns,'node':'postgres-pilot','poolID':'bd0ba8e4-e801-4c16-bf1b-060129437ccb','revision':1,'cnpgUID':uid,'resources':{'cpuMillicores':250,'memoryBytes':268435456,'volumeBytes':1073741824},'host':'db-'+args.installation_id+'.example.invalid','directPort':32000,'pooledPort':32001},
          'database':{'id':member['id'],'kind':'shared','name':member['databaseName'],'revision':1,'credentialRevision':1,'connectionLimit':4,'credentials':{'migrationPassword':secrets.token_urlsafe(32),'applicationPassword':secrets.token_urlsafe(32)}},'members':members}
        sql=sql_configuration(p);admin(sql['roles'])
        exists=admin("SELECT 1 FROM pg_database WHERE datname='"+member['databaseName']+"'")
        if not exists:admin(sql['create']);admin(sql['mark'])
        elif admin("SELECT shobj_description(oid,'pg_database') FROM pg_database WHERE datname='"+member['databaseName']+"'")!='webdock:database:'+member['id']:raise RuntimeError('Unowned database fixture')
        admin(sql['database']);admin(sql['schema'],member['databaseName']);admin(sql['enable']);packets.append(p)
    admin('REVOKE ALL ON DATABASE postgres,template0,template1 FROM PUBLIC;')
    kube('patch','cluster','postgres','-n',ns,'--type=merge','-p',json.dumps({'spec':{'postgresql':{'pg_hba':hba(packets[0])}}}))
    with tempfile.TemporaryDirectory(prefix='webdock-postgres-sql-') as temporary:
        ca=Path(temporary)/'ca.crt'
        public_ca=kube('get','secret','server-ca','-n',ns,'--ignore-not-found','-o','jsonpath={.data.ca\\.crt}')
        ca.write_bytes(base64.b64decode(public_ca or kube('get','secret','postgres-ca','-n',ns,'-o','jsonpath={.data.ca\\.crt}')))
        node=json.loads(kube('get','node','postgres-pilot','-o','json'))
        address=next(a['address'] for a in node['status']['addresses'] if a['type']=='InternalIP')
        import ipaddress
        if not ipaddress.ip_address(address).is_private:raise RuntimeError('Private disposable node required')
        existing=kube('get','service','public-direct','-n',ns,'--ignore-not-found','-o','json')
        if existing:
            port=str(json.loads(existing)['spec']['ports'][0]['nodePort'])
            pooled=str(json.loads(kube('get','service','public-pooled','-n',ns,'-o','json'))['spec']['ports'][0]['nodePort'])
        else:
            selector=json.loads(kube('get','service','postgres-rw','-n',ns,'-o','json'))['spec']['selector']
            service={'apiVersion':'v1','kind':'Service','metadata':{'name':'isolation-probe','namespace':ns},'spec':{'type':'NodePort','selector':selector,'ports':[{'port':5432,'targetPort':5432,'nodePort':32198}]}}
            kube('apply','-f','-',source=json.dumps(service));port='32198';pooled=None
        def query(index,sql,migration=False,database=None,host=None,pool=False):
            d=packets[index]['database'];role=('wd_migrate_' if migration else 'wd_app_')+d['id']
            env={**os.environ,'PGPASSWORD':d['credentials']['migrationPassword' if migration else 'applicationPassword']}
            connection=f"host={host or ('db-'+args.installation_id+'.example.invalid' if public_ca else 'postgres-rw.'+ns+'.svc')} hostaddr={address} port={pooled if pool else port} dbname={database or d['name']} user={role} sslmode=verify-full sslrootcert={ca} connect_timeout=5"
            return subprocess.run(['psql',connection,'-At','-v','ON_ERROR_STOP=1'],input=sql,capture_output=True,text=True,env=env,timeout=10)
        for i in range(2):
            migration=query(i,'CREATE TABLE IF NOT EXISTS app.isolation_proof(value text); TRUNCATE app.isolation_proof; INSERT INTO app.isolation_proof VALUES (\'customer-'+str(i)+'\');',migration=True)
            assert migration.returncode==0,'Migration access failed: '+migration.stderr[-500:]
            own=query(i,'SELECT value FROM app.isolation_proof;')
            assert own.stdout.strip()=='customer-'+str(i), 'Own data unavailable: '+own.stderr[-400:]
        if pooled:
            assert query(0,'SELECT value FROM app.isolation_proof;',pool=True).stdout.strip()=='customer-0','Pooled access failed'
            assert query(0,'SELECT 1;',database='db_9301',pool=True).returncode!=0,'Pooled cross-customer access succeeded'
        for other in ['db_9301','postgres','template1']:
            assert query(0,'SELECT 1;',database=other).returncode!=0,'Cross-database connection succeeded'
        for sql in ['SET ROLE wd_app_9301;','SET ROLE wd_owner_9300;','CREATE ROLE attacker;','CREATE DATABASE attacker;','CREATE EXTENSION dblink;','SELECT pg_read_file(\'/etc/passwd\');']:
            assert query(0,sql,migration=True).returncode!=0,'Privilege escalation succeeded'
        assert query(0,'SELECT 1;',host='wrong.example.invalid').returncode!=0,'TLS hostname mismatch accepted'
        admin(sql_configuration(packets[0])['disable'])
        assert query(0,'SELECT 1;').returncode!=0,'Paused role could connect'
        assert query(1,'SELECT value FROM app.isolation_proof;').stdout.strip()=='customer-1','Pause affected neighbor'
        admin(sql_configuration(packets[0])['enable'])
        assert query(0,'SELECT value FROM app.isolation_proof;').stdout.strip()=='customer-0','Resume lost data'
        print(json.dumps({'twoCustomers':True,'poolerVerified':bool(pooled),'ownMigrationAndAppRoles':True,'crossDatabaseDenied':True,'roleEscalationDenied':True,'serverFilesDenied':True,'tlsHostnameVerified':True,'pausePreservesNeighbor':True,'resumePreservesData':True}))


if __name__=='__main__':main()
