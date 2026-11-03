"""Provision a UID-fenced plugin namespace boundary without changing admission rules."""
import argparse
import hashlib
import json
import re
import subprocess


def documents(namespace, uid, fingerprint, cpu, memory):
    if (not re.fullmatch(r"wd-plugin-[a-f0-9]{24}", namespace)
            or not re.fullmatch(r"[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}", uid)
            or not re.fullmatch(r"[a-f0-9]{64}", fingerprint)
            or not 10 <= cpu <= 64000 or not 8388608 <= memory <= 68719476736):
        raise ValueError("Exact bounded plugin namespace identity and resources required")
    cpu = str(cpu // 1000) if cpu % 1000 == 0 else f"{cpu}m"
    for scale, suffix in [(1073741824, "Gi"), (1048576, "Mi"),
                          (1000000000, "G"), (1000000, "M"), (1000, "k"), (1024, "Ki")]:
        if memory % scale == 0:
            memory = f"{memory // scale}{suffix}"
            break
    else:
        memory = str(memory)
    def obj(kind, name, spec):
        return {"apiVersion": "networking.k8s.io/v1" if kind == "NetworkPolicy" else "v1",
                "kind": kind, "metadata": {"name": name, "namespace": namespace,
                "annotations": {"webdock.dev/spec-sha256": fingerprint},
                "ownerReferences": [{"apiVersion": "v1", "kind": "Namespace", "name": namespace,
                                     "uid": uid, "controller": False, "blockOwnerDeletion": False}]}, "spec": spec}
    return [obj("ResourceQuota", "runtime-boundary", {"hard": {
        "pods": "1", "requests.cpu": cpu, "limits.cpu": cpu,
        "requests.memory": memory, "limits.memory": memory,
        "requests.ephemeral-storage": "16Mi", "limits.ephemeral-storage": "16Mi",
        "persistentvolumeclaims": "0", "requests.storage": "0",
        "services": "1", "services.nodeports": "0", "services.loadbalancers": "0",
        "secrets": "1", "configmaps": "1", "count/deployments.apps": "1",
        "count/replicasets.apps": "4"}}),
        obj("LimitRange", "runtime-boundary", {"limits": [{"type": "Container",
            "max": {"cpu": cpu, "memory": memory, "ephemeral-storage": "16Mi"},
            "min": {"cpu": "10m", "memory": "8Mi", "ephemeral-storage": "1Mi"},
            "default": {"cpu": cpu, "memory": memory, "ephemeral-storage": "16Mi"},
            "defaultRequest": {"cpu": cpu, "memory": memory, "ephemeral-storage": "16Mi"}}]}),
        obj("NetworkPolicy", "boundary-deny", {"podSelector": {}, "policyTypes": ["Ingress", "Egress"]})]


def verify(actual, desired):
    if (actual["metadata"].get("deletionTimestamp")
            or actual["metadata"].get("ownerReferences") != desired["metadata"]["ownerReferences"]
            or actual["metadata"].get("annotations", {}).get("webdock.dev/spec-sha256")
            != desired["metadata"]["annotations"]["webdock.dev/spec-sha256"]
            or actual["spec"] != desired["spec"]):
        raise ValueError("Existing boundary object differs; refusing adoption")


def policy_digest(policy):
    return hashlib.sha256(json.dumps(policy["spec"], sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--kubeconfig", required=True)
    parser.add_argument("--namespace", required=True)
    parser.add_argument("--namespace-uid", required=True)
    parser.add_argument("--spec-sha256", required=True)
    parser.add_argument("--runtime-policy-sha256", required=True)
    parser.add_argument("--cpu-millicores", type=int, required=True)
    parser.add_argument("--memory-bytes", type=int, required=True)
    parser.add_argument("--apply", action="store_true")
    a = parser.parse_args()
    items = documents(a.namespace, a.namespace_uid, a.spec_sha256, a.cpu_millicores, a.memory_bytes)
    k = ["kubectl", "--kubeconfig", a.kubeconfig]
    def get(kind, name):
        return json.loads(subprocess.check_output([*k, "get", kind, name, "-n", a.namespace,
                                                  "--ignore-not-found", "-o", "json"]) or b"null")
    def namespace():
        ns = get("namespace", a.namespace)
        if (not ns or ns["metadata"]["uid"] != a.namespace_uid
                or ns["metadata"].get("deletionTimestamp")
                or ns["metadata"].get("annotations", {}).get("webdock.dev/spec-sha256") != a.spec_sha256
                or ns["metadata"].get("labels", {}).get("pod-security.kubernetes.io/enforce") != "restricted"):
            raise ValueError("Namespace custody or Restricted Pod Security changed")
        return ns
    namespace()
    def isolation():
        policy = get("networkpolicy", "runtime")
        if (not re.fullmatch(r"[a-f0-9]{64}", a.runtime_policy_sha256)
            or not policy or policy["metadata"].get("annotations", {}).get("webdock.dev/spec-sha256") != a.spec_sha256
            or set(policy["spec"].get("policyTypes", [])) != {"Ingress", "Egress"}
            or not policy["spec"].get("podSelector", {}).get("matchLabels")
            or policy_digest(policy) != a.runtime_policy_sha256):
            raise ValueError("Exact reviewed runtime isolation policy required before boundary activation")
    isolation()
    for item in items:
        existing = get(item["kind"], item["metadata"]["name"])
        if existing:
            verify(existing, item)
        elif a.apply:
            namespace()
            subprocess.run([*k, "create", "--field-manager=webdock-plugin-boundary", "-f", "-"],
                           input=json.dumps(item).encode(), check=True, capture_output=True)
    if not a.apply:
        print(json.dumps({"apiVersion": "v1", "kind": "List", "items": items}, indent=2))
        return
    for item in items:
        verify(get(item["kind"], item["metadata"]["name"]), item)
    isolation()
    ns = namespace()
    patch = [{"op": "test", "path": "/metadata/uid", "value": a.namespace_uid},
             {"op": "test", "path": "/metadata/resourceVersion", "value": ns["metadata"]["resourceVersion"]},
             {"op": "add", "path": "/metadata/labels/security.webdock.dev~1ready", "value": "true"}]
    subprocess.run([*k, "patch", "namespace", a.namespace, "--type=json", "--patch-file=/dev/stdin"],
                   input=json.dumps(patch).encode(), check=True, capture_output=True)
    print(json.dumps({"namespace": a.namespace, "namespaceUID": a.namespace_uid,
                      "resourceQuota": True, "limitRange": True, "defaultDeny": True,
                      "restricted": True, "admissionPoliciesUnchanged": True}))


if __name__ == "__main__":
    main()
