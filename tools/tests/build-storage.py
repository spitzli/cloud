#!/usr/bin/env python3
"""Fresh owned native DB fixture for local/remote build publication and rollback acceptance."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
import time
import urllib.parse

ROOT = Path(__file__).resolve().parents[2]
ROLES = ["webdock_auth_bootstrap", "webdock_auth_runtime", "webdock_worker_runtime",
         "webdock_admin_runtime", "webdock_registry_runtime", "webdock_runtime",
         "webdock_browser_website", "webdock_cms_archive_runtime", "webdock_offline_migrate"]


def diagnostics_directory(root):
    parent = root / ".superpowers/sdd/2026-10-25-s3-managed-storage"
    parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix="build-s3-", dir=parent))
    directory.chmod(0o700)
    return directory


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    selector = parser.add_mutually_exclusive_group()
    selector.add_argument("--remote-only", action="store_true", help="Run only the remote build/publication acceptance after a targeted change")
    selector.add_argument("--frontend-only", action="store_true", help="Run only coordinated frontend native acceptance, after Mail0052/0053 freeze")
    selector.add_argument("--schema-only", action="store_true", help="Run only fresh compiled native migration/security smoke")
    selector.add_argument("--legacy-only", action="store_true", help="Run only ordinary Dockerfile/local legacy publication preservation")
    return parser.parse_args(argv)


def acceptance_plan(args):
    if args.schema_only:
        return None, 0
    if args.frontend_only:
        return "git_frontend_remote_outputs_cross_worker_publication_and_rollback", 900
    if args.legacy_only:
        return "git_build_publication_preserves_durable_authority_and_atomic_hosting", 360
    return "git_remote_artifact_cross_worker_publication_and_rollback" if args.remote_only else "git_", 360


def main():
    args = parse_args()
    assert args.schema_only or os.environ.get("WEBDOCK_S3_TEST_CONFIG"), "run inside the owned object-storage.py fixture"
    nonce = secrets.token_hex(12)
    name = f"webdock-build-s3-{nonce}"
    administrator = secrets.token_hex(32)
    passwords = {role: secrets.token_hex(32) for role in ROLES}
    image = "sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336"
    environment = {**os.environ, "POSTGRES_PASSWORD": administrator}
    custody = None

    def docker(*args):
        result = subprocess.run(["docker", *args], text=True, capture_output=True, env=environment, timeout=40)
        if result.returncode:
            raise RuntimeError(f"owned PostgreSQL fixture operation failed: {args[0]}")
        return result.stdout.strip()

    directory = diagnostics_directory(ROOT)
    try:
        custody = docker("create", "--name", name, "--label", f"webdock.build-storage={nonce}",
                         "--env", "POSTGRES_PASSWORD", "--env", "POSTGRES_DB=webdock_rls_test",
                         "--tmpfs", "/var/lib/postgresql:rw,nosuid,nodev,size=1g", "-p", "127.0.0.1::5432", image)
        record = {"containerId": custody, "name": name, "runId": nonce, "image": image}
        (directory / "custody.json").write_text(json.dumps(record))
        docker("start", custody)
        for attempt in range(100):
            ready = subprocess.run(["docker", "exec", custody, "psql", "-X", "-qAt", "-U", "postgres", "-d", "webdock_rls_test", "-c", "SELECT 1"], capture_output=True, timeout=5)
            if ready.returncode == 0:
                break
            time.sleep(0.1)
        else:
            raise RuntimeError("owned PostgreSQL fixture readiness timed out")
        address = docker("port", custody, "5432/tcp")
        assert address.startswith("127.0.0.1:")

        def url(role, password):
            return f"postgresql://{role}:{urllib.parse.quote(password, safe='')}@{address}/webdock_rls_test"

        env = {**os.environ, "WEBDOCK_MIGRATION_DATABASE_URL": url("postgres", administrator),
               "WEBDOCK_TEST_MIGRATION_DATABASE_URL": url("postgres", administrator),
               "DATABASE_URL": url("webdock_auth_runtime", passwords["webdock_auth_runtime"]),
               "AUTH_DATABASE_URL": url("webdock_auth_bootstrap", passwords["webdock_auth_bootstrap"]),
               "WEBDOCK_PLUGIN_WORKER_DATABASE_URL": url("webdock_worker_runtime", passwords["webdock_worker_runtime"]),
               "DATABASE_RLS_SECRET": secrets.token_hex(32), "BETTER_AUTH_SECRET": secrets.token_hex(32),
               "WEBDOCK_BUILD_VM_TEST": "1", "WEBDOCK_BUILD_REGISTRY_TEST": "1"}
        password_file = directory / "passwords.json"
        password_file.write_text(json.dumps(passwords))
        password_file.chmod(0o600)
        sensitive = [administrator, *passwords.values(), env["DATABASE_RLS_SECRET"], env["BETTER_AUTH_SECRET"]]

        def run(title, command, timeout):
            log = directory / f"{title}.log"
            with log.open("wb") as output:
                log.chmod(0o600)
                result = subprocess.run(command, cwd=ROOT, env=env, stdout=output, stderr=subprocess.STDOUT, timeout=timeout)
            tail = "\n".join(log.read_text().splitlines()[-32:])
            for value in sensitive:
                tail = tail.replace(value, "[redacted]")
            # Also protect full credential-bearing URLs, including generated test roles.
            import re
            tail = re.sub(r"postgres(?:ql)?://[^\s\"']+", "[redacted-dsn]", tail)
            print(tail, flush=True)
            if result.returncode:
                raise RuntimeError(f"{title} failed; private diagnostics retained at {directory}")

        run("migration", ["cargo", "run", "--locked", "-p", "webdock-control-plane", "--bin", "webdock-migrate", "--", "--target", f"{address}/webdock_rls_test", "--initialize-empty", "--provision-roles-from", str(password_file)], 420)
        def sql(statement):
            return docker("exec", custody, "psql", "-X", "-qAt", "-v", "ON_ERROR_STOP=1", "-U", "postgres", "-d", "webdock_rls_test", "-c", statement)

        journal = json.loads(sql("SELECT json_agg(json_build_object('version',version,'checksum',encode(checksum,'hex'),'success',success) ORDER BY version) FROM webdock_api_migrations.rust_journal"))
        migrations = sorted((ROOT / "crates/persistence/migrations").glob("*.sql"))
        expected = {int(path.name.split('_')[0]): hashlib.sha384(path.read_bytes()).hexdigest() for path in migrations}
        assert len(journal) == len(expected) and all(row['success'] and row['checksum'] == expected[row['version']] for row in journal)
        assert max(expected) == 53
        authority = json.loads(sql("SELECT json_build_object('owner',pg_get_userbyid(p.proowner),'securityDefiner',p.prosecdef,'config',p.proconfig,'body',p.prosrc,'publicExecute',EXISTS(SELECT 1 FROM aclexplode(coalesce(p.proacl,acldefault('f',p.proowner))) a WHERE a.grantee=0 AND a.privilege_type='EXECUTE'),'workerExecute',has_function_privilege('webdock_worker_runtime',p.oid,'EXECUTE'),'authExecute',has_function_privilege('webdock_auth_runtime',p.oid,'EXECUTE')) FROM pg_proc p WHERE p.oid='webdock_rls.git_release_authority(text)'::regprocedure"))
        assert authority['owner'] == 'webdock_platform_owner' and authority['securityDefiner']
        assert authority['config'] == ['search_path=pg_catalog']
        assert not authority['publicExecute'] and authority['workerExecute'] and not authority['authExecute']
        original = (ROOT / 'crates/persistence/migrations/0012_build_authority.sql').read_text().split('CREATE OR REPLACE FUNCTION webdock_rls.git_release_authority(',1)[1].split('$$',2)[1]
        assert authority['body'].replace("s.recipe NOT IN ('dockerfile','dockerfile-frontend')", "s.recipe<>'dockerfile'") == original
        expression = sql("SELECT pg_get_expr(conbin,conrelid) FROM pg_constraint WHERE conrelid='webdock_auth.git_source'::regclass AND conname='git_source_recipe_check'")
        recipes = json.loads(sql("SELECT json_object_agg(recipe,allowed) FROM (SELECT recipe,(" + expression + ") AS allowed FROM (VALUES ('dockerfile'),('vercel'),('dockerfile-frontend'),('unknown')) v(recipe)) checks"))
        assert recipes == {'dockerfile': True, 'vercel': True, 'dockerfile-frontend': True, 'unknown': False}
        assert sql("SET ROLE webdock_worker_runtime; SELECT webdock_rls.git_release_authority('1') IS NULL") == 't'
        smoke = {'journal': journal, 'recipeConstraint': recipes, 'authority': {k:v for k,v in authority.items() if k != 'body'}, 'unsignedWorkerDenied': True, 'migrationSHA256': {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in migrations}}
        (directory / 'schema-smoke.json').write_text(json.dumps(smoke,indent=2)+'\n')
        print(f"Fresh compiled native schema/security smoke GREEN; journal entries={len(journal)} maxVersion=53; original authority guards and ACL preserved.", flush=True)
        if args.schema_only:
            return
        # Existing Hosting fixtures exercise retained writers as well as native ID allocation.
        # Fresh schema-only bootstrap has no legacy Auth generator state; no customer data is seeded.
        docker("exec", custody, "psql", "-U", "postgres", "-d", "webdock_rls_test", "-v", "ON_ERROR_STOP=1", "-c", "INSERT INTO webdock_auth.snowflake_state(singleton) VALUES(true)")
        target, timeout = acceptance_plan(args)
        run("acceptance", ["cargo", "test", "--locked", "-p", "webdock-hosting", "--test", "postgres", target, "--", "--include-ignored", "--test-threads=1"], timeout)
        if args.frontend_only:
            print("Fresh native frontend admission, compatible claims, cross-worker S3 publication, output retention, rollback and terminal-limit queue checks passed.", flush=True)
        elif args.legacy_only:
            print("Fresh native ordinary Dockerfile/local legacy completion, publication and rollback checks passed.", flush=True)
        else:
            print("Fresh native DB migration, cross-worker S3 publication, signed source evidence, actual worker execution and rollback checks passed." if args.remote_only else "Fresh native DB migration, legacy local build, cross-worker S3 publication, signed source evidence and rollback checks passed.", flush=True)
    finally:
        (directory / "passwords.json").unlink(missing_ok=True)
        if custody:
            actual = json.loads(docker("inspect", "--format", '{"id":{{json .Id}},"label":{{json (index .Config.Labels "webdock.build-storage")}}}', custody))
            assert actual["id"] == custody and actual["label"] == nonce
            docker("rm", "-f", custody)
            absent = subprocess.run(["docker", "inspect", custody], capture_output=True, text=True, timeout=15)
            assert absent.returncode == 1 and any(f"no such {kind}: {custody}" in absent.stderr.lower() for kind in ["object", "container"])
            print("Exact owned PostgreSQL fixture removed; other services unchanged.", flush=True)


if __name__ == "__main__":
    main()
