#!/usr/bin/env python3
"""Render-only checks: no Kubernetes connection or external credentials."""
import json
import subprocess
import tempfile
from pathlib import Path
import yaml


def render(chart, values):
    with tempfile.TemporaryDirectory(prefix="owned-scaling-chart-") as directory:
        path = Path(directory) / "values.json"
        path.write_text(json.dumps(values))
        raw = subprocess.check_output(["helm", "template", "fixture", chart, "-f", str(path)], text=True)
    return [item for item in yaml.safe_load_all(raw) if item]


image = "registry.example/app@sha256:" + "a" * 64
for enabled in [False, True]:
    objects = render("deploy/helm/webdock-agent", {
        "enabled": True, "image": image, "existingSecret": "owned", "existingClaim": "owned",
        "namespaces": ["owned-apps"], "autoscalingEnabled": enabled,
    })
    deployment = next(item for item in objects if item["kind"] == "Deployment")
    pod = deployment["spec"]["template"]["spec"]
    agent = next(container for container in pod["containers"] if container["name"] == "agent")
    assert {entry["name"]: entry["value"] for entry in agent.get("env", [])}["WEBDOCK_AGENT_AUTOSCALING_ENABLED"] == str(enabled).lower()
    role = next(item for item in objects if item["kind"] == "Role")
    assert role["metadata"]["namespace"] == "owned-apps"
    rules = next(rule for rule in role["rules"] if rule["apiGroups"] == ["autoscaling"])
    assert set(rules["verbs"]) == ({"get", "list", "create", "update", "delete"} if enabled else {"get", "list"})

values = {"name": "wd-fixture-456", "image": image, "labels": {"webdock.dev/managed-by":"webdock","webdock.dev/installation":"fixture","webdock.dev/project-id":"123","webdock.dev/resource-id":"456"}, "specHash":"a"*64,"replicas": 3}
for policy, expected in [(None, 3), ({"minReplicas": 1, "maxReplicas": 3, "targetCPUUtilizationPercent": 70}, None)]:
    objects = render("deploy/helm/webdock-plugin", {**values, "autoscaling": policy})
    deployment = next(item for item in objects if item["kind"] == "Deployment")
    assert deployment["spec"].get("replicas") == expected
    assert all(item["kind"] != "HorizontalPodAutoscaler" for item in objects), "HPA custody belongs to the guarded agent"
objects = render("deploy/helm/webdock-plugin", {**values, "replicas": 0, "autoscaling": None})
assert next(item for item in objects if item["kind"] == "Deployment")["spec"]["replicas"] == 0
print("PASS agent opt-in, namespace-scoped HPA permissions and single replica authority")
