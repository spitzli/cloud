#!/usr/bin/env python3
"""Fresh PostgreSQL + exact plugin-created resources on the parent's running MinIO.
Never prints private input, signed plans, provider errors, or touches other resources.
"""
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[4]
MANIFEST = ROOT / "crates/plugin-host/tests/producer/Cargo.toml"


def docker(*args):
    result = subprocess.run(["docker", "--host", "unix:///var/run/docker.sock", *args], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError("owned producer PostgreSQL operation failed")
    return result.stdout.strip()


def main():
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(ROOT / "target")
    subprocess.run(["cargo", "build", "--manifest-path", str(MANIFEST), "--locked", "-j", "2"], cwd=ROOT, env=env, check=True)
    name = "wd-gateway-producer-" + uuid.uuid4().hex
    with tempfile.TemporaryDirectory(prefix="wd-gateway-producer-") as private:
        path = Path(private) / "postgres.env"
        password = secrets.token_hex(32)
        fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as output:
            output.write("POSTGRES_USER=fixture\nPOSTGRES_DB=gateway_fixture\nPOSTGRES_PASSWORD=" + password + "\n")
        docker("run", "-d", "--rm", "--name", name, "--label", "webdock.gateway-producer=" + name,
               "--env-file", str(path), "--publish", "127.0.0.1::5432", "--tmpfs", "/var/lib/postgresql", "postgres:18-alpine")
        try:
            for _ in range(120):
                if subprocess.run(["docker", "--host", "unix:///var/run/docker.sock", "exec", name, "pg_isready", "-U", "fixture", "-d", "gateway_fixture"], capture_output=True).returncode == 0:
                    break
                time.sleep(0.25)
            else:
                raise RuntimeError("owned producer PostgreSQL readiness")
            port = docker("port", name, "5432/tcp").rsplit(":", 1)[1]
            config = {"database": f"postgresql://fixture:{password}@127.0.0.1:{port}/gateway_fixture",
                      "backend": str(ROOT / "plugins/object-storage/fixtures/.private/backend.json")}
            result = subprocess.run([str(ROOT / "target/debug/webdock-storage-producer-fixture")], input=json.dumps(config), text=True, cwd=ROOT)
            if result.returncode:
                raise RuntimeError("owned producer gateway acceptance failed")
        finally:
            if docker("inspect", name, "--format", '{{index .Config.Labels "webdock.gateway-producer"}}') != name:
                raise RuntimeError("owned producer PostgreSQL label mismatch")
            docker("rm", "-f", "-v", name)
    print("Fresh producer PostgreSQL and exact private inputs removed; parent MinIO preserved.")


if __name__ == "__main__":
    try:
        main()
    except Exception:
        raise SystemExit("Owned producer fixture incomplete; private diagnostics withheld.") from None
