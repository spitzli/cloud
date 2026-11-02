#!/usr/bin/env python3
"""Fresh, bounded native Core PostgreSQL backup authority fixture; no S3 server is started."""
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
import time
import urllib.parse

ROOT = Path(__file__).resolve().parents[3]
ROLES = ["webdock_auth_bootstrap", "webdock_auth_runtime", "webdock_worker_runtime",
         "webdock_admin_runtime", "webdock_registry_runtime", "webdock_runtime",
         "webdock_browser_website", "webdock_cms_archive_runtime", "webdock_offline_migrate"]


def diagnostics_directory(root):
    parent = Path("/tmp/webdock-backup-core-20261025")
    parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix="authority-", dir=parent))
    directory.chmod(0o700)
    return directory


def main():

    nonce = secrets.token_hex(12)
    name = f"webdock-backup-core-{nonce}"
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
        custody = docker("create", "--name", name, "--label", f"webdock.backup-core={nonce}",
                         "--env", "POSTGRES_PASSWORD", "--env", "POSTGRES_DB=webdock_rls_test",
                         "--memory", "1g", "--memory-swap", "1g", "--cpus", "2", "--pids-limit", "512", "--tmpfs", "/var/lib/postgresql:rw,nosuid,nodev,size=1g", "-p", "127.0.0.1::5432", image)
        record = {"containerId": custody, "name": name, "runId": nonce, "image": image}
        (directory / "custody.json").write_text(json.dumps(record))
        docker("start", custody)
        for attempt in range(100):
            ready = subprocess.run(["docker", "exec", custody, "pg_isready", "-U", "postgres"], capture_output=True, timeout=5)
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
               "WEBDOCK_BACKUP_AUTHORITY_TEST": "yes"}
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
        # Existing Hosting fixtures exercise retained writers as well as native ID allocation.
        # Fresh schema-only bootstrap has no legacy Auth generator state; no customer data is seeded.
        docker("exec", custody, "psql", "-U", "postgres", "-d", "webdock_rls_test", "-v", "ON_ERROR_STOP=1", "-c", "INSERT INTO webdock_auth.snowflake_state(singleton) VALUES(true)")
        run("acceptance", ["cargo", "test", "--locked", "-p", "webdock-databases", "--test", "persistence", "--", "--include-ignored", "--test-threads=1"], 360)
        print("Fresh native DB migration, existing database lifecycle and backup session/revocation authority checks passed.", flush=True)
    finally:
        (directory / "passwords.json").unlink(missing_ok=True)
        if custody:
            actual = json.loads(docker("inspect", "--format", '{"id":{{json .Id}},"label":{{json (index .Config.Labels "webdock.backup-core")}}}', custody))
            assert actual["id"] == custody and actual["label"] == nonce
            docker("rm", "-f", custody)
            absent = subprocess.run(["docker", "inspect", custody], capture_output=True, text=True, timeout=15)
            assert absent.returncode == 1 and any(f"no such {kind}: {custody}" in absent.stderr.lower() for kind in ["object", "container"])
            print("Exact owned PostgreSQL fixture removed; other services unchanged.", flush=True)


if __name__ == "__main__":
    main()
