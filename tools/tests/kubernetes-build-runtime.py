#!/usr/bin/env python3
"""Actual retained OCI -> registry -> owned local RUN K3s acceptance.

Requires the private OCI image exported by kubernetes-builds.py --live-only.
The HTTP registry/mirror exception exists only in an ownership-checked disposable
local Docker node. No production kubeconfig, registry or build host is modified.
"""
import argparse
import json
import os
from pathlib import Path
import re
import secrets
import signal
import stat
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
REGISTRY_IMAGE = "registry@sha256:ddf754342cfc8acc51a56d5d0ab6af06826461864460636d8bd5c546dab2a7b8"
RUN_IMAGE = "rancher/k3s@sha256:057cfd5a9d51b89f1f0ee139c28d5d5fdd5aa7ddeac2742a0ba90c6dbc0329bb"
MIRRORS = "/etc/rancher/k3s/registries.yaml"


def run(arguments, *, data=None, check=True, timeout=120):
    result = subprocess.run(arguments, input=data, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, timeout=timeout, cwd=ROOT)
    if check and result.returncode:
        raise RuntimeError(f"owned fixture command failed: {arguments[0]}")
    return result


def private_file(path):
    path = Path(path)
    metadata = path.lstat()
    if (not path.is_absolute() or path.resolve() != path or
            not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != os.geteuid() or
            metadata.st_mode & 0o077 or metadata.st_nlink != 1):
        raise RuntimeError("expected an absolute, owned private regular file")
    return path


def inspect_container(identifier):
    return json.loads(run(["docker", "inspect", identifier]).stdout)[0]


def owned_node(record):
    node = inspect_container(record["container"])
    if (node["Id"] != record["container"] or
            node["Name"] != "/" + record["name"] or
            node["Config"]["Labels"].get("webdock.build-runtime") != record["nonce"] or
            not record["name"].startswith("webdock-build-run-test-") or
            not re.fullmatch(r"[0-9a-f]{16}", record["nonce"])):
        raise RuntimeError("local RUN fixture ownership mismatch")
    return node


def wait_ready(record, kubeconfig=None):
    node = owned_node(record)
    if not node["State"]["Running"]:
        run(["docker", "start", record["container"]])
    deadline = time.monotonic() + 120
    while time.monotonic() < deadline:
        result = run(["docker", "exec", record["container"], "kubectl", "get", "nodes", "-o", "json"], check=False, timeout=10)
        if result.returncode == 0:
            items = json.loads(result.stdout)["items"]
            if len(items) == 1 and any(c["type"] == "Ready" and c["status"] == "True" for c in items[0]["status"].get("conditions", [])):
                ports = inspect_container(record["container"])["NetworkSettings"]["Ports"]["6443/tcp"]
                if len(ports) != 1 or ports[0]["HostIp"] != "127.0.0.1":
                    raise RuntimeError("owned RUN API must be loopback-only")
                if kubeconfig is not None:
                    text = kubeconfig.read_text()
                    if text.startswith("{"):
                        config = json.loads(text)
                        if len(config["clusters"]) != 1 or not config["clusters"][0]["cluster"]["server"].startswith("https://127.0.0.1:"):
                            raise RuntimeError("owned kubeconfig endpoint mismatch")
                        config["clusters"][0]["cluster"]["server"] = f"https://127.0.0.1:{ports[0]['HostPort']}"
                        kubeconfig.write_text(json.dumps(config))
                    else:
                        text, replacements = re.subn(r"server: https://127\.0\.0\.1:[0-9]+", f"server: https://127.0.0.1:{ports[0]['HostPort']}", text)
                        if replacements != 1:
                            raise RuntimeError("owned kubeconfig endpoint mismatch")
                        kubeconfig.write_text(text)
                return
        time.sleep(0.5)
    raise RuntimeError("owned RUN node did not become Ready")


def write_private(path, data):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "wb") as output:
        output.write(data)
        output.flush()
        os.fsync(output.fileno())


def cleanup_created_run(record):
    found = run(["docker", "inspect", record["container"] or record["name"]], check=False) if record.get("containerCreationStarted") else None
    if found is not None and found.returncode != 0 and b"no such" not in found.stderr.lower():
        raise RuntimeError("created RUN container absence is uncertain")
    if found is not None and found.returncode == 0:
        node = json.loads(found.stdout)[0]
        if record["container"] is None:
            if node["Name"] != "/" + record["name"] or node["Config"]["Labels"].get("webdock.build-runtime") != record["nonce"]:
                raise RuntimeError("created RUN cleanup ownership mismatch")
            record["container"] = node["Id"]
        owned_node(record)
        run(["docker", "rm", "-f", "-v", record["container"]])
    found = run(["docker", "volume", "inspect", record["volume"]], check=False) if record.get("volumeCreated") else None
    if found is not None and found.returncode != 0 and b"no such" not in found.stderr.lower():
        raise RuntimeError("created RUN volume absence is uncertain")
    if found is not None and found.returncode == 0:
        volume = json.loads(found.stdout)[0]
        if volume["Name"] != record["volume"] or (volume.get("Labels") or {}).get("webdock.build-runtime") != record["nonce"]:
            raise RuntimeError("created RUN volume cleanup ownership mismatch")
        run(["docker", "volume", "rm", record["volume"]])


def create_run():
    # This privileged local K3s node runs only the trusted acceptance image. It is
    # a disposable test fixture, not the production builder or an MVP dependency.
    run(["docker", "image", "inspect", RUN_IMAGE])  # Never pull or select a moving tag.
    nonce = secrets.token_hex(8)
    directory = ROOT / ".runtime-test/build-runtime/created" / nonce
    directory.mkdir(parents=True, mode=0o700)
    directory.chmod(0o700)
    name = f"webdock-build-run-test-{nonce}"
    record = {"nonce": nonce, "name": name, "container": None, "volume": name + "-data", "image": RUN_IMAGE, "snapshotter": "overlayfs",
              "volumeCreated": False, "containerCreationStarted": False}
    ownership = directory / "ownership.json"
    kubeconfig = directory / "owned.kubeconfig"
    try:
        if run(["docker", "volume", "inspect", record["volume"]], check=False).returncode == 0:
            raise RuntimeError("refusing existing RUN fixture volume")
        if run(["docker", "inspect", name], check=False).returncode == 0:
            raise RuntimeError("refusing existing RUN fixture container")
        run(["docker", "volume", "create", "--label", f"webdock.build-runtime={nonce}", record["volume"]])
        record["volumeCreated"] = True
        record["containerCreationStarted"] = True
        record["container"] = run(["docker", "run", "-d", "--name", name, "--label", f"webdock.build-runtime={nonce}",
            "--privileged", "--cpus", "2", "--memory", "3g", "--pids-limit", "4096", "-p", "127.0.0.1::6443",
            "-v", f"{record['volume']}:/var/lib/rancher/k3s", RUN_IMAGE, "server", "--node-name", name,
            # Match existing recovery fixtures: native snapshots charge copied image
            # contents to the Pod's 16 MiB writable-layer budget; overlayfs does not.
            "--snapshotter", "overlayfs", "--disable", "traefik", "--disable", "servicelb"]).stdout.decode().strip()
        write_private(ownership, json.dumps(record, indent=2).encode() + b"\n")
        wait_ready(record)
        config = json.loads(run(["docker", "exec", record["container"], "kubectl", "config", "view", "--raw", "-o", "json"]).stdout)
        context = f"k3d-webdock-rust-build-run-{nonce}"
        if any(len(config[key]) != 1 for key in ["clusters", "users", "contexts"]):
            raise RuntimeError("unexpected owned kubeconfig shape")
        for key in ["clusters", "users", "contexts"]:
            config[key][0]["name"] = context
        config["contexts"][0]["context"].update({"cluster": context, "user": context})
        config["current-context"] = context
        config["clusters"][0]["cluster"]["server"] = "https://127.0.0.1:6443"
        write_private(kubeconfig, json.dumps(config).encode())
        wait_ready(record, kubeconfig)
        return record, ownership, kubeconfig
    except BaseException:
        cleanup_created_run(record)
        raise


def acceptance(oci, ownership=None, kubeconfig=None, *, publish_only=False, health_only=False):
    record = None
    context = None
    if not publish_only:
        record = json.loads(ownership.read_bytes())
        owned_node(record)
        context = f"k3d-webdock-rust-build-run-{record['nonce']}"
        text = kubeconfig.read_text()
        configured_context = json.loads(text)["current-context"] if text.startswith("{") else None
        if configured_context != context and f"name: {context}" not in text:
            raise RuntimeError("owned kubeconfig context mismatch")
        wait_ready(record, kubeconfig)
        # Refuse to overwrite any pre-existing runtime registry configuration.
        if run(["docker", "exec", record["container"], "test", "-e", MIRRORS], check=False).returncode == 0:
            raise RuntimeError("owned node already has registry configuration; inspect before reuse")
    run(["docker", "image", "inspect", REGISTRY_IMAGE])
    nonce = secrets.token_hex(8)
    name = f"webdock-build-runtime-registry-{nonce}"
    registry_id = None
    mirror_bytes = None
    evidence = (ROOT / ".runtime-test/build-runtime/publication" if publish_only else ownership.parent / "evidence") / nonce
    evidence.mkdir(parents=True, mode=0o700)
    evidence.chmod(0o700)
    print(f"Owned {'registry-only' if publish_only else 'RUN'} fixture; private evidence: {evidence}", flush=True)
    result_code = 1
    cleanup_ok = True
    try:
        registry_id = run(["docker", "run", "-d", "--name", name,
            "--label", f"webdock.build-runtime-registry={nonce}",
            "--read-only", "--tmpfs", f"/var/lib/registry:rw,nosuid,nodev,size={'1g' if publish_only or health_only else '128m'}",
            "--tmpfs", "/tmp:rw,nosuid,nodev,size=16m", "-p", "127.0.0.1::5000",
            "-e", "REGISTRY_HTTP_SECRET=owned-disposable-fixture-only", REGISTRY_IMAGE]).stdout.decode().strip()
        registry = inspect_container(registry_id)
        ports = registry["NetworkSettings"]["Ports"]["5000/tcp"]
        if len(ports) != 1 or ports[0]["HostIp"] != "127.0.0.1":
            raise RuntimeError("owned registry endpoint must be loopback-only")
        address = f"127.0.0.1:{ports[0]['HostPort']}"
        env = {**os.environ, "SPITZLI_BUILD_RUN_REGISTRY": address, "SPITZLI_BUILD_RUN_OCI": str(oci)}
        if publish_only:
            env["SPITZLI_BUILD_PUBLISH_TEST"] = "1"
            test_name = "real_build_oci_publishes_and_registry_readback_matches"
        else:
            endpoint = registry["NetworkSettings"]["Networks"]["bridge"]["IPAddress"]
            # Only this disposable registry authority uses plaintext on the local
            # Docker bridge. This is not a production configuration example.
            mirror_bytes = json.dumps({"mirrors": {address: {"endpoint": [f"http://{endpoint}:5000"]}}}, sort_keys=True).encode() + b"\n"
            run(["docker", "exec", "-i", record["container"], "sh", "-c",
                 f"umask 077; mkdir -p /etc/rancher/k3s; test ! -e {MIRRORS} && cat > {MIRRORS}"], data=mirror_bytes)
            owned_node(record)
            run(["docker", "restart", record["container"]])
            wait_ready(record, kubeconfig)
            env.update({"SPITZLI_BUILD_RUNTIME_TEST": "1", "SPITZLI_KUBE_TEST_CONFIG": str(kubeconfig), "SPITZLI_KUBE_TEST_CONTEXT": context})
            if health_only:
                env["SPITZLI_BUILD_HEALTH_TEST"] = "1"
                test_name = "real_build_registry_digest_serves_http_under_native_runtime_policy"
            else:
                test_name = "real_build_registry_digest_rolls_out_and_failed_rollout_remains_recoverable"
        log_path = evidence / "acceptance.log"
        with log_path.open("xb") as log:
            os.chmod(log_path, 0o600)
            process = subprocess.Popen(["cargo", "test", "--locked", "-p", "spitzli-agent", "--no-default-features",
                "--features", "kubernetes", "--test", "kubernetes_build_runtime", "--", "--ignored", "--exact",
                test_name, "--nocapture"],
                cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                result_code = process.wait(timeout=900)
            except BaseException:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
                raise
        # Logs contain only owned fixture status and test assertions, no provider credentials.
        sys.stdout.write(log_path.read_text(errors="replace"))
    finally:
        if mirror_bytes is not None:
            owned_node(record)
            existing = run(["docker", "exec", record["container"], "cat", MIRRORS], check=False)
            if existing.returncode == 0 and existing.stdout == mirror_bytes:
                run(["docker", "exec", record["container"], "rm", MIRRORS])
                run(["docker", "restart", record["container"]])
                wait_ready(record, kubeconfig)
            else:
                cleanup_ok = False
        if registry_id is not None:
            registry = inspect_container(registry_id)
            if (registry["Id"] == registry_id and registry["Name"] == "/" + name and
                    registry["Config"]["Labels"].get("webdock.build-runtime-registry") == nonce):
                run(["docker", "rm", "-f", registry_id])
            else:
                cleanup_ok = False
        proof = {"passed": result_code == 0, "cleanupVerified": cleanup_ok and result_code == 0, "ociFile": str(oci),
                 "runFixture": None if publish_only else record["name"], "publicationOnly": publish_only,
                 "healthOnly": health_only,
                 "registryException": "anonymous HTTP only inside owned local Docker fixture"}
        (evidence / "result.json").write_text(json.dumps(proof, indent=2) + "\n")
        (evidence / "result.json").chmod(0o600)
    if not cleanup_ok:
        raise RuntimeError("owned cleanup could not be proved; inspect retained fixture")
    return result_code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--oci", required=True, help="absolute private validated raw OCI archive from actual K3s build")
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--create-run", action="store_true", help="create and finally remove a fresh pinned local RUN K3s fixture (cached Docker images required)")
    mode.add_argument("--publish-only", action="store_true", help="push and read back any validated actual build OCI using only an owned loopback registry (no RUN node)")
    parser.add_argument("--health-only", action="store_true", help="verify generic OCI startup, exact digest and HTTP under native nonroot/read-only policy; skip static fixture recovery")
    parser.add_argument("--ownership", default=str(ROOT / ".runtime-test/build-runtime/ownership.json"))
    parser.add_argument("--kubeconfig", default=str(ROOT / ".runtime-test/build-runtime/owned.kubeconfig"))
    args = parser.parse_args()
    if args.health_only and args.publish_only:
        parser.error("--health-only requires runtime acceptance, not --publish-only")
    oci = private_file(args.oci)
    if oci.stat().st_size > 512 * 1024 * 1024:
        raise RuntimeError("OCI acceptance input exceeds retained output limit")
    if args.publish_only:
        return acceptance(oci, publish_only=True)
    if args.create_run:
        record, ownership, kubeconfig = create_run()
        try:
            return acceptance(oci, ownership, kubeconfig, health_only=args.health_only)
        finally:
            cleanup_created_run(record)
            private_file(kubeconfig).unlink()
            write_private(ownership.parent / "cleanup.json", json.dumps({"nonce": record["nonce"], "containerRemoved": True,
                "ownedVolumeRemoved": True, "privateKubeconfigRemoved": True}).encode() + b"\n")
            print("Removed fresh owned RUN node, volumes and private kubeconfig", flush=True)
    return acceptance(oci, private_file(args.ownership), private_file(args.kubeconfig), health_only=args.health_only)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:
        print(f"runtime acceptance failed: {error}", file=sys.stderr)
        sys.exit(1)
