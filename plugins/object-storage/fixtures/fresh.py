#!/usr/bin/env python3
"""Run PG/MinIO acceptance using only newly created owned containers and private configs.
Never read existing MinIO configuration or start/stop a retained service.
"""
import os
from pathlib import Path
import secrets
import shutil
import stat
import subprocess
import time
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[3]


def docker(*args):
    result = subprocess.run(["docker", *args], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError("owned MinIO Docker command failed")
    return result.stdout.strip()


def run():
    private = Path(__file__).resolve().parent / ".private"
    private.mkdir(mode=0o700, exist_ok=True)
    info = private.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) & 0o077:
        raise RuntimeError("owned private fixture parent required")
    directory = private / ("fresh-" + uuid.uuid4().hex)
    directory.mkdir(mode=0o700)
    name = "wd-object-minio-" + uuid.uuid4().hex
    created = False
    try:
        env = directory / "minio.env"
        fd = os.open(env, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        with os.fdopen(fd, "w") as output:
            output.write("MINIO_ROOT_USER=fixture" + secrets.token_hex(8) + "\nMINIO_ROOT_PASSWORD=" + secrets.token_hex(32) + "\n")
        # Use the already available local test image; never pull or change a running service.
        image = docker("image", "inspect", "webdock-dev-minio:local", "--format", "{{.Id}}")
        docker("run", "--detach", "--name", name, "--label", "webdock.object-storage-fixture=" + name,
               "--env-file", str(env), "--publish", "127.0.0.1::9000", "--volume", "/data", image, "server", "/data")
        created = True
        port = docker("port", name, "9000/tcp").rsplit(":", 1)[1]
        endpoint = "http://127.0.0.1:" + port
        for _ in range(240):
            try:
                with urllib.request.urlopen(endpoint + "/minio/health/ready", timeout=1) as response:
                    if response.status == 200:
                        break
            except Exception:
                time.sleep(0.25)
        else:
            raise RuntimeError("owned MinIO readiness deadline")
        subprocess.run(["python3", str(Path(__file__).with_name("prepare.py")), "--minio-env", str(env),
                        "--endpoint", endpoint, "--directory", str(directory)], cwd=ROOT, check=True)
        env.unlink()
        child_env = os.environ.copy()
        child_env["WEBDOCK_OBJECT_STORAGE_MINIO_CONFIG"] = str(directory / "backend.json")
        subprocess.run(["python3", str(Path(__file__).with_name("postgres.py")), "--minio"], cwd=ROOT, env=child_env, check=True)
        label = docker("inspect", name, "--format", '{{index .Config.Labels "webdock.object-storage-fixture"}}')
        if label != name:
            raise RuntimeError("owned fixture label mismatch")
        docker("rm", "--force", "--volumes", name)
        created = False
        shutil.rmtree(directory)
        print("Fresh owned MinIO and PostgreSQL acceptance passed; exact containers, volumes and private configs removed.")
    except Exception:
        if created:
            print("Retained exact owned MinIO " + name + " and private fixture directory " + str(directory))
        raise SystemExit("Fresh fixture acceptance incomplete; no private diagnostics printed.") from None


if __name__ == "__main__":
    run()
