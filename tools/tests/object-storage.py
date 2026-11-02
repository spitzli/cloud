#!/usr/bin/env python3
"""Bounded S3 acceptance using only fresh, owned buckets on existing Webdock MinIO."""
import json
import os
from pathlib import Path
import secrets
import stat
import subprocess
import tempfile
import urllib.parse
import sys

ROOT = Path(__file__).resolve().parents[2]
MC = "quay.io/minio/mc:RELEASE.2025-08-13T08-35-41Z"


def cleanup_owned(mc, owned):
    """Each independent owned resource gets its cleanup attempt, even after another fails."""
    failures = []
    def attempt(label, *args):
        try:
            return mc(*args)
        except Exception:
            failures.append(label)
            return None
    for kind, value in reversed(owned):
        if kind == "attachment":
            policy, user = value
            attempt("detach-policy", "admin", "policy", "detach", "fixture", policy, "--user", user)
    # Removing the owned user also removes any attachment whose acknowledgement was lost.
    for kind, name in reversed(owned):
        if kind == "user":
            attempt("disable-user", "admin", "user", "disable", "fixture", name)
            attempt("remove-user", "admin", "user", "remove", "fixture", name)
    for kind, name in reversed(owned):
        if kind == "policy":
            attempt("remove-policy", "admin", "policy", "remove", "fixture", name)
    for kind, name in reversed(owned):
        if kind != "bucket":
            continue
        attempt("empty-bucket", "rm", "--recursive", "--force", f"fixture/{name}")
        attempt("remove-bucket", "rb", f"fixture/{name}")
        listing = attempt("verify-bucket-absence", "ls", "--json", "fixture")
        if listing is not None:
            try:
                if any(json.loads(line).get("key", "").rstrip("/") == name for line in listing.splitlines()):
                    failures.append("bucket-still-present")
            except (ValueError, TypeError):
                failures.append("invalid-absence-evidence")
    if failures:
        raise RuntimeError("owned fixture cleanup failed: " + ", ".join(failures))


def main(command=None, callback=None, registry=False):
    credential_file = ROOT / ".dev-local/minio.env"
    meta = credential_file.lstat()
    assert stat.S_ISREG(meta.st_mode) and meta.st_uid == os.getuid()
    assert meta.st_nlink == 1 and meta.st_mode & 0o077 == 0
    values = dict(line.split("=", 1) for line in credential_file.read_text().splitlines() if "=" in line)
    endpoint = f"http://127.0.0.1:{int(os.environ.get('WEBDOCK_DEV_S3_PORT', '55900'))}"
    parsed = urllib.parse.urlsplit(endpoint)
    user = urllib.parse.quote(values["MINIO_ROOT_USER"], safe="")
    password = urllib.parse.quote(values["MINIO_ROOT_PASSWORD"], safe="")
    environment = {**os.environ, "MC_HOST_fixture": f"{parsed.scheme}://{user}:{password}@{parsed.netloc}"}
    nonce = secrets.token_hex(10)
    bucket, denied_bucket = f"wd-s3-test-{nonce}", f"wd-s3-denied-{nonce}"
    access, secret = f"wd{nonce}", secrets.token_hex(32)
    policy_name = f"wd-s3-{nonce}"
    owned = []

    def mc(*args, input_text=None):
        result = subprocess.run(
            ["docker", "run", "--rm", "--network", "host", "-i", "--env", "MC_HOST_fixture", MC, *args],
            input=input_text, text=True, capture_output=True, env=environment, timeout=90,
        )
        # Provider/CLI errors can contain authenticated URLs. Never echo raw diagnostics.
        if result.returncode:
            raise RuntimeError(f"owned MinIO fixture operation failed: {args[0]}")
        return result.stdout

    try:
        for name in (bucket, denied_bucket):
            mc("mb", f"fixture/{name}")
            owned.append(("bucket", name))
        mc("admin", "user", "add", "fixture", access, secret)
        owned.append(("user", access))
        policy = {"Version": "2012-10-17", "Statement": [
            {"Effect": "Allow", "Action": ["s3:GetObject", "s3:PutObject", "s3:AbortMultipartUpload", "s3:ListMultipartUploadParts"], "Resource": [f"arn:aws:s3:::{bucket}/*"]},
        ]}
        if registry:
            policy["Statement"][0]["Action"].append("s3:DeleteObject")
            policy["Statement"].append({"Effect": "Allow", "Action": ["s3:ListBucket", "s3:GetBucketLocation", "s3:ListBucketMultipartUploads"], "Resource": [f"arn:aws:s3:::{bucket}"]})
        mc("admin", "policy", "create", "fixture", policy_name, "/dev/stdin", input_text=json.dumps(policy))
        owned.append(("policy", policy_name))
        mc("admin", "policy", "attach", "fixture", policy_name, "--user", access)
        owned.append(("attachment", (policy_name, access)))
        with tempfile.TemporaryDirectory(prefix="webdock-s3-") as directory:
            config = Path(directory) / "fixture.json"
            config.write_text(json.dumps({
                "config": {"id": f"minio-{nonce}", "endpoint": endpoint, "region": "us-east-1", "bucket": bucket, "namespace": "builds"},
                "credentials": {"accessKeyId": access, "secretAccessKey": secret}, "deniedBucket": denied_bucket,
            }))
            config.chmod(0o600)
            run_environment = {**os.environ, "WEBDOCK_S3_TEST_CONFIG": str(config)}
            if callback:
                callback(config, mc, access)
            else:
                result = subprocess.run(command or [
                    "cargo", "test", "--locked", "-p", "webdock-object-storage", "--features", "local-test",
                    "--test", "transfers", "real_minio", "--", "--include-ignored",
                ], cwd=ROOT, env=run_environment, timeout=900)
                if result.returncode:
                    raise RuntimeError("S3 acceptance failed")
        incomplete = mc("ls", "--incomplete", "--recursive", "--json", f"fixture/{bucket}").strip()
        assert not incomplete, "failed upload left incomplete multipart state"
        print("Owned MinIO acceptance command passed; no incomplete multipart uploads remain.")
    finally:
        cleanup_owned(mc, owned)
        print("Exact owned test buckets, user and policy removed; existing MinIO data retained.")


if __name__ == "__main__":
    main(sys.argv[1:] or None)
