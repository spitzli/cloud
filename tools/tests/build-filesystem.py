#!/usr/bin/env python3
"""Owned trusted storage-helper fixture; never executes customer code or mounts host paths."""
import json
from pathlib import Path
import secrets
import subprocess

ROOT = Path(__file__).resolve().parents[2]
IMAGE = "sha256:60e7c44155bb"
TEST = "fixed_build_filesystem_enforces_enospc_checks_mount_identity_and_fences_retirement"
TESTS = {
    "build_storage": TEST,
    "storage": "disposable_filesystem_preserves_identity_enforces_bytes_and_serves_private_rpc",
    "database_storage": "disposable_database_pool_keeps_uid_content_identity_and_separate_rpc",
}


def docker(*arguments, timeout=30):
    result = subprocess.run(["docker", *arguments], text=True, capture_output=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError(f"owned storage fixture {arguments[0]} failed: {result.stderr[-1000:]}")
    return result.stdout.strip()


def main():
    # Cached, operator-owned Ubuntu/util-linux image. Never pull a mutable test tag.
    image = json.loads(docker("image", "inspect", IMAGE))[0]
    assert image["Config"]["Labels"].get("dev.webdock.test") == "rust-storage-fixture"
    compile_result = subprocess.run([
        "cargo", "test", "--locked", "-p", "webdock-agent", "--no-default-features",
        "--test", "build_storage", "--test", "storage", "--test", "database_storage",
        "--no-run", "--message-format=json",
    ], cwd=ROOT, text=True, capture_output=True, timeout=420)
    if compile_result.returncode:
        raise RuntimeError(compile_result.stderr[-3000:])
    artifacts = [json.loads(line) for line in compile_result.stdout.splitlines()]
    binaries = {row["target"]["name"]: row["executable"] for row in artifacts
                if row.get("reason") == "compiler-artifact" and row.get("executable")
                and row["target"]["name"] in TESTS}
    assert set(binaries) == set(TESTS)
    run = secrets.token_hex(12)
    container = None
    try:
        # Privilege belongs only to this trusted filesystem-helper test. Customer
        # build manifests keep their sandbox/security profile and no host devices.
        container = docker("create", "--label", "webdock.build-filesystem=" + run,
                           "--network=none", "--privileged", "--cpus=1", "--memory=512m",
                           "--pids-limit=128", "--tmpfs",
                           "/tmp:rw,exec,nosuid,nodev,size=384m,mode=1777",
                           "--entrypoint", "/bin/sleep", image["Id"], "infinity")
        info = json.loads(docker("inspect", container))[0]
        assert not info["HostConfig"].get("Binds") and not info["HostConfig"].get("Mounts")
        for name, binary in binaries.items():
            docker("cp", binary, container + ":/usr/local/bin/" + name + "-test")
        docker("start", container)
        for name, test in TESTS.items():
            result = subprocess.run([
                "docker", "exec", "-e", "WEBDOCK_DISPOSABLE_STORAGE_TEST=yes", container,
                "/usr/local/bin/" + name + "-test", test, "--ignored", "--exact", "--nocapture",
            ], cwd=ROOT, timeout=120)
            if result.returncode:
                raise RuntimeError("owned storage acceptance failed; no automatic retry")
    finally:
        if container is not None:
            info = json.loads(docker("inspect", container))[0]
            assert info["Id"] == container and info["Config"]["Labels"]["webdock.build-filesystem"] == run
            docker("rm", "--force", "--volumes", container)
            print("Exact owned trusted helper container removed; no host mounts.", flush=True)


if __name__ == "__main__":
    main()
