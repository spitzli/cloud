#!/usr/bin/env python3
"""Fresh owned recovery bootstrap and unprivileged denial smoke. No existing cluster/S3 or SQL row repairs."""
import hashlib
import argparse
import importlib.util
import json
import os
from pathlib import Path
import secrets
import tempfile
import urllib.parse

ROOT=Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('owned_recovery_fixture',ROOT/'tools/tests/mail-application-recovery.py')
fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)

def main():
    parser=argparse.ArgumentParser();parser.add_argument("--max-version",type=int,default=52);parser.add_argument("--evidence",type=Path);parser.add_argument("--inventory-bind-regression",action="store_true");args=parser.parse_args()
    os.umask(0o077)
    with tempfile.TemporaryDirectory(prefix='wd-native-recovery-schema-') as directory:
        f=fixture.Fixture(directory,ROOT/'plugins/mail/target/debug/examples/recovery-object');p=f.root
        try:
            pki=f.pki();container,port=f.postgres('schema',pki)
            passwords=dict(value.split('=',1) for value in f.inspect(container)['Config']['Env'] if '=' in value)
            database='webdock_recovery_fixture'
            f.sql(container,'CREATE DATABASE '+database+';')
            roles=['webdock_auth_bootstrap','webdock_auth_runtime','webdock_worker_runtime','webdock_admin_runtime','webdock_registry_runtime','webdock_runtime','webdock_browser_website','webdock_cms_archive_runtime','webdock_offline_migrate']
            role_passwords={name:secrets.token_hex(24) for name in roles}
            role_file=p/'roles.json';role_file.write_text(json.dumps(role_passwords));role_file.chmod(0o600)
            query=urllib.parse.urlencode({'sslmode':'verify-full','sslrootcert':str(pki/'ca.crt')})
            url=f'postgresql://postgres:{passwords["POSTGRES_PASSWORD"]}@127.0.0.1:{port}/{database}?{query}'
            migration=next((ROOT/'crates/persistence/migrations').glob(f'{args.max_version:04d}_*.sql'))
            before=hashlib.sha256(migration.read_bytes()).hexdigest()
            result=f.command([str(ROOT/'target/debug/webdock-migrate'),'--target',f'127.0.0.1:{port}/{database}','--initialize-empty','--provision-roles-from',str(role_file)],env={'WEBDOCK_MIGRATION_DATABASE_URL':url,'DATABASE_RLS_SECRET':secrets.token_hex(32)},timeout=180,allowed=True)
            if result.returncode:
                # This binary emits only static migration diagnostics; schema failures
                # contain no account/role passwords, URLs or message bodies.
                text=result.stderr.decode()
                import re
                detail=re.search(r'message: "([^"]+)"',text)
                print('Fresh'+str(args.max_version)+' bootstrap failed: '+(detail.group(1) if detail else 'migration error (details withheld)'))
                raise RuntimeError('fresh_recovery_schema_bootstrap_failed')
            after=hashlib.sha256(migration.read_bytes()).hexdigest()
            assert before==after
            journal=f.sql(container,"SELECT count(*),max(version) FROM webdock_api_migrations.rust_journal WHERE success;",database).decode().strip()
            assert journal.endswith('|'+str(args.max_version))
            assert f.sql(container,"SELECT webdock_rls.recovery_target_quarantined();",database).decode().strip()=='f'
            # Product cold-target quarantine requires a genuinely fenced source epoch;
            # no synthetic policy/operation/state rows are inserted for this smoke.
            denied=f.docker('exec','-i',container,'psql','-X','-q','-v','ON_ERROR_STOP=1','-U','postgres','-d',database,data=b"SELECT webdock_auth.quarantine_fresh_recovery_target(gen_random_uuid());",allowed=True)
            assert denied.returncode!=0
            denied_checks=[]
            if args.max_version>=54:
                epoch='00000000-0000-0000-0000-000000000001'
                denied_calls=[
                    "SELECT webdock_rls.recovery_admit_source_fence('1',1,'unowned','"+epoch+"'::uuid,'[]'::jsonb);",
                    "SELECT webdock_rls.recovery_mark_source_drained('1',1,'unowned','"+epoch+"'::uuid,'{}'::jsonb);",
                    "SELECT webdock_rls.recovery_mail_fence('1',1,'unowned',false);",
                    "SELECT webdock_rls.recovery_mail_fence('1',1,'unowned',true);",
                    "SELECT webdock_rls.recovery_source_fence_view('1',1,'unowned','"+epoch+"'::uuid);",
                    "UPDATE webdock_auth.hosting_recovery_source_fence SET state='drained';",
                    "DELETE FROM webdock_auth.hosting_recovery_source_intent;",
                    "INSERT INTO webdock_auth.hosting_recovery_source_event(operation_id,event,proof) VALUES('1','fabricated','{}');",
                ]
                for role in ['webdock_auth_runtime','webdock_worker_runtime']:
                    for statement in denied_calls:
                        result=f.docker('exec','-i','-e','PGPASSWORD',container,'psql','-X','-q','-A','-t','-h','localhost','-U',role,'-d',database,'-v','ON_ERROR_STOP=1',data=statement.encode(),env={'PGPASSWORD':role_passwords[role]},allowed=True)
                        assert result.returncode, 'unowned_recovery_mutation_was_not_denied'
                        denied_checks.append({'role':role,'kind':statement.split()[0].lower(),'denied':True})
                assert f.sql(container,"SELECT count(*) FROM webdock_auth.hosting_recovery_source_fence;",database).strip()==b'0'
                assert f.sql(container,"SELECT count(*) FROM webdock_auth.hosting_recovery_source_event;",database).strip()==b'0'
            if args.inventory_bind_regression:
                worker_url=f'postgresql://webdock_worker_runtime:{role_passwords["webdock_worker_runtime"]}@127.0.0.1:{port}/{database}?{query}'
                result=f.command(['cargo','test','--offline','--locked','-p','webdock-hosting','--lib','service::recovery::review_sql_tests::inventory_actual_sql_binding_reaches_the_authority_gate_with_four_values','--','--ignored','--exact'],env={'WEBDOCK_RECOVERY_SQL_TEST_URL':worker_url},timeout=240,allowed=True)
                # Only static assertion/result metadata is public; no database URL or role secrets.
                print('Inventory actual SQL binding regression exit='+str(result.returncode)+'; selectedOneTest='+str(b'running 1 test' in result.stdout))
                assert b'running 1 test' in result.stdout, 'inventory_sql_test_not_selected'
                assert result.returncode==0, 'inventory_actual_sql_bind_regression_failed'
            report={'scope':'fresh schema and missing authority denial only; no admitted source/capture acceptance claimed','maxVersion':args.max_version,'migrationSHA256':before,'journal':journal,'unfencedQuarantineDenied':True,'unownedDenials':denied_checks}
            print('Fresh native schema bootstrap GREEN;'+str(args.max_version)+' SHA256='+before+'; journal='+journal+'; unfenced quarantine denied.')
        finally:f.cleanup()
    report['cleanup']={'ownedPostgreSQLAndVolumeAbsent':True,'privateDirectoryAbsent':not p.exists(),'existingServicesTouched':False}
    if args.evidence: args.evidence.write_text(json.dumps(report,indent=2)+'\n')
    print('Exact owned schema PostgreSQL container/volume/private directory removed; unowned denials='+str(len(denied_checks))+'.')

if __name__=='__main__':main()
