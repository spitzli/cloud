#!/usr/bin/env python3
"""Owned archives acceptance; reuse the running MinIO and the existing exact-cleanup helper."""
import importlib.util
import json
import os
from pathlib import Path
import secrets
import sys
import subprocess
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("owned_s3", ROOT / "tools/tests/object-storage.py")
owned_s3 = importlib.util.module_from_spec(spec)
spec.loader.exec_module(owned_s3)


def docker(*args, input=None):
    r = subprocess.run(["docker", "--host", "unix:///var/run/docker.sock", *args], input=input, text=True, capture_output=True, timeout=90)
    if r.returncode:
        raise RuntimeError("owned archives PostgreSQL operation failed")
    return r.stdout.strip()


def acceptance(config, mc, access):
    binding = json.loads(config.read_text())
    binding["config"]["namespace"] = "archives"
    bucket = binding["config"]["bucket"]
    nonce = binding["config"]["id"].removeprefix("minio-")
    policy = {"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": ["s3:GetObject", "s3:PutObject", "s3:DeleteObject", "s3:AbortMultipartUpload", "s3:ListMultipartUploadParts"], "Resource": [f"arn:aws:s3:::{bucket}/*"]},
        {"Effect": "Allow", "Action": ["s3:GetBucketVersioning", "s3:GetLifecycleConfiguration", "s3:GetBucketPolicy"], "Resource": [f"arn:aws:s3:::{bucket}"]},
    ]}
    # Replaces only this helper's freshly generated and already tracked policy.
    mc("admin", "policy", "create", "fixture", f"wd-s3-{nonce}", "/dev/stdin", input_text=json.dumps(policy))
    mc("ilm", "import", f"fixture/{bucket}", input_text=json.dumps({"Rules":[{"ID":"webdock-archives-retention","Status":"Enabled","Filter":{"Prefix":binding["config"]["namespace"]+"/"},"Expiration":{"Days":2},"AbortIncompleteMultipartUpload":{"DaysAfterInitiation":1}}]}))
    print("Owned lifecycle evidence:", mc("ilm", "export", f"fixture/{bucket}"))
    name = "wd-archives-" + uuid.uuid4().hex
    password = secrets.token_hex(32)
    with tempfile.TemporaryDirectory(prefix="webdock-archives-") as private:
        directory = Path(private)
        env_file = directory / "postgres.env"
        env_file.write_text("POSTGRES_PASSWORD=" + password + "\n")
        env_file.chmod(0o600)
        docker("run", "-d", "--rm", "--name", name, "--label", "webdock.archives=" + name, "--env-file", str(env_file), "--publish", "127.0.0.1::5432", "--tmpfs", "/var/lib/postgresql", "postgres:18-alpine")
        try:
            for _ in range(120):
                if subprocess.run(["docker", "--host", "unix:///var/run/docker.sock", "exec", name, "pg_isready", "-h", "127.0.0.1", "-U", "postgres"], capture_output=True).returncode == 0:
                    break
                time.sleep(0.25)
            else:
                raise RuntimeError("owned archives PostgreSQL readiness")
            port = docker("port", name, "5432/tcp").rsplit(":", 1)[1]
            source_password = secrets.token_hex(32)
            docker("exec", "-i", name, "psql", "-U", "postgres", "-v", "ON_ERROR_STOP=1", input=f"CREATE DATABASE archive_metadata; CREATE DATABASE archive_source; CREATE DATABASE archive_restore; CREATE ROLE archive_reader LOGIN PASSWORD '{source_password}';\n")
            docker("exec", "-i", name, "psql", "-U", "postgres", "-d", "archive_source", "-v", "ON_ERROR_STOP=1", input="CREATE TABLE proof(id integer primary key, value text); INSERT INTO proof VALUES (1,'owned export'),(2,'restore verified'); GRANT CONNECT ON DATABASE archive_source TO archive_reader; GRANT USAGE ON SCHEMA public TO archive_reader; GRANT SELECT ON ALL TABLES IN SCHEMA public TO archive_reader; ALTER ROLE archive_reader SET default_transaction_read_only=on;\n")
            fixture = directory / "fixture.json"
            fixture.write_text(json.dumps({"database": f"postgresql://postgres:{password}@127.0.0.1:{port}/archive_metadata", "source": f"postgresql://archive_reader:{source_password}@127.0.0.1:{port}/archive_source", "backend": {"config": binding["config"], "credentials": binding["credentials"]}}))
            fixture.chmod(0o600)
            env = {**os.environ, "CARGO_TARGET_DIR": str(ROOT / "target"), "WEBDOCK_ARCHIVES_FIXTURE": str(fixture), "WEBDOCK_ARCHIVES_RESTORE": f"postgresql://postgres:{password}@127.0.0.1:{port}/archive_restore"}
            r = subprocess.run(["cargo", "test", "--manifest-path", "plugins/archives/Cargo.toml", "--locked", "--features", "local-test", "--test", "owned", "-j", "2", "--", "--include-ignored"], cwd=ROOT, env=env, timeout=900)
            if r.returncode:
                raise RuntimeError("owned archive acceptance failed")
            readonly = subprocess.run(["cargo", "test", "--manifest-path", "plugins/archives/Cargo.toml", "--locked", "--features", "local-test", "--test", "readonly", "-j", "2", "--", "--include-ignored"], cwd=ROOT, env=env, timeout=900)
            if readonly.returncode:
                raise RuntimeError("owned readonly source privilege acceptance failed")
            if os.environ.get("WEBDOCK_ARCHIVES_SKIP_CORE") != "1":
                core = subprocess.run(["cargo", "test", "--manifest-path", "plugins/archives/Cargo.toml", "--locked", "--features", "local-test", "--test", "core", "-j", "2", "--", "--include-ignored"], cwd=ROOT, env=env, timeout=900)
                if core.returncode:
                    raise RuntimeError("owned Core archive authority acceptance failed")
            # All recorded objects were explicitly deleted, not merely made inaccessible.
            assert not mc("ls", "--recursive", "--json", f"fixture/{bucket}").strip()
            print("Legacy-PG rejection/recovery, durable diagnostic receipts, structured gzip, authenticated binary gateway, expiry and exact deletion passed.")
        finally:
            if docker("inspect", name, "--format", '{{index .Config.Labels "webdock.archives"}}') != name:
                raise RuntimeError("owned archives PostgreSQL ownership mismatch")
            docker("rm", "-f", "-v", name)
    print("Exact owned archives PostgreSQL and private temporary inputs removed.")


if __name__ == "__main__":
    try:
        owned_s3.main(callback=acceptance)
    except Exception as error:
        if isinstance(error, RuntimeError):
            print(str(error))
        raise SystemExit("Owned archives acceptance incomplete; private diagnostics withheld.") from None
