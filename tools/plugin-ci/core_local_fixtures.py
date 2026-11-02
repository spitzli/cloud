#!/usr/bin/env python3
"""Owned PostgreSQL/website acceptance inside the qualified manual build, never a supplied URL."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import signal
import socket
import stat
import subprocess
import tempfile
import time
import urllib.request
from urllib.parse import urlparse
import uuid
import core_tooling

PG=Path('/usr/lib/postgresql/18/bin')
ROOT=Path('/src')


def require(value,message):
    if not value:raise ValueError(message)


def clean_environment():
    return {k:v for k,v in os.environ.items() if k in {'PATH','HOME','USER','LANG','CI','TMPDIR','NODE_OPTIONS','BUN_INSTALL','NEXT_TELEMETRY_DISABLED'}}


def assert_owned_url(url):
    p=urlparse(url)
    import re
    require(p.scheme=='postgresql' and p.hostname=='127.0.0.1' and p.username=='quality' and p.port is not None and not p.query and re.fullmatch(r'/owned_website_[a-f0-9]{32}',p.path),'Only internally generated local owned website databases are accepted')


def verify_migration(before,after,future,permissions_before,permissions_after):
    require(before==after,'Migration changed an existing native/imported row')
    require(future['privacy_version']=='2026-11-01' and future['version']==1,'New intake did not receive the current notice default')
    require(permissions_before==permissions_after,'Migration changed existing ownership/RLS/permissions')


class LocalPostgres:
    def __init__(self,output):
        self.output=output;self.root=None;self.password=secrets.token_hex(32);self.env=clean_environment()
    def command(self,args,**kw):
        return subprocess.run(args,env=self.env,capture_output=True,text=True,check=True,timeout=60,**kw)
    def __enter__(self):
        require(os.getuid()==1001 and (PG/'initdb').is_file(),'Qualified nonroot PostgreSQL18 fixture required')
        self.root=Path(tempfile.mkdtemp(prefix='webdock-owned-gate-'));self.root.chmod(0o700)
        self.data=self.root/'data';self.sock=self.root/'socket';self.sock.mkdir(mode=0o700)
        self.marker=self.root/'owner';self.marker.write_text(uuid.uuid4().hex);self.marker.chmod(0o600)
        password=self.root/'password';password.write_text(self.password+'\n');password.chmod(0o600)
        with socket.socket() as s:s.bind(('127.0.0.1',0));self.port=s.getsockname()[1]
        self.env.update({'PGHOST':str(self.sock),'PGPORT':str(self.port),'PGUSER':'quality','PGDATABASE':'postgres'})
        try:
            self.command([str(PG/'initdb'),'-D',str(self.data),'--username=quality','--auth-local=trust','--auth-host=scram-sha-256','--pwfile='+str(password),'--encoding=UTF8','--no-locale'])
            self.command([str(PG/'pg_ctl'),'-D',str(self.data),'-l',str(self.root/'postgres.log'),'-o',f'-h 127.0.0.1 -p {self.port} -k {self.sock} -c max_connections=20 -c shared_buffers=32MB','-w','start'])
            require(self.query('SHOW data_directory;')==str(self.data),'PostgreSQL fixture does not own the expected data root')
            require((self.data/'PG_VERSION').read_text().strip()=='18','Unexpected PostgreSQL fixture major')
        except Exception:
            self.close();raise
        return self
    def query(self,sql,database='postgres'):
        r=self.command([str(PG/'psql'),'-X','--no-psqlrc','-v','ON_ERROR_STOP=1','-A','-t','-d',database],input=sql)
        return r.stdout.strip()
    def script(self,path,database):
        self.command([str(PG/'psql'),'-X','--no-psqlrc','-v','ON_ERROR_STOP=1','-d',database,'-f',str(path)])
    def close(self):
        if self.root is None:return
        root=self.root
        if (self.data/'postmaster.pid').exists():self.command([str(PG/'pg_ctl'),'-D',str(self.data),'-m','fast','-w','stop'])
        require(root.parent==Path(tempfile.gettempdir()) and root.name.startswith('webdock-owned-gate-') and root.resolve()==root and root.stat().st_uid==1001 and stat.S_IMODE(root.stat().st_mode)==0o700 and self.marker.is_file() and not self.marker.is_symlink(),'Refusing unowned PostgreSQL fixture cleanup')
        require(not (self.data/'postmaster.pid').exists(),'Fixture PostgreSQL still active')
        shutil.rmtree(root);require(not root.exists(),'Owned fixture root still exists after cleanup');self.root=None
    def __exit__(self,*args):self.close()


def migration_gate(pg):
    name='owned_notice_'+uuid.uuid4().hex;pg.query('CREATE DATABASE '+name)
    pg.query('''CREATE ROLE webdock_platform_owner NOLOGIN; CREATE ROLE webdock_auth_runtime NOLOGIN; CREATE ROLE webdock_auth_bootstrap NOLOGIN;
CREATE SCHEMA webdock_auth; CREATE SCHEMA webdock_rls; CREATE SCHEMA webdock_live;
GRANT USAGE ON SCHEMA webdock_auth,webdock_rls,webdock_live TO webdock_platform_owner,webdock_auth_runtime,webdock_auth_bootstrap;
CREATE FUNCTION webdock_rls.operator() RETURNS boolean LANGUAGE sql AS 'SELECT false';
CREATE FUNCTION webdock_live.invalidate_directory() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RETURN NULL; END';
CREATE TABLE webdock_auth."rateLimit"(id text PRIMARY KEY,key text UNIQUE NOT NULL,count integer NOT NULL,"lastRequest" bigint NOT NULL);
ALTER TABLE webdock_auth."rateLimit" OWNER TO webdock_platform_owner;''',name)
    original=ROOT/'crates/persistence/migrations/0068_waitlist.sql';migration=ROOT/'crates/persistence/migrations/0073_waitlist_notice_version.sql'
    pg.script(original,name)
    pg.query('''SET ROLE webdock_auth_bootstrap; SELECT webdock_auth.submit_waitlist('1','native@example.test','Owned native fixture'); RESET ROLE;
INSERT INTO webdock_auth.waitlist_entry(id,email,name,privacy_version) VALUES('2','imported@example.test','Owned imported fixture','legacy-import-notice');
INSERT INTO webdock_auth.waitlist_entry(id,email,name,status,privacy_version,reviewed_by,reviewed_at,version) VALUES('3','reviewed@example.test','Owned reviewed fixture','approved','2026-10-31','fixture-operator','2026-10-31T12:00:00Z',4);''',name)
    rows="SELECT jsonb_agg(to_jsonb(t) ORDER BY id)::text FROM webdock_auth.waitlist_entry t WHERE id IN ('1','2','3');"
    acl="SELECT jsonb_build_object('relationOID',c.oid,'owner',pg_get_userbyid(relowner),'rls',relrowsecurity,'forceRLS',relforcerowsecurity,'acl',relacl::text,'columnACL',(SELECT jsonb_agg(jsonb_build_array(attname,attacl::text) ORDER BY attnum) FROM pg_attribute WHERE attrelid=c.oid AND attnum>0),'policies',(SELECT jsonb_agg(to_jsonb(p) ORDER BY policyname) FROM pg_policies p WHERE schemaname='webdock_auth' AND tablename='waitlist_entry'))::text FROM pg_class c WHERE oid='webdock_auth.waitlist_entry'::regclass;"
    before=json.loads(pg.query(rows,name));permissions=pg.query(acl,name)
    pg.script(migration,name)
    after=json.loads(pg.query(rows,name));permissions_after=pg.query(acl,name)
    pg.query("SET ROLE webdock_auth_bootstrap; SELECT webdock_auth.submit_waitlist('4','future@example.test','Owned future fixture'); SELECT webdock_auth.submit_waitlist('5','native@example.test','Must not replace existing'); RESET ROLE;",name)
    future=json.loads(pg.query("SELECT to_jsonb(t)::text FROM webdock_auth.waitlist_entry t WHERE id='4';",name))
    verify_migration(before,after,future,permissions,permissions_after)
    require(json.loads(pg.query(rows,name))==before,'Duplicate intake changed a preexisting row')
    require(pg.query("SELECT count(*) FROM webdock_auth.waitlist_entry;",name)=='4','Duplicate intake created another row')
    return {'actualMigration0068SHA256':hashlib.sha256(original.read_bytes()).hexdigest(),'actualMigration0073SHA256':hashlib.sha256(migration.read_bytes()).hexdigest(),'existingNativeImportedReviewedRowsUnchanged':True,'existingPrivacyVersions':[r['privacy_version'] for r in before],'futurePrivacyVersion':future['privacy_version'],'ownershipRLSAndColumnPrivilegesUnchanged':True,'duplicateIntakeDidNotReplaceExistingRow':True,'acceptanceScope':'actual0068+0073 with owned minimal prerequisite roles/rate-limit/invalidation fixtures; not a full Core upgrade'}


def website_package_counts(raw):
    import re
    passed=re.search(r'\b(\d+) pass\b',raw);failed=re.search(r'\b(\d+) fail\b',raw);skipped=re.search(r'\b(\d+) skip(?:ped)?\b',raw)
    require(passed and failed and int(passed[1])>=8 and int(failed[1])==0,'Website package unit and HTTP tests must execute without failures')
    return {'passed':int(passed[1]),'failed':0,'skipped':int(skipped[1]) if skipped else 0}


def website_gate(pg,output):
    import re
    config_source=(ROOT/'apps/website/src/payload.config.ts').read_text()
    require(re.search(r'\bpush\s*:\s*false\b',config_source),'Actual website config must explicitly preserve schema push=false')
    name='owned_website_'+uuid.uuid4().hex;pg.query('CREATE DATABASE '+name)
    url=f'postgresql://quality:{pg.password}@127.0.0.1:{pg.port}/{name}';assert_owned_url(url)
    # Match install.ts: provision the website namespace before Payload's table migrations.
    pg.query('CREATE SCHEMA webdock AUTHORIZATION quality;',name)
    with socket.socket() as s:s.bind(('127.0.0.1',0));port=s.getsockname()[1]
    env=clean_environment();env.update({'TEST_BASE_URL':f'http://127.0.0.1:{port}','NEXT_PUBLIC_SERVER_URL':f'http://127.0.0.1:{port}','DATABASE_URL':url,'DATABASE_URL_UNPOOLED':url,'PAYLOAD_SECRET':secrets.token_hex(32),'PAYLOAD_MIGRATING':'true','WEBDOCK_OWNED_WEBSITE_FIXTURE':'1','WEBDOCK_SSO_ENFORCE':'true','OPERATOR_EMAIL':'owned-qualification@example.test','NEXT_TELEMETRY_DISABLED':'1','NODE_ENV':'production'})
    def run(label,args,cwd=ROOT):
        log=output/(label+'.log')
        with log.open('xb') as out:result=subprocess.run(args,cwd=cwd,env=env,stdout=out,stderr=subprocess.STDOUT,timeout=1800)
        log.chmod(0o600)
        if result.returncode:
            print(log.read_text(errors='replace')[-32768:].replace(pg.password,'[fixture-password-redacted]'));raise ValueError('Owned website gate failed: '+label)
    run('payload-migrate-seed',['bun','--no-env-file','/harness/website_fixture_seed.ts'])
    env.pop('PAYLOAD_MIGRATING')
    run('website-production-build',['bun','run','build'],ROOT/'apps/website')
    log=output/'website-server.log'
    with log.open('xb') as stream:
        log.chmod(0o600)
        proc=subprocess.Popen(['bun','--bun','../../node_modules/next/dist/bin/next','start','-H','127.0.0.1','-p',str(port)],cwd=ROOT/'apps/website',env=env,stdout=stream,stderr=subprocess.STDOUT,start_new_session=True)
        try:
            deadline=time.monotonic()+90
            while True:
                require(proc.poll() is None,'Owned production Next server exited')
                try:
                    with urllib.request.urlopen(env['TEST_BASE_URL'],timeout=2) as response:
                        if response.status==200:break
                except Exception:pass
                require(time.monotonic()<deadline,'Owned production Next server did not become ready');time.sleep(0.2)
            run('website-site-http',['bun','--no-env-file','test','apps/website/tests/site.test.mjs'])
            run('website-package-tests',['bun','--no-env-file','run','test'],ROOT/'apps/website')
        finally:
            if proc.poll() is None:os.killpg(proc.pid,signal.SIGTERM)
            try:proc.wait(timeout=20)
            except subprocess.TimeoutExpired:os.killpg(proc.pid,signal.SIGKILL);proc.wait(timeout=10)
    raw=(output/'website-site-http.log').read_text();require('1 pass' in raw and '0 fail' in raw,'Nonzero successful actual site.test required')
    package_counts=website_package_counts((output/'website-package-tests.log').read_text())
    return {'actualPayloadMigrationsAndOwnedSeedPassed':True,'productionNextBuildPassed':True,'productionNextHTTPTestPassed':True,'sitePassedTests':1,'websitePackageTestCounts':package_counts,'databaseAndHTTPWereOwnedLoopbackOnly':True,'schemaPushEnabled':False,'liveCMSOrDatabaseURLUsed':False,'NextBuildID':(ROOT/'apps/website/.next/BUILD_ID').read_text().strip()}


def recovery_url(port,password,database):
    import re
    require(1<=port<=65535 and re.fullmatch(r'[a-f0-9]{64}',password) and re.fullmatch(r'owned_neon_recovery_[a-f0-9]{32}_test',database),'Recovery fixture URL requires a new generated local owner database')
    return f'postgresql://neondb_owner:{password}@127.0.0.1:{port}/{database}'


def recovery_bootstrap_url(port,password,database):
    return recovery_url(port,password,database).replace('://neondb_owner:', '://quality:', 1)


def recovery_setup(password,database):
    recovery_url(54329,password,database)  # Validate generated identifiers/password, never accept a supplied DSN.
    return ["CREATE ROLE neondb_owner LOGIN NOSUPERUSER NOBYPASSRLS NOINHERIT NOCREATEDB NOCREATEROLE PASSWORD '"+password+"';", "CREATE DATABASE "+database+" OWNER quality;"]


def recovery_gate(pg,output):
    database='owned_neon_recovery_'+uuid.uuid4().hex+'_test';password=secrets.token_hex(32)
    for sql in recovery_setup(password,database):pg.query(sql)
    url=recovery_url(pg.port,password,database)
    bootstrap=recovery_bootstrap_url(pg.port,pg.password,database)
    flags_sql="SELECT jsonb_build_object('superuser',r.rolsuper,'bypassRLS',r.rolbypassrls,'inherit',r.rolinherit,'createDB',r.rolcreatedb,'createRole',r.rolcreaterole,'dbOwner',pg_get_userbyid(d.datdba))::text FROM pg_roles r,pg_database d WHERE r.rolname='neondb_owner' AND d.datname='"+database+"';"
    expected={'superuser':False,'bypassRLS':False,'inherit':False,'createDB':False,'createRole':False,'dbOwner':'quality'}
    role_before=json.loads(pg.query(flags_sql));require(role_before==expected,'Restricted recovery owner bootstrap is too privileged')
    env=clean_environment();env.update({'WEBDOCK_TEST_NEON_RECOVERY':'true','WEBDOCK_TEST_NEON_RECOVERY_DATABASE_URL':url,'WEBDOCK_TEST_NEON_RECOVERY_BOOTSTRAP_DATABASE_URL':bootstrap})
    test=ROOT/'tools/migrations/neon-enrollment-recovery.integration.test.ts';migration=ROOT/'crates/persistence/migrations/0074_neon_enrollment_recovery.sql'
    args=['bun','--no-env-file','test',str(test.relative_to(ROOT))];log=output/'neon-owner-recovery-pg18.log'
    with log.open('xb') as out:result=subprocess.run(args,env=env,stdout=out,stderr=subprocess.STDOUT,timeout=1800)
    log.chmod(0o600);raw=log.read_text(errors='replace')
    if result.returncode:
        print(raw[-65536:].replace(password,'[fixture-password-redacted]').replace(pg.password,'[bootstrap-password-redacted]'));raise ValueError('Owned PG18 Neon recovery acceptance failed')
    import re
    passed=re.search(r'\b(\d+) pass\b',raw);failed=re.search(r'\b(\d+) fail\b',raw)
    require(passed and failed and int(passed[1])==1 and int(failed[1])==0,'Exact real PG recovery test must execute, not skip')
    require(json.loads(pg.query(flags_sql))==expected,'Recovery test changed restricted owner powers or database ownership')
    return {'recoveryPGAcceptancePassed':True,'testCounts':{'passed':1,'failed':0},'acceptanceCommand':args,'actualMigration0074SHA256':hashlib.sha256(migration.read_bytes()).hexdigest(),'actualRecoveryAcceptanceSourceSHA256':hashlib.sha256(test.read_bytes()).hexdigest(),'fixtureOwnerRole':'neondb_owner','fixtureOwnerIsolatedSuperuser':False,'restrictedOwnerFlags':role_before,'bootstrapRole':'quality','bootstrapIsolatedSuperuser':True,'bootstrapAndOwnerShareOnlyGeneratedOwnedEndpoint':True,'providerHTTPWasOwnedLoopbackFakeOnly':True,'liveProviderEndpointOrAccountUsed':False,'externalDatabaseURLAccepted':False}


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--mode',choices=['migration','website','recovery'],required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--checkpoint',type=Path,required=True);a=p.parse_args()
    require(os.getuid()==1001 and Path.cwd().resolve()==ROOT,'Only isolated nonroot source fixture execution is allowed')
    require(not a.output.exists(),'New private fixture proof output required');a.output.mkdir(mode=0o700)
    tooling=core_tooling.verify(a.output)
    with LocalPostgres(a.output) as pg:
        version=pg.command([str(PG/'postgres'),'--version']).stdout.strip()
        result=migration_gate(pg) if a.mode=='migration' else recovery_gate(pg,a.output) if a.mode=='recovery' else website_gate(pg,a.output)
    proof={**json.loads(a.checkpoint.read_text()),'mode':a.mode,'ownedPostgresVersion':version,'tooling':tooling,'fixturePassed':True,'ownedDatabaseProcessStoppedAndRootRemoved':True,'productionDatabaseWritesPerformed':False,'productionActivationPerformed':False,**result}
    (a.output/'core-quality-proof.json').write_text(json.dumps(proof,indent=2)+'\n');print(json.dumps(proof))

if __name__=='__main__':
    try:main()
    except Exception as e:raise SystemExit(str(e)) from None
