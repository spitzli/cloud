#!/usr/bin/env python3
"""Run acceptance against a fresh owned PostgreSQL container; never target existing databases.
On success remove exactly that container and private fixture files. On failure retain
both for exact MinIO recovery and private diagnostics; print no database credentials.
"""
import argparse
import json
import os
from pathlib import Path
import secrets
import stat
import subprocess
import time
import uuid

ROOT = Path(__file__).resolve().parents[3]


def docker(*args):
    result = subprocess.run(["docker", *args], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError("owned PostgreSQL fixture command failed")
    return result.stdout.strip()


def run():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--minio", action="store_true", help="also run real MinIO acceptance using its existing private config")
    args = parser.parse_args()
    private = ROOT / "plugins/object-storage/fixtures/.private"
    private.mkdir(mode=0o700, exist_ok=True)
    info = private.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) & 0o077:
        raise SystemExit("owned private fixture parent required")
    directory = private / ("postgres-" + uuid.uuid4().hex)
    directory.mkdir(mode=0o700)
    name = "wd-object-metadata-" + uuid.uuid4().hex
    created = False
    stage = "create"
    try:
        password = secrets.token_hex(32)
        env = directory / "postgres.env"
        fd = os.open(env, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        with os.fdopen(fd, "w") as output:
            output.write("POSTGRES_USER=fixture\nPOSTGRES_PASSWORD=" + password + "\n")
        docker("run", "--detach", "--name", name, "--label", "webdock.object-storage-fixture=" + name,
               "--env-file", str(env), "--publish", "127.0.0.1::5432", "--volume", "/var/lib/postgresql", "postgres:18-alpine")
        created = True
        port = docker("port", name, "5432/tcp").rsplit(":", 1)[1]
        for _ in range(240):
            ready = subprocess.run(["docker", "exec", name, "pg_isready", "-h", "127.0.0.1", "-U", "fixture", "-d", "postgres"], capture_output=True)
            if ready.returncode == 0:
                break
            time.sleep(0.25)
        else:
            raise RuntimeError("owned PostgreSQL fixture readiness deadline")
        config = directory / "storage.json"
        fd = os.open(config, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        with os.fdopen(fd, "w") as output:
            json.dump({"url": f"postgresql://fixture:{password}@127.0.0.1:{port}/postgres", "container": name}, output)
        env.unlink()
        child_env = os.environ.copy()
        child_env["WEBDOCK_OBJECT_STORAGE_POSTGRES_CONFIG"] = str(config)
        if args.minio:
            child_env.setdefault("WEBDOCK_OBJECT_STORAGE_MINIO_CONFIG", str(directory.parent / "backend.json"))
        command = ["cargo", "test", "--manifest-path", "plugins/object-storage/Cargo.toml", "--locked", "-j", "2",
                   "--", "--include-ignored", "--skip", "fence_child", "--skip", "recover_exact_owned_fixture", "--skip", "postgres_server_restart"]
        if not args.minio:
            command.extend(["--skip", "real_minio"])
        result = subprocess.run(command, cwd=ROOT, env=child_env)
        if result.returncode:
            raise RuntimeError("acceptance failed")
        restart_command = ["cargo", "test", "--manifest-path", "plugins/object-storage/Cargo.toml", "--locked", "-j", "2", "--test", "lifecycle",
                           "postgres_server_restart", "--", "--ignored", "--exact", "postgres_server_restart_preserves_intent_sealed_credentials_and_replay"]
        child_env["OBJECT_STORAGE_RESTART_PHASE"] = "write"
        if subprocess.run(restart_command, cwd=ROOT, env=child_env).returncode:
            raise RuntimeError("restart setup failed")
        stage = "restart container"
        docker("restart", "--timeout", "60", name)
        stage = "restart readiness"
        for _ in range(240):
            if subprocess.run(["docker", "exec", name, "pg_isready", "-h", "127.0.0.1", "-U", "fixture", "-d", "postgres"], capture_output=True).returncode == 0:
                break
            time.sleep(0.25)
        else:
            raise RuntimeError("restart readiness deadline")
        stage = "refresh published port"
        # Docker may reassign an ephemeral published host port on container restart.
        port = docker("port", name, "5432/tcp").rsplit(":", 1)[1]
        with config.open("w") as output:
            json.dump({"url": f"postgresql://fixture:{password}@127.0.0.1:{port}/postgres", "container": name}, output)
        stage = "read restart proof"
        child_env["OBJECT_STORAGE_RESTART_PHASE"] = "read"
        if subprocess.run(restart_command, cwd=ROOT, env=child_env).returncode:
            raise RuntimeError("restart proof failed")
        stage = "cleanup"
        docker("rm", "--force", "--volumes", name)
        created = False
        config.unlink()
        directory.rmdir()
        print("Owned PostgreSQL acceptance passed; exact container and private fixture config removed.")
    except Exception:
        print("Failed fixture stage: " + stage)
        if created:
            print("Acceptance incomplete; retained exact owned container " + name + " and private fixture directory " + str(directory))
        raise SystemExit("Owned fixture failed; no private database/provider diagnostics printed.") from None


if __name__ == "__main__":
    run()
