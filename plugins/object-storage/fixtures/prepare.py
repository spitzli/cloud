#!/usr/bin/env python3
"""Opt-in local fixture setup. Reuses an already running MinIO; starts no service.
Extracts only the pinned, already present official mc image, removing the extraction container.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import stat
import subprocess
import uuid
from urllib.parse import urlsplit

IMAGE = "quay.io/minio/mc@sha256:a7fe349ef4bd8521fb8497f55c6042871b2ae640607cf99d9bede5e9bdf11727"
DIGEST = "01f866e9c5f9b87c2b09116fa5d7c06695b106242d829a8bb32990c00312e891"


def private_write(path, data, mode=0o600):
    fd = os.open(path, os.O_CREAT | os.O_WRONLY | os.O_NOFOLLOW, mode)
    with os.fdopen(fd, "wb") as output:
        info = os.fstat(output.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) & 0o077:
            raise RuntimeError("private fixture file required")
        output.truncate(0)
        output.write(data)


def run():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--minio-env", type=Path, required=True, help="owned private existing MinIO env file")
    parser.add_argument("--endpoint", default="http://127.0.0.1:55900")
    parser.add_argument("--directory", type=Path, help="separate owned private fixture directory")
    args = parser.parse_args()
    endpoint = urlsplit(args.endpoint)
    if endpoint.hostname not in ("127.0.0.1", "::1") or endpoint.scheme not in ("http", "https") or endpoint.username or endpoint.password or endpoint.path not in ("", "/") or endpoint.query or endpoint.fragment:
        raise RuntimeError("loopback MinIO endpoint required")
    fd = os.open(args.minio_env, os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd) as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_uid != os.geteuid() or stat.S_IMODE(info.st_mode) & 0o077:
            raise RuntimeError("owned private MinIO env required")
        values = dict(line.split("=", 1) for line in source.read().splitlines() if "=" in line and not line.startswith("#"))
    access = values["MINIO_ROOT_USER"].strip().strip("\"'")
    secret = values["MINIO_ROOT_PASSWORD"].strip().strip("\"'")
    private = args.directory.resolve() if args.directory else Path(__file__).resolve().parent / ".private"
    private.mkdir(mode=0o700, exist_ok=True)
    info = private.lstat()
    if not stat.S_ISDIR(info.st_mode) or stat.S_IMODE(info.st_mode) & 0o077 or info.st_uid != os.geteuid():
        raise RuntimeError("owned private fixture directory required")
    binary = private / "mc"
    if not binary.exists():
        subprocess.run(["docker", "image", "inspect", IMAGE], capture_output=True, check=True)
        name = "wd-storage-mc-" + uuid.uuid4().hex
        custody = {"kind": "container", "name": name, "image": IMAGE, "status": "intent"}
        receipt = private / "mc-extraction.json"
        if os.path.lexists(receipt):
            raise RuntimeError("retained extraction receipt requires review")
        private_write(receipt, json.dumps(custody).encode())
        result = subprocess.run(["docker", "create", "--name", name, "--label", "webdock.object-storage-mc-extract=" + name, IMAGE], capture_output=True, text=True, check=True)
        container = result.stdout.strip()
        if len(container) != 64 or any(c not in "0123456789abcdef" for c in container):
            raise RuntimeError("extraction container identity unavailable")
        custody.update({"id": container, "status": "owned"})
        private_write(receipt, json.dumps(custody).encode())
        try:
            subprocess.run(["docker", "cp", container + ":/usr/bin/mc", str(binary)], capture_output=True, check=True)
            binary.chmod(0o700)
        finally:
            subprocess.run(["docker", "rm", container], capture_output=True, check=True)
            absent = subprocess.run(["docker", "inspect", container], capture_output=True, text=True)
            if absent.returncode != 1 or "no such object" not in absent.stderr.lower():
                raise RuntimeError("extraction container absence unverified")
            custody["status"] = "removed"
            private_write(receipt, json.dumps(custody).encode())
    if binary.is_symlink() or hashlib.sha256(binary.read_bytes()).hexdigest() != DIGEST:
        raise RuntimeError("pinned official mc hash mismatch")
    config = private / "mc-config"
    config.mkdir(mode=0o700, exist_ok=True)
    info = config.lstat()
    if not stat.S_ISDIR(info.st_mode) or stat.S_IMODE(info.st_mode) & 0o077 or info.st_uid != os.geteuid():
        raise RuntimeError("owned private mc directory required")
    alias = {"url": args.endpoint, "accessKey": access, "secretKey": secret, "api": "S3v4", "path": "on"}
    private_write(config / "config.json", json.dumps({"version": "10", "aliases": {"local": alias}}).encode())
    backend = {"id": "owned-minio-acceptance", "kind": "minio-local", "endpoint": args.endpoint, "region": "us-east-1", "account": "local-owned-fixture", "accessKey": access, "secretKey": secret, "minio": {"executable": str(binary), "configDir": str(config)}}
    private_write(private / "backend.json", json.dumps(backend).encode())
    print("Private loopback fixture prepared; no service was started or changed.")


if __name__ == "__main__":
    try:
        run()
    except Exception:
        raise SystemExit("Local fixture setup failed; check private paths and the pinned existing mc image.") from None
