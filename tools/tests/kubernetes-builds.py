#!/usr/bin/env python3
"""Owned local PostgreSQL acceptance; live NEW build-cluster calls require --live-config."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import signal
import stat
import subprocess
import tempfile
import time
import urllib.parse

ROOT = Path(__file__).resolve().parents[2]
ROLES = ["webdock_auth_bootstrap", "webdock_auth_runtime", "webdock_worker_runtime",
         "webdock_admin_runtime", "webdock_registry_runtime", "webdock_runtime",
         "webdock_browser_website", "webdock_cms_archive_runtime", "webdock_offline_migrate"]
IMAGE = "sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336"


def live_cleanup_proven(path, container):
    try:
        metadata=path.lstat()
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink!=1 or metadata.st_uid!=os.getuid() or metadata.st_mode&0o077:
            return False
        proof=json.loads(path.read_text())
    except (OSError,ValueError,UnicodeError):
        return False
    return isinstance(proof,dict) and proof.get("containerID")==container and proof.get("allJobsAbsent") is True


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--live-config", type=Path, help="explicit private dedicated build-cluster acceptance configuration")
    parser.add_argument("--live-only", action="store_true", help="skip already-covered admission/webhook acceptance")
    parser.add_argument("--railpack", action="store_true", help="run the owned Railpack live acceptance instead of Dockerfile/cancellation")
    options = parser.parse_args(argv)
    if options.railpack and not options.live_config:
        parser.error("--railpack requires --live-config")
    if options.live_only and not options.live_config:
        parser.error("--live-only requires --live-config")
    live = None
    if options.live_config:
        options.live_config = options.live_config.resolve(strict=True)
        metadata = options.live_config.lstat()
        assert stat.S_ISREG(metadata.st_mode) and metadata.st_nlink == 1 and metadata.st_uid == os.getuid() and metadata.st_mode & 0o077 == 0
        live = json.loads(options.live_config.read_text())
        assert 30 <= live["timeoutSeconds"] <= 1800
    nonce = secrets.token_hex(12)
    administrator = secrets.token_hex(32)
    passwords = {role: secrets.token_hex(32) for role in ROLES}
    parent = ROOT / ".runtime-test" / "kubernetes-admission"
    parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix="owned-", dir=parent))
    directory.chmod(0o700)
    if live:
        preflight=directory/"live-preflight.log"
        with preflight.open("wb") as output:
            preflight.chmod(0o600)
            result=subprocess.run(["cargo","test","--locked","-p","spitzli-hosting","--test","postgres","kubernetes_live_configuration_preflight","--","--include-ignored","--test-threads=1"],cwd=ROOT,env={**os.environ,"WEBDOCK_KUBERNETES_LIVE_CONFIG":str(options.live_config)},stdout=output,stderr=subprocess.STDOUT,timeout=600)
        assert result.returncode==0 and re.search(r"test result: ok\. 1 passed; 0 failed",preflight.read_text()), "live policy/TLS preflight failed; no PostgreSQL container created; private log: "+str(preflight)
    environment = {**os.environ, "DOCKER_HOST": "unix:///var/run/docker.sock", "POSTGRES_PASSWORD": administrator}
    environment.pop("DOCKER_CONTEXT", None)
    custody = None
    live_started = False
    database_environment = None
    cleanup_proof = directory / "live-cleanup.json"

    def docker(*args):
        result = subprocess.run(["docker", *args], text=True, capture_output=True, env=environment, timeout=40)
        if result.returncode:
            raise RuntimeError(f"owned PostgreSQL fixture operation failed: {args[0]}")
        return result.stdout.strip()

    try:
        # Use an already-present pinned EU/local test image; never initiate a remote build or pull.
        docker("image", "inspect", IMAGE)
        custody = docker("create", "--name", f"webdock-kubernetes-admission-{nonce}",
                         "--label", f"webdock.kubernetes-admission={nonce}",
                         "--env", "POSTGRES_PASSWORD", "--env", "POSTGRES_DB=webdock_rls_test",
                         "--tmpfs", "/var/lib/postgresql:rw,nosuid,nodev,size=1g", "-p", "127.0.0.1::5432", IMAGE)
        (directory / "custody.json").write_text(json.dumps({"containerID": custody, "runID": nonce, "image": IMAGE}))
        (directory / "custody.json").chmod(0o600)
        cleanup_proof.write_text(json.dumps({"containerID":custody,"allJobsAbsent":True,"phase":"no-remote-effects"}))
        cleanup_proof.chmod(0o600)
        docker("start", custody)
        for _ in range(100):
            ready = subprocess.run(["docker", "exec", custody, "psql", "-X", "-qAt", "-U", "postgres", "-d", "webdock_rls_test", "-c", "SELECT 1"], capture_output=True, env=environment, timeout=5)
            if ready.returncode == 0:
                break
            time.sleep(0.1)
        else:
            raise RuntimeError("owned PostgreSQL fixture readiness timed out")
        address = docker("port", custody, "5432/tcp")
        assert address.startswith("127.0.0.1:") and "\n" not in address

        def url(role, password):
            return f"postgresql://{role}:{urllib.parse.quote(password, safe='')}@{address}/webdock_rls_test"

        env = {**environment, "WEBDOCK_MIGRATION_DATABASE_URL": url("postgres", administrator),
               "WEBDOCK_TEST_MIGRATION_DATABASE_URL": url("postgres", administrator),
               "DATABASE_URL": url("webdock_auth_runtime", passwords["webdock_auth_runtime"]),
               "AUTH_DATABASE_URL": url("webdock_auth_bootstrap", passwords["webdock_auth_bootstrap"]),
               "WEBDOCK_PLUGIN_WORKER_DATABASE_URL": url("webdock_worker_runtime", passwords["webdock_worker_runtime"]),
               "DATABASE_RLS_SECRET": secrets.token_hex(32), "BETTER_AUTH_SECRET": secrets.token_hex(32)}
        if live:
            env.update({"WEBDOCK_KUBERNETES_LIVE_CONFIG":str(options.live_config),
                        "WEBDOCK_KUBERNETES_LIVE_CUSTODY":str(directory/"custody.json"),
                        "WEBDOCK_KUBERNETES_LIVE_CLEANUP_PROOF":str(cleanup_proof)})
        database_environment = env
        sensitive = [administrator, *passwords.values(), env["DATABASE_RLS_SECRET"], env["BETTER_AUTH_SECRET"]]
        password_file = directory / "passwords.json"
        password_file.write_text(json.dumps(passwords))
        password_file.chmod(0o600)

        def run(title, command, timeout=600):
            log = directory / f"{title}.log"
            with log.open("wb") as output:
                log.chmod(0o600)
                if title == "live-build":
                    process = subprocess.Popen(command,cwd=ROOT,env=env,stdout=output,stderr=subprocess.STDOUT,start_new_session=True)
                    try:
                        returncode = process.wait(timeout=timeout)
                    except BaseException:
                        os.killpg(process.pid,signal.SIGTERM)
                        try: process.wait(timeout=5)
                        except subprocess.TimeoutExpired:
                            os.killpg(process.pid,signal.SIGKILL)
                            process.wait(timeout=5)
                        raise
                else:
                    returncode = subprocess.run(command, cwd=ROOT, env=env, stdout=output, stderr=subprocess.STDOUT, timeout=timeout).returncode
            tail = "\n".join(log.read_text().splitlines()[-28:])
            if title == "live-build":
                # Live provider diagnostics may contain presigned capabilities. Keep
                # them private; only test/result lines reach the terminal.
                tail = "\n".join(line for line in log.read_text().splitlines() if re.match(r"^(test build_kubernetes_live::|test result:|running [0-9]+ tests)",line))
            for secret in sensitive:
                tail = tail.replace(secret, "[redacted]")
            print(re.sub(r"postgres(?:ql)?://[^\s\"']+", "[redacted-dsn]", tail), flush=True)
            if returncode:
                diagnostic = subprocess.run(["docker", "logs", custody], env=environment, text=True, capture_output=True, timeout=15)
                errors = "\n".join(line for line in (diagnostic.stdout+diagnostic.stderr).splitlines() if "ERROR:" in line)
                for secret in sensitive:
                    errors = errors.replace(secret, "[redacted]")
                print(errors[-4000:], flush=True)
                raise RuntimeError(f"{title} failed; private diagnostics: {directory}")
            if title in {"admission","publication","github-webhook","live-build"}:
                assert re.search(r"test result: ok\. 1 passed; 0 failed", log.read_text()), "focused acceptance test did not run"

        migrate = ["cargo", "run", "--locked", "-p", "spitzli-control-plane", "--bin", "spitzli-migrate", "--", "--target", f"{address}/webdock_rls_test"]
        run("bootstrap", [*migrate, "--initialize-empty", "--provision-roles-from", str(password_file)])
        run("repeat-upgrade", migrate)

        def sql(statement):
            return docker("exec", custody, "psql", "-X", "-qAt", "-v", "ON_ERROR_STOP=1", "-U", "postgres", "-d", "webdock_rls_test", "-c", statement)

        migrations = sorted((ROOT / "crates/persistence/migrations").glob("*.sql"))
        expected = {int(path.name.split("_")[0]): hashlib.sha384(path.read_bytes()).hexdigest() for path in migrations}
        journal = json.loads(sql("SELECT json_agg(json_build_object('version',version,'checksum',encode(checksum,'hex'),'success',success) ORDER BY version) FROM webdock_api_migrations.rust_journal"))
        assert len(journal) == len(expected) and all(row["success"] and row["checksum"] == expected[row["version"]] for row in journal)
        assert 57 in expected
        assert sql("SELECT NOT enabled AND global_slots=1 AND tenant_slots=1 FROM webdock_auth.git_build_admission WHERE singleton") == "t"
        assert sql("SELECT count(*)=0 FROM webdock_auth.git_build_cluster") == "t"
        assert sql("SELECT count(*)=0 FROM webdock_auth.git_build_trusted_project") == "t"
        assert sql("SET ROLE webdock_worker_runtime; SELECT count(*)=0 FROM webdock_auth.git_build_admission") == "t"
        assert sql("SELECT has_table_privilege('webdock_worker_runtime','webdock_auth.git_build_attempt','UPDATE') AND NOT has_table_privilege('webdock_worker_runtime','webdock_auth.git_build_admission','UPDATE') AND NOT has_table_privilege('webdock_auth_runtime','webdock_auth.git_build_attempt','UPDATE')") == "t"
        # Retained fixture writers need their existing local ID checkpoint.
        sql("INSERT INTO webdock_auth.snowflake_state(singleton) VALUES(true)")
        if not options.live_only:
            run("admission", ["cargo", "test", "--locked", "-p", "spitzli-hosting", "--test", "postgres", "kubernetes_admission_reserves", "--", "--include-ignored", "--test-threads=1"])
            run("volume-authority", ["cargo", "test", "--locked", "-p", "spitzli-hosting", "--test", "postgres", "kubernetes_volume_authority_is_immutable", "--", "--include-ignored", "--test-threads=1"])
            run("fair-retry", ["cargo", "test", "--locked", "-p", "spitzli-hosting", "--test", "postgres", "kubernetes_fair_queue_and_retired_infrastructure_retry", "--", "--include-ignored", "--test-threads=1"])
            run("github-webhook", ["cargo", "test", "--locked", "-p", "spitzli-hosting", "--test", "postgres", "github_webhook_receipts_bind", "--", "--include-ignored", "--test-threads=1"])
            run("publication", ["cargo", "test", "--locked", "-p", "spitzli-hosting", "--test", "postgres", "kubernetes_publication_stages_fresh", "--", "--include-ignored", "--test-threads=1"])
        if live:
            live_started = True
            live_test = "kubernetes_live_railpack_retains" if options.railpack else "kubernetes_live_real_run"
            run("live-build", ["cargo","test","--locked","-p","spitzli-hosting","--test","postgres",live_test,"--","--include-ignored","--test-threads=1"],timeout=2*live["timeoutSeconds"]+360)
        checks=["compiled-bootstrap","repeat-upgrade","exact-journal-checksums","default-disabled","restricted-role-RLS"]
        if not options.live_only:
            checks += ["concurrent-capacity","uncertain-job-retention","UID-and-fingerprint-fences","immutable-request-snapshot","worker-router-authority","source-backend-selection","source-and-worker-revocation","redacted-durable-log-cursors","artifact-ready-completion-and-idempotency","cancelled-generation-rejection"]
            checks += ["fresh-manual-publication-hosting-handoff", "registry-success-after-operation-staging", "rollout-failure-independent-of-build", "expired-publication-receipt-replay"]
        if live:
            proof=json.loads(cleanup_proof.read_text())
            assert live_cleanup_proven(cleanup_proof,custody) and proof.get("evidenceDirectory")
            checks += ["real-owned-Git-commit-and-archive","pinned-real-rootless-runner","TLS-source-result-transfer","durable-live-log-redaction","Job-Pod-handoff-cleanup","artifact-ready-not-registry-success"]
            checks += ["actual-Railpack-prepare-and-pinned-frontend", "actual-Node-build-OCI-proof", "proxy-not-in-final-image-config"] if options.railpack else ["actual-Dockerfile-RUN-OCI-layer", "actual-RUN-cancellation"]
        (directory / "result.json").write_text(json.dumps({"passed": True, "journalVersions": sorted(expected), "checks": checks}, indent=2)+"\n")
        if live:
            print("Owned live build acceptance passed; private logs and cleanup proof: " + str(directory),flush=True)
        else:
            print("Owned build admission, publication and signed GitHub fixtures passed; no provider/Kubernetes API or VM calls.", flush=True)
    finally:
        if custody:
            if live_started:
                if not live_cleanup_proven(cleanup_proof,custody):
                    # Never destroy the only reconciliation ledger while a live
                    # Job/Secret might remain. This container is still owned/local.
                    recovery=directory/"private-recovery-env.json"
                    recovery.write_text(json.dumps({key:value for key,value in (database_environment or {}).items() if key in {"WEBDOCK_TEST_MIGRATION_DATABASE_URL","DATABASE_URL","AUTH_DATABASE_URL","WEBDOCK_PLUGIN_WORKER_DATABASE_URL","DATABASE_RLS_SECRET","BETTER_AUTH_SECRET"}}))
                    recovery.chmod(0o600)
                    print("Live cleanup not proven; exact owned PostgreSQL ledger retained: " + str(directory),flush=True)
                    raise RuntimeError("live cleanup not proven; private owned PostgreSQL ledger retained")
            (directory / "passwords.json").unlink(missing_ok=True)
            actual = json.loads(docker("inspect", "--format", '{"id":{{json .Id}},"label":{{json (index .Config.Labels "webdock.kubernetes-admission")}}}', custody))
            assert actual["id"] == custody and actual["label"] == nonce
            docker("rm", "-f", custody)
            absent = subprocess.run(["docker", "inspect", custody], env=environment, capture_output=True, text=True, timeout=15)
            assert absent.returncode == 1 and any(f"no such {kind}: {custody}" in absent.stderr.lower() for kind in ["object", "container"])
            print("Exact owned PostgreSQL container removed.", flush=True)


if __name__ == "__main__":
    main()
