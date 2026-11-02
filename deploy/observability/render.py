#!/usr/bin/env python3
"""Fetch verified public charts OR render/check an offline candidate. Never deploys."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile
import urllib.request

import yaml

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
MAX_ARCHIVE = 32 * 1024**2


def require(condition, message):
    if not condition:
        raise ValueError(message)


def verified_bytes(data, chart):
    require(len(data) <= MAX_ARCHIVE, "chart archive exceeds fetch bound")
    require(hashlib.sha256(data).hexdigest() == chart["sha256"], "chart SHA256 mismatch")
    return data


def archive(cache, chart, fetch):
    path = cache / f'{chart["name"]}-{chart["version"]}.tgz'
    if fetch:
        require(chart["url"].startswith("https://github.com/"), "only locked upstream HTTPS release URLs")
        with urllib.request.urlopen(chart["url"], timeout=30) as response:
            data = verified_bytes(response.read(MAX_ARCHIVE + 1), chart)
        cache.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(dir=cache, delete=False) as output:
            temporary = Path(output.name)
            output.write(data)
        temporary.replace(path)
    verified_bytes(path.read_bytes(), chart)
    # Inspect metadata only; never extract upstream paths into the workspace.
    with tarfile.open(path) as content:
        metadata = yaml.safe_load(content.extractfile(f'{chart["name"]}/Chart.yaml'))
    require(metadata["name"] == chart["name"] and metadata["version"] == chart["version"], "chart identity mismatch")
    require(metadata["appVersion"] == chart["appVersion"], "chart application version mismatch")
    return path


def quantity(value):
    match = re.fullmatch(r"([0-9.]+)(m|Ki|Mi|Gi|Ti|KB|MB|GB)?", str(value))
    require(match is not None, "unrecognized bounded resource quantity")
    return float(match[1]) * {None: 1, "m": .001, "Ki": 1024, "Mi": 1024**2,
                             "Gi": 1024**3, "Ti": 1024**4, "KB": 1000,
                             "MB": 1000**2, "GB": 1000**3}[match[2]]


def check(objects, lock):
    namespace = "webdock-observability"
    cpu = memory = disk = 0
    images = set()
    reloader = None
    dashboards = 0
    loki = grafana = prometheus = alertmanager = isolation = None
    for obj in objects:
        kind, metadata, spec = obj["kind"], obj["metadata"], obj.get("spec", {})
        name = metadata["name"]
        require(metadata.get("namespace", namespace) == namespace, "unexpected resource namespace")
        require(kind not in {"Ingress", "Gateway", "HTTPRoute", "DaemonSet"}, "public route or unreviewed host collector")
        if kind == "Service":
            require(spec.get("type", "ClusterIP") == "ClusterIP" and not spec.get("externalIPs"), "public service")
        if kind == "NetworkPolicy":
            require(name == "obs-unqualified-deny-all" and spec == {
                "podSelector": {}, "policyTypes": ["Ingress", "Egress"], "ingress": [], "egress": []
            }, "network must remain closed until qualification")
            isolation = obj
        require("gateway" not in name and "canary" not in name and "minio" not in name, "unexpected Loki gateway/canary/store")
        if kind == "ConfigMap":
            dashboards += int(metadata.get("labels", {}).get("grafana_dashboard") == "1")
            if "grafana.ini" in obj.get("data", {}):
                grafana = obj["data"]["grafana.ini"]
            if "config.yaml" in obj.get("data", {}) and metadata.get("labels", {}).get("app.kubernetes.io/name") == "loki":
                loki = yaml.safe_load(obj["data"]["config.yaml"])
        if kind in {"Prometheus", "Alertmanager"}:
            if kind == "Prometheus":
                prometheus = spec
            else:
                alertmanager = spec
            require(spec["replicas"] == 1, "only one metrics/alert replica qualified for rendering")
            images.add(spec["image"])
            resources = spec["resources"]
            for key in ["cpu", "memory", "ephemeral-storage"]:
                require(0 < quantity(resources["requests"][key]) <= quantity(resources["limits"][key]), "unbounded operator workload")
            cpu += quantity(resources["limits"]["cpu"])
            memory += quantity(resources["limits"]["memory"])
        pod = spec.get("template", {}).get("spec")
        if pod:
            require(not any(pod.get(key) for key in ["hostNetwork", "hostPID", "hostIPC"]), "host namespace prohibited")
            require(not any("hostPath" in volume for volume in pod.get("volumes") or []), "host mount prohibited")
            require(spec.get("replicas", 1) == 1, "unexpected workload replicas")
            for container in (pod.get("containers") or []) + (pod.get("initContainers") or []):
                images.add(container["image"])
                flags = dict(arg[2:].split("=", 1) for arg in container.get("args", []) if arg.startswith("--") and "=" in arg)
                if "prometheus-config-reloader" in flags:
                    images.add(flags["prometheus-config-reloader"])
                    reloader = flags
                security = container.get("securityContext", {})
                require(not security.get("privileged") and security.get("allowPrivilegeEscalation") is False, "privileged container")
                inherited = {**pod.get("securityContext", {}), **security}
                require(inherited.get("runAsNonRoot") is True and inherited.get("runAsUser", 1) != 0, "root container prohibited")
                require(inherited.get("seccompProfile", {}).get("type") == "RuntimeDefault", "default seccomp required")
                require("ALL" in security.get("capabilities", {}).get("drop", []) and not security.get("capabilities", {}).get("add"), "capability escalation prohibited")
                require(not any(port.get("hostPort") for port in container.get("ports", [])), "host port prohibited")
                resources = container.get("resources", {})
                for key in ["cpu", "memory", "ephemeral-storage"]:
                    require(0 < quantity(resources.get("requests", {}).get(key, 0)) <= quantity(resources.get("limits", {}).get(key, 0)), "missing resource bounds")
                cpu += quantity(resources["limits"]["cpu"])
                memory += quantity(resources["limits"]["memory"])
            if name == "obs-loki":
                require(pod.get("automountServiceAccountToken") is False, "Loki must not receive an API token")
                require(pod["containers"][0]["envFrom"] == [{"secretRef": {"name": "obs-loki-s3-unconfigured", "optional": False}}], "S3 must remain a private unbound Secret reference")
            if name == "obs-grafana":
                refs = [env.get("valueFrom", {}).get("secretKeyRef", {}).get("name") for c in pod["containers"] for env in c.get("env", [])]
                require("obs-grafana-admin-unconfigured" in refs, "Grafana must use existing private admin Secret")
        claims = []
        if kind == "PersistentVolumeClaim":
            claims.append(spec)
        claims.extend(claim["spec"] for claim in spec.get("volumeClaimTemplates", []))
        if kind in {"Prometheus", "Alertmanager"}:
            claims.append(spec["storage"]["volumeClaimTemplate"]["spec"])
        for claim in claims:
            require(claim["storageClassName"] == "webdock-obs-storage-unqualified", "storage must remain unbound")
            size = quantity(claim["resources"]["requests"]["storage"])
            require(0 < size <= quantity("30Gi"), "unbounded persistent disk")
            disk += size
    require(isolation is not None, "missing deny-all isolation")
    require(dashboards >= 10, "upstream default dashboards missing")
    require(grafana is not None and "[auth.anonymous]\nenabled = false" in grafana and "disable_login_form = true" in grafana, "Grafana authentication gate weakened")
    require(loki is not None and loki["auth_enabled"] is True, "Loki tenant mode missing")
    require(loki["common"]["replication_factor"] == 1, "Loki replica factor mismatch")
    require(loki["common"]["storage"]["s3"]["endpoint"] == "${OBS_S3_ENDPOINT}", "actual S3 binding must remain unset")
    require(loki["schema_config"]["configs"][0]["schema"] == "v13", "expected Loki TSDB v13")
    require(loki["limits_config"]["retention_period"] == "336h" and loki["compactor"]["retention_enabled"] is True, "Loki retention missing")
    require(loki["limits_config"]["ingestion_rate_mb"] <= 2 and loki["limits_config"]["max_global_streams_per_user"] <= 1000, "Loki ingest bounds weakened")
    require(prometheus and prometheus["retention"] == "15d" and prometheus["retentionSize"] == "20GB", "metrics retention missing")
    require(not prometheus.get("enableRemoteWriteReceiver") and not prometheus.get("enableAdminAPI"), "unreviewed metrics receiver/admin API")
    require(alertmanager is not None and alertmanager["retention"] == "120h", "alert state retention missing")
    # Two operator-generated config reloaders are not materialized by helm template.
    require(reloader is not None, "missing generated config-reloader bounds")
    for resource in ["cpu", "memory"]:
        require(0 < quantity(reloader[f"config-reloader-{resource}-request"]) <= quantity(reloader[f"config-reloader-{resource}-limit"]), "unbounded config reloader")
    cpu += 2 * quantity(reloader["config-reloader-cpu-limit"])
    memory += 2 * quantity(reloader["config-reloader-memory-limit"])
    require(cpu <= 4 and memory <= quantity("6Gi") and disk <= quantity("53Gi"), "candidate resource budget exceeded")
    pinned = set()
    for image in images:
        require(re.search(r"@sha256:[0-9a-f]{64}$", image) is not None, "image lacks immutable digest")
        repository, digest = image.split("@")
        repository = repository.rsplit(":", 1)[0] if ":" in repository.rsplit("/", 1)[-1] else repository
        pinned.add(repository + "@" + digest)
    require(pinned == {entry["reference"].rsplit(":", 1)[0] + "@" + entry["digest"] for entry in lock["images"]}, "rendered images differ from registry lock")
    return {"objects": len(objects), "upstreamDashboards": dashboards, "containerMemoryLimitBytesIncludingTwoReloaders": int(memory),
            "containerCpuLimitsIncludingTwoReloaders": cpu, "requestedPersistentBytes": int(disk), "immutableImages": sorted(images)}


def self_test(objects, lock):
    cases = []
    bad = copy.deepcopy(objects)
    next(obj for obj in bad if obj["kind"] == "Service")["spec"]["type"] = "LoadBalancer"
    cases.append(("public service", bad))
    cases.append(("missing isolation", [obj for obj in objects if obj["kind"] != "NetworkPolicy"]))
    bad = copy.deepcopy(objects)
    next(obj for obj in bad if "grafana.ini" in obj.get("data", {}))["data"]["grafana.ini"] = "[auth.anonymous]\nenabled = true\n"
    cases.append(("anonymous Grafana", bad))
    bad = copy.deepcopy(objects)
    next(obj for obj in bad if obj["kind"] == "Deployment")["spec"]["template"]["spec"]["containers"][0]["resources"] = {}
    cases.append(("unbounded container", bad))
    bad = copy.deepcopy(objects)
    next(obj for obj in bad if obj["kind"] == "Deployment")["spec"]["template"]["spec"]["containers"][0]["image"] = "quay.io/kiwigrid/k8s-sidecar:latest"
    cases.append(("floating image", bad))
    for label, bad in cases:
        try:
            check(bad, lock)
        except ValueError:
            continue
        raise ValueError(f"negative control unexpectedly accepted: {label}")
    try:
        verified_bytes(b"owned corrupted archive", lock["charts"][0])
    except ValueError:
        return len(cases) + 1
    raise ValueError("corrupted archive accepted")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, default=ROOT / ".runtime-test/observability-cache")
    parser.add_argument("--output", type=Path, default=ROOT / ".runtime-test/observability-render")
    parser.add_argument("--fetch", action="store_true", help="network access only to locked public archive URLs")
    parser.add_argument("--qualify-offline", action="store_true", help="render inert candidate; never install")
    args = parser.parse_args()
    lock = json.loads((HERE / "charts.lock.json").read_text())
    require(lock["activationEnabled"] is False, "activation is intentionally unavailable in this helper")
    if not args.fetch and not args.qualify_offline:
        print(json.dumps({"activationEnabled": False, "renderedObjects": 0}))
        return
    archives = [(chart, archive(args.cache, chart, args.fetch)) for chart in lock["charts"]]
    if not args.qualify_offline:
        print("Verified two locked upstream chart archives; no rendering or activation.")
        return
    rendered = []
    # Empty Helm homes and client-only template: no ambient kube context, repo or plugin dependency.
    with tempfile.TemporaryDirectory(prefix="webdock-obs-helm-") as temporary:
        env = {**os.environ, "KUBECONFIG": "/dev/null", "HELM_CACHE_HOME": temporary,
               "HELM_CONFIG_HOME": temporary, "HELM_DATA_HOME": temporary, "HELM_PLUGINS": temporary}
        helm_version = subprocess.run(["helm", "version", "--short"], env=env, capture_output=True, text=True, timeout=10, check=True).stdout.strip().split("+")[0]
        require(helm_version == lock["helmQualifiedVersion"], "use the qualified Helm version or review and update its lock")
        for chart, path in archives:
            result = subprocess.run(["helm", "template", "obs-metrics" if chart["name"] == "kube-prometheus-stack" else "obs-loki",
                                     str(path.resolve()), "--namespace", lock["namespace"], "--kube-version", lock["kubernetesRenderVersion"],
                                     "--skip-tests", "-f", str(HERE / chart["values"])], env=env, capture_output=True, text=True, timeout=60, check=True)
            rendered.append(result.stdout)
    rendered.append((HERE / "isolation.yaml").read_text())
    objects = [obj for text in rendered for obj in yaml.safe_load_all(text) if obj]
    report = check(objects, lock)
    report["negativeControlsRejected"] = self_test(objects, lock)
    report["activationEnabled"] = False
    report["runtimeQualified"] = False
    report["s3MfaTenantGatewayQualified"] = False
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "candidate.yaml").write_text("\n---\n".join(rendered))
    (args.output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
