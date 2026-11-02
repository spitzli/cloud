#!/usr/bin/python3
"""Own only the new host's package gateway, its UID firewall and port-3128 Pod admission."""
import argparse
import datetime
import fcntl
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import pwd
import re
import socket
import stat
import subprocess
import tempfile

HOST = "webdock-build-eu-01"
PUBLIC = "167.86.75.144"
PRIVATE = "10.83.0.1"
PACKAGE_VERSION = "6.14-0ubuntu0.24.04.5"
STATE = Path("/etc/webdock-build-package-proxy/owner.json")
MANAGER = Path("/usr/local/libexec/webdock-package-proxy-manage")
NFT = Path("/etc/webdock-build-package-proxy/guard.nft")
COMMENT = "webdock-package-proxy-pods-only"
EXPECTED_RULE = 'ip saddr 10.84.0.0/16 ip daddr 10.83.0.1 tcp dport 3128 accept comment "' + COMMENT + '"'
BUNDLE = Path(__file__).resolve().parent
FILES = {
    "squid.conf": Path("/etc/squid/webdock-package-proxy.conf"),
    "hosts.allow": Path("/etc/squid/webdock-package-hosts.allow"),
    "guard.nft": NFT,
    "webdock-package-proxy.service": Path("/etc/systemd/system/webdock-package-proxy.service"),
    "webdock-package-proxy-firewall.service": Path("/etc/systemd/system/webdock-package-proxy-firewall.service"),
    "webdock-package-proxy-firewall-refresh.service": Path("/etc/systemd/system/webdock-package-proxy-firewall-refresh.service"),
    "webdock-package-proxy-firewall.timer": Path("/etc/systemd/system/webdock-package-proxy-firewall.timer"),
}
ROOTS = {"github.com", "githubusercontent.com", "ghcr.io", "nodejs.org", "npmjs.org", "mise.jdx.dev", "cdn.jdx.dev", "docker.io", "docker.com", "debian.org"}


def run(args, *, check=True, input=None):
    result = subprocess.run(args, input=input, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=180, check=False)
    if len(result.stdout) > 8 * 1024 * 1024 or len(result.stderr) > 1024 * 1024:
        raise RuntimeError("package_proxy_command_limit")
    if check and result.returncode:
        raise RuntimeError("package_proxy_command_failed")
    return result


def kubectl(*args):
    return run(["/usr/local/bin/k3s", "kubectl", "--kubeconfig", "/etc/rancher/k3s/k3s.yaml", "--context", "default", *args])


def assert_inventory(inventory):
    if (inventory["hostname"] != HOST or not {PUBLIC, PRIVATE}.issubset(inventory["addresses"])
            or inventory["os_id"] != "ubuntu" or inventory["os_version"] != "24.04"
            or inventory["nodes"] != [HOST] or inventory["proxy_uid"] != 13):
        raise RuntimeError("wrong_build_host")


def inventory(cluster=True):
    release = dict(line.split("=", 1) for line in Path("/etc/os-release").read_text().splitlines() if "=" in line)
    addresses = json.loads(run(["ip", "-j", "-4", "address"]).stdout)
    nodes = json.loads(kubectl("get", "nodes", "-o", "json").stdout)["items"] if cluster else [{"metadata": {"name": HOST}}]
    return {"hostname": socket.gethostname(), "addresses": [a["local"] for link in addresses for a in link["addr_info"]],
            "os_id": release["ID"].strip('"'), "os_version": release["VERSION_ID"].strip('"'),
            "nodes": [node["metadata"]["name"] for node in nodes], "proxy_uid": pwd.getpwnam("proxy").pw_uid}


def assert_idle_jobs(payload):
    for job in payload["items"]:
        if job.get("metadata", {}).get("labels", {}).get("webdock.dev/component") != "build-job":
            continue
        status = job.get("status", {})
        terminal = any(c.get("status") == "True" and c.get("type") in {"Complete", "Failed"} for c in status.get("conditions", []))
        if status.get("active", 0) or not terminal:
            raise RuntimeError("active_native_build_job")


def assert_default_deny():
    policies = json.loads(kubectl("get", "networkpolicy", "-n", "webdock-build-qualification", "-o", "json").stdout)["items"]
    if not any(p["spec"].get("podSelector", {}) == {} and {"Ingress", "Egress"}.issubset(p["spec"].get("policyTypes", []))
               and not p["spec"].get("egress", []) and not p["spec"].get("ingress", []) for p in policies):
        raise RuntimeError("namespace_default_deny_required")


def validate_hosts(text):
    hosts = []
    for line in text.splitlines():
        host = line.strip()
        if not host or host.startswith("#"):
            continue
        domain = host.removeprefix(".")
        if (not re.fullmatch(r"[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?", domain)
                or ".." in domain or not any(domain == root or domain.endswith("." + root) for root in ROOTS)):
            raise RuntimeError("invalid_package_host")
        try:
            ipaddress.ip_address(domain)
        except ValueError:
            pass
        else:
            raise RuntimeError("invalid_package_host")
        hosts.append(host)
    if not hosts or len(hosts) != len(set(hosts)) or len(hosts) > 40:
        raise RuntimeError("invalid_package_host")
    return hosts


def owned_bytes(path, maximum=1024 * 1024):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, "rb") as stream:
        metadata = os.fstat(stream.fileno())
        if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != 0 or metadata.st_mode & 0o022
                or metadata.st_nlink != 1 or metadata.st_size > maximum):
            raise RuntimeError("unowned_package_proxy_file")
        value = stream.read(maximum + 1)
        if len(value) > maximum:
            raise RuntimeError("package_proxy_file_limit")
        return value


def write(path, value, mode=0o644):
    path.parent.mkdir(mode=0o755, parents=True, exist_ok=True)
    if path.is_symlink():
        raise RuntimeError("unowned_package_proxy_file")
    with tempfile.NamedTemporaryFile(dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        os.fchmod(stream.fileno(), mode)
        stream.write(value)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)


def package_present(status):
    return status.strip() not in {"", "unknown ok not-installed"}


def package_version():
    return run(["dpkg-query", "-W", "-f=${Version}", "squid"]).stdout.decode()


def check_installed():
    record = json.loads(owned_bytes(STATE))
    if record.get("owner") != "webdock-package-proxy-v1" or record.get("phase") != "ready" or package_version() != PACKAGE_VERSION:
        raise RuntimeError("package_proxy_ownership_changed")
    for path, digest in record["files"].items():
        if hashlib.sha256(owned_bytes(Path(path))).hexdigest() != digest:
            raise RuntimeError("package_proxy_configuration_changed")
    if hashlib.sha256(owned_bytes(Path("/usr/sbin/squid"), 64 * 1024 * 1024)).hexdigest() != record["squidBinarySHA256"]:
        raise RuntimeError("package_proxy_binary_changed")
    return record


def firewall():
    if os.geteuid() != 0:
        raise RuntimeError("root_required")
    assert_inventory(inventory(cluster=False))
    check_installed()
    if run(["nft", "list", "table", "inet", "webdock_package_proxy"], check=False).returncode:
        run(["nft", "add", "table", "inet", "webdock_package_proxy"])
    # Atomic replacement affects our own table only; it also restores the independent deny boundary.
    run(["nft", "-f", str(NFT)])
    current = run(["nft", "-a", "list", "chain", "inet", "webdock_build_host", "input"]).stdout.decode()
    owned = [" ".join(line.split("# handle", 1)[0].split()) for line in current.splitlines() if COMMENT in line]
    if owned and owned != [EXPECTED_RULE]:
        raise RuntimeError("package_proxy_firewall_drift")
    if not owned:
        run(["nft", "add", "rule", "inet", "webdock_build_host", "input", "ip", "saddr", "10.84.0.0/16", "ip", "daddr", PRIVATE,
             "tcp", "dport", "3128", "accept", "comment", COMMENT])


def update_hosts():
    """Change only a reviewed endpoint list, preserving installed binary and firewall authority."""
    if os.geteuid() != 0:
        raise RuntimeError("root_required")
    assert_inventory(inventory())
    assert_idle_jobs(json.loads(kubectl("get", "jobs", "-A", "-o", "json").stdout))
    assert_default_deny()
    record = check_installed()
    destination = FILES["hosts.allow"]
    previous = owned_bytes(destination)
    approved = (BUNDLE / "hosts.allow").read_bytes()
    validate_hosts(approved.decode())
    try:
        write(destination, approved)
        run(["/usr/sbin/squid", "-k", "parse", "-f", "/etc/squid/webdock-package-proxy.conf"])
        run(["/usr/sbin/squid", "-k", "reconfigure", "-f", "/etc/squid/webdock-package-proxy.conf"])
    except Exception:
        write(destination, previous)
        run(["/usr/sbin/squid", "-k", "reconfigure", "-f", "/etc/squid/webdock-package-proxy.conf"], check=False)
        raise
    record["files"][str(destination)] = hashlib.sha256(approved).hexdigest()
    write(STATE, json.dumps(record, indent=2).encode())
    print(json.dumps({"status": "endpoint-list-updated", "hostsSHA256": record["files"][str(destination)]}))


def install(update=False):
    if os.geteuid() != 0:
        raise RuntimeError("root_required")
    observed = inventory()
    assert_inventory(observed)
    assert_idle_jobs(json.loads(kubectl("get", "jobs", "-A", "-o", "json").stdout))
    assert_default_deny()
    validate_hosts((BUNDLE / "hosts.allow").read_text())
    source = {str(destination): (BUNDLE / name).read_bytes() for name, destination in FILES.items()}
    source[str(MANAGER)] = Path(__file__).read_bytes()
    source["/etc/squid/webdock-package-proxy.hosts"] = b""
    source_hashes = {path: hashlib.sha256(data).hexdigest() for path, data in source.items()}
    if STATE.exists():
        previous = json.loads(owned_bytes(STATE))
        if previous.get("owner") != "webdock-package-proxy-v1":
            raise RuntimeError("unowned_package_proxy_state")
        if previous.get("files") != source_hashes and not update:
            raise RuntimeError("package_proxy_profile_changed")
        if previous.get("phase") == "ready":
            check_installed()
        for path, digest in previous.get("files", {}).items():
            if Path(path).exists() and hashlib.sha256(owned_bytes(Path(path))).hexdigest() != digest:
                raise RuntimeError("package_proxy_configuration_changed")
    else:
        if package_present(run(["dpkg-query", "-W", "-f=${Status}", "squid"], check=False).stdout.decode()):
            raise RuntimeError("unowned_squid_installation")
        if run(["ss", "-H", "-lnt", "sport", "=", ":3128"]).stdout:
            raise RuntimeError("unowned_proxy_listener")
        write(STATE, json.dumps({"owner": "webdock-package-proxy-v1", "phase": "installing", "files": source_hashes}).encode())
    run(["systemctl", "stop", "webdock-package-proxy-firewall.timer", "webdock-package-proxy-firewall-refresh.service", "webdock-package-proxy.service"], check=False)
    write(STATE, json.dumps({"owner": "webdock-package-proxy-v1", "phase": "installing", "files": source_hashes}).encode())
    # The stock unit must never start with a broad default listener during package installation.
    run(["systemctl", "mask", "--now", "squid.service"])
    environment = os.environ | {"DEBIAN_FRONTEND": "noninteractive", "NEEDRESTART_MODE": "l"}
    for arguments in [["apt-get", "update"], ["apt-get", "install", "-y", "--no-install-recommends", "squid=" + PACKAGE_VERSION]]:
        result = subprocess.run(arguments, env=environment, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=300, check=False)
        if result.returncode:
            raise RuntimeError("squid_package_install_failed")
    if package_version() != PACKAGE_VERSION:
        raise RuntimeError("squid_package_version_changed")
    for destination, data in source.items():
        write(Path(destination), data, 0o755 if destination == str(MANAGER) else 0o644)
    # Parse the actual installed maintained binary against the complete profile before starting it.
    run(["/usr/sbin/squid", "-k", "parse", "-f", "/etc/squid/webdock-package-proxy.conf"])
    run(["systemd-analyze", "verify", "/etc/systemd/system/webdock-package-proxy.service",
         "/etc/systemd/system/webdock-package-proxy-firewall.service",
         "/etc/systemd/system/webdock-package-proxy-firewall-refresh.service",
         "/etc/systemd/system/webdock-package-proxy-firewall.timer"])
    record = {"owner": "webdock-package-proxy-v1", "phase": "ready", "packageVersion": PACKAGE_VERSION, "proxyUID": 13,
              "hostname": HOST, "publicIPv4": PUBLIC, "privateIPv4": PRIVATE,
              "files": source_hashes, "squidBinarySHA256": hashlib.sha256(owned_bytes(Path("/usr/sbin/squid"), 64 * 1024 * 1024)).hexdigest(),
              "installedAt": datetime.datetime.now(datetime.timezone.utc).isoformat()}
    write(STATE, json.dumps(record, indent=2).encode())
    run(["systemctl", "daemon-reload"])
    firewall()
    kubectl("apply", "-f", str(BUNDLE / "network-policy.json"))
    run(["systemctl", "enable", "--now", "webdock-package-proxy-firewall.service", "webdock-package-proxy-firewall.timer", "webdock-package-proxy.service"])
    print(json.dumps({"status": "installed", "packageVersion": PACKAGE_VERSION, "squidBinarySHA256": record["squidBinarySHA256"], "listen": PRIVATE + ":3128"}))


def main():
    parser = argparse.ArgumentParser()
    action = parser.add_mutually_exclusive_group(required=True)
    action.add_argument("--install", action="store_true")
    action.add_argument("--firewall", action="store_true")
    action.add_argument("--check", action="store_true")
    action.add_argument("--update-hosts", action="store_true")
    parser.add_argument("--on-new-build-host", action="store_true")
    parser.add_argument("--update-owned-profile", action="store_true")
    args = parser.parse_args()
    try:
        if (args.install or args.update_hosts) and not args.on_new_build_host:
            raise RuntimeError("explicit_new_build_host_required")
        if args.update_hosts and not args.update_owned_profile:
            raise RuntimeError("explicit_profile_update_required")
        if args.check:
            check_installed()
            return
        name="install" if args.install or args.update_hosts else "firewall"
        descriptor=os.open("/run/lock/webdock-package-proxy-"+name+".lock",os.O_RDWR|os.O_CREAT|os.O_NOFOLLOW,0o600)
        with os.fdopen(descriptor,"a+b") as lock:
            metadata=os.fstat(lock.fileno())
            if metadata.st_uid!=0 or metadata.st_mode&0o077 or metadata.st_nlink!=1:raise RuntimeError("unowned_package_proxy_lock")
            fcntl.flock(lock, fcntl.LOCK_EX)
            if args.update_hosts:
                update_hosts()
            elif args.install:
                install(args.update_owned_profile)
            else:
                firewall()
    except RuntimeError as error:
        print(str(error), file=__import__("sys").stderr)
        raise SystemExit(1)
    except (OSError, ValueError, subprocess.TimeoutExpired):
        print("package_proxy_operation_failed", file=__import__("sys").stderr)
        raise SystemExit(1)

if __name__ == "__main__":
    main()
