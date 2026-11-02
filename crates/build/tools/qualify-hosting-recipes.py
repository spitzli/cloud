#!/usr/bin/env python3
"""Owned minimal SQL recipe regression. Never bootstraps Core or native migrations."""
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
import time
import urllib.parse

ROOT = Path(__file__).resolve().parents[3]
IMAGE = "sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336"


def main():
    nonce = secrets.token_hex(12)
    name = "webdock-git-recipe-query-" + nonce
    secret = secrets.token_hex(32)
    environment = {**os.environ, "POSTGRES_PASSWORD": secret}
    container = None

    def docker(*args):
        result = subprocess.run(["docker", *args], env=environment, capture_output=True, text=True, timeout=40)
        if result.returncode:
            detail = result.stderr.replace(secret, "[redacted]")[:500] if "psql" in args else ""
            raise RuntimeError("owned recipe SQL fixture failed: " + args[0] + " " + detail)
        return result.stdout.strip()

    with tempfile.TemporaryDirectory(prefix="webdock-git-recipe-query-") as private:
        try:
            container = docker("create", "--name", name, "--label", "webdock.git-recipe-query=" + nonce,
                               "--env", "POSTGRES_PASSWORD", "--env", "POSTGRES_DB=webdock_git_recipe_query",
                               "--tmpfs", "/var/lib/postgresql:rw,nosuid,nodev,size=256m", "-p", "127.0.0.1::5432", IMAGE)
            Path(private, "custody.json").write_text(json.dumps({"id": container, "runId": nonce, "name": name, "image": IMAGE}))
            docker("start", container)
            for _ in range(100):
                result = subprocess.run(["docker", "exec", container, "psql", "-X", "-qAt", "-U", "postgres", "-d", "webdock_git_recipe_query", "-c", "SELECT 1"], capture_output=True, timeout=5)
                if result.returncode == 0:
                    break
                time.sleep(0.1)
            else:
                raise RuntimeError("owned query fixture readiness timeout")
            address = docker("port", container, "5432/tcp")
            assert address.startswith("127.0.0.1:")
            docker("exec", container, "psql", "-X", "-q", "-v", "ON_ERROR_STOP=1", "-U", "postgres", "-d", "webdock_git_recipe_query", "-c",
                   "CREATE SCHEMA webdock_auth; CREATE TABLE webdock_auth.git_source(id text PRIMARY KEY,project_id text,recipe text); CREATE TABLE webdock_auth.hosting_project(project_id text PRIMARY KEY,cluster_id text);")
            environment["WEBDOCK_GIT_RECIPE_DATABASE_URL"] = "postgresql://postgres:" + urllib.parse.quote(secret, safe="") + "@" + address + "/webdock_git_recipe_query"
            result = subprocess.run(["cargo", "+1.94.0", "test", "--offline", "--locked", "-p", "webdock-hosting", "--lib", "frontend_source_", "-j", "2", "--", "--include-ignored", "--nocapture"], cwd=ROOT, env=environment, capture_output=True, text=True, timeout=420)
            output = (result.stdout + result.stderr).replace(secret, "[redacted]")
            import re
            print(re.sub(r"postgres(?:ql)?://[^\s\"']+", "[redacted-dsn]", output))
            if result.returncode:
                raise RuntimeError("hosting recipe query regression failed")
        finally:
            if container:
                actual = json.loads(docker("inspect", "--format", '{"id":{{json .Id}},"label":{{json (index .Config.Labels "webdock.git-recipe-query")}}}', container))
                assert actual == {"id": container, "label": nonce}
                docker("rm", "-f", container)
                absent = subprocess.run(["docker", "inspect", container], capture_output=True, text=True, timeout=15)
                assert absent.returncode == 1 and any(f"no such {kind}: {container}" in absent.stderr.lower() for kind in ["object", "container"])
                print("Exact owned minimal query fixture removed; no Core/native bootstrap or migration ran.")


if __name__ == "__main__":
    main()
