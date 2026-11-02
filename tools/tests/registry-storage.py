#!/usr/bin/env python3
"""Owned authenticated Distribution/S3 acceptance; existing MinIO must already be running."""
import importlib.util
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import yaml

ROOT = Path(__file__).resolve().parents[2]
IMAGE = "registry@sha256:ddf754342cfc8acc51a56d5d0ab6af06826461864460636d8bd5c546dab2a7b8"
spec = importlib.util.spec_from_file_location("object_storage_fixture", Path(__file__).with_name("object-storage.py"))
storage = importlib.util.module_from_spec(spec)
spec.loader.exec_module(storage)


def run(s3_path, mc, access):
    s3 = json.loads(s3_path.read_text())
    nonce = secrets.token_hex(12)
    name = f"webdock-registry-s3-{nonce}"
    username, password = "fixture-publisher", secrets.token_hex(24)
    output = ROOT / ".superpowers/sdd/2026-10-25-s3-registry"
    output.mkdir(mode=0o700, parents=True, exist_ok=True)
    run_output = Path(tempfile.mkdtemp(prefix="registry-s3-", dir=output))
    run_output.chmod(0o700)
    custody = None
    private = None

    def docker(*args):
        result = subprocess.run(["docker", *args], capture_output=True, text=True, timeout=40)
        if result.returncode:
            raise RuntimeError(f"owned registry fixture operation failed: {args[0]}")
        return result.stdout.strip()

    try:
        docker("image", "inspect", IMAGE, "--format", "{{.Id}}")
        current = json.loads(docker("inspect", "webdock-dev-minio-1", "--format", '{"running":{{.State.Running}},"project":{{json (index .Config.Labels "com.docker.compose.project")}}}'))
        assert current == {"running": True, "project": "webdock-dev"}, "existing owned Webdock MinIO service must be running"
        private = Path(tempfile.mkdtemp(prefix="private-registry-"))
        # Bun already belongs to the repository toolchain; no extra password utility installed.
        hashed = subprocess.run(["bun", "-e", 'const p=await Bun.stdin.text();process.stdout.write(await Bun.password.hash(p,{algorithm:"bcrypt",cost:10}));'], input=password, capture_output=True, text=True, check=True, timeout=15).stdout
        assert re.fullmatch(r"\$2[aby]\$10\$[./A-Za-z0-9]{53}", hashed)
        (private / "htpasswd").write_text(f"{username}:{hashed}\n")
        (private / "htpasswd").chmod(0o600)
        values = {
            "image": IMAGE, "existingSecret": "fixture-only", "locationEvidence": "unverified local disposable S3 fixture",
            "s3": {"endpoint": "https://fixture.invalid", "region": "us-east-1", "bucket": s3["config"]["bucket"], "port": 443},
            "objectStoreCIDRs": ["192.0.2.1/32"], "clientNamespaceSelector": {"fixture": nonce}, "clientPodSelector": {"fixture": nonce},
        }
        value_file = private / "values.yaml"
        value_file.write_text(yaml.safe_dump(values))
        template = subprocess.run(["helm", "template", "owned-registry", str(ROOT / "deploy/helm/webdock-registry"), "--values", str(value_file)], capture_output=True, text=True, check=True, timeout=20)
        config_map = next(v for v in yaml.safe_load_all(template.stdout) if v and v["kind"] == "ConfigMap")
        config = yaml.safe_load(config_map["data"]["config.yml"])
        # Only this owned loopback fixture omits TLS; production chart enforces HTTPS.
        config["http"].pop("tls")
        config["http"]["secret"] = secrets.token_hex(32)
        config["storage"]["s3"].update({"regionendpoint": "http://webdock-dev-minio-1:9000", "secure": False,
                                               "accesskey": s3["credentials"]["accessKeyId"], "secretkey": s3["credentials"]["secretAccessKey"]})
        config_file = private / "config.yml"
        config_file.write_text(yaml.safe_dump(config))
        config_file.chmod(0o600)
        # Docker may reassign an unspecified host port on restart; retain this owned endpoint.
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            host_port = reservation.getsockname()[1]
        custody = docker("create", "--name", name, "--label", f"webdock.registry-storage={nonce}",
                         "--network", "webdock-dev_default", "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
                         "--user", f"{os.getuid()}:{os.getgid()}", "--mount", f"type=bind,src={private},dst=/run/registry-private,readonly",
                         "--tmpfs", "/tmp:rw,nosuid,nodev,size=16m", "-p", f"127.0.0.1:{host_port}:5000", "--entrypoint", "/bin/registry", IMAGE,
                         "serve", "/run/registry-private/config.yml")
        (run_output / "custody.json").write_text(json.dumps({"id": custody, "name": name, "runId": nonce, "image": IMAGE}))
        docker("start", custody)
        address = docker("port", custody, "5000/tcp")
        assert re.fullmatch(r"127\.0\.0\.1:[0-9]+", address)
        for _ in range(100):
            try:
                urllib.request.urlopen(f"http://{address}/v2/", timeout=1)
            except urllib.error.HTTPError as error:
                if error.code == 401:
                    break
            except (urllib.error.URLError, TimeoutError, ConnectionError):
                pass
            time.sleep(0.05)
        else:
            raise RuntimeError("owned registry startup timed out")
        fixture = private / "fixture.json"
        fixture.write_text(json.dumps({"address": address, "username": username, "password": password, "containerId": custody, "runId": nonce}))
        fixture.chmod(0o600)
        environment = {**os.environ, "WEBDOCK_REGISTRY_S3_TEST_CONFIG": str(fixture)}

        def test(case):
            log = run_output / f"{case}.log"
            with log.open("wb") as stream:
                log.chmod(0o600)
                result = subprocess.run(["cargo", "test", "--locked", "-p", "webdock-build", "--all-features", "--test", "registry_storage", case, "--", "--include-ignored", "--exact"], cwd=ROOT, env=environment, stdout=stream, stderr=subprocess.STDOUT, timeout=180)
            tail = "\n".join(log.read_text().splitlines()[-24:])
            for secret in [password, s3["credentials"]["accessKeyId"], s3["credentials"]["secretAccessKey"]]:
                tail = tail.replace(secret, "[redacted]")
            print(tail, flush=True)
            if result.returncode:
                raise RuntimeError(f"registry acceptance failed: {case}; private evidence at {run_output}")

        test("registry_s3_publish_pull_and_restart")
        inventory = mc("ls", "--recursive", "--json", f"fixture/{s3['config']['bucket']}/registry/")
        assert any("docker/registry/v2/blobs/" in json.loads(line).get("key", "") for line in inventory.splitlines()), "registry bytes were not observed in S3"
        try:
            mc("admin", "user", "disable", "fixture", access)
            test("registry_s3_backend_revocation_fails_closed")
        finally:
            mc("admin", "user", "enable", "fixture", access)
        test("registry_s3_backend_recovery_preserves_releases")
        print("Authenticated S3 registry push/pull, old release retention, restart, backend revocation and recovery verified.", flush=True)
    finally:
        try:
            if custody:
                actual = docker("inspect", "--format", '{{.Id}} {{index .Config.Labels "webdock.registry-storage"}}', custody)
                assert actual == f"{custody} {nonce}", "registry custody changed"
                logs = subprocess.run(["docker", "logs", custody], capture_output=True, text=True, timeout=15)
                log = run_output / "registry-private.log"
                log.write_text(logs.stdout + logs.stderr)
                log.chmod(0o600)
                docker("rm", "-f", "-v", custody)
                absent = subprocess.run(["docker", "inspect", custody], capture_output=True, text=True, timeout=15)
                assert absent.returncode == 1 and f"no such object: {custody}" in absent.stderr.lower()
                print("Exact owned registry container and its anonymous test volumes removed.", flush=True)
        finally:
            if private is not None:
                shutil.rmtree(private)


if __name__ == "__main__":
    storage.main(callback=run, registry=True)
