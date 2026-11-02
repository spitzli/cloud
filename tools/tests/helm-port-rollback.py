#!/usr/bin/env python3
"""Real Helm/Kubernetes strategic-patch regression against an owned in-memory API.
No cluster, Pods, credentials or workloads are created. kubectl patch --local
implements Kubernetes strategic patch semantics; this fixture supplies storage,
discovery, admission and synthetic readiness for the rendered runtime objects.
"""
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

RESOURCES = {
    "/api/v1": [("configmaps", "ConfigMap"), ("secrets", "Secret"), ("serviceaccounts", "ServiceAccount"), ("services", "Service"), ("pods", "Pod"), ("persistentvolumeclaims", "PersistentVolumeClaim")],
    "/apis/apps/v1": [("deployments", "Deployment"), ("replicasets", "ReplicaSet")],
    "/apis/networking.k8s.io/v1": [("networkpolicies", "NetworkPolicy")],
}
OBJECTS = {}
LOCK = threading.RLock()
NEXT = 0
PATCHES = []
DELETES = []


def status(code, reason):
    return {"apiVersion": "v1", "kind": "Status", "status": "Failure", "code": code, "reason": reason, "message": reason}


def defaults(obj):
    global NEXT
    obj = copy.deepcopy(obj)
    NEXT += 1
    meta = obj.setdefault("metadata", {})
    meta.setdefault("uid", "owned-" + str(NEXT))
    meta["resourceVersion"] = str(NEXT)
    if obj["kind"] == "Service":
        obj["spec"].setdefault("clusterIP", "10.222.0.2")
    if obj["kind"] == "PersistentVolumeClaim":
        obj["spec"].setdefault("volumeName", "owned-retained-volume")
        obj["status"] = {"phase": "Bound"}
    if obj["kind"] == "Deployment":
        meta["generation"] = NEXT
        replicas = obj["spec"]["replicas"]
        ready = replicas if obj["spec"]["template"]["spec"]["containers"][0]["ports"][0]["containerPort"] == 8080 else 0
        obj["status"] = {"observedGeneration": NEXT, "replicas": replicas, "updatedReplicas": replicas, "readyReplicas": ready, "availableReplicas": ready}
    return obj


def protobuf_fields(data):
    # client-go's ConfigMap release store sends protobuf even with JSON responses.
    # Decode only this trusted fixture vocabulary; no production parser uses it.
    def varint(offset):
        value = shift = 0
        while True:
            byte = data[offset]
            offset += 1
            value |= (byte & 127) << shift
            if byte < 128:
                return value, offset
            shift += 7
    offset = 0
    while offset < len(data):
        tag, offset = varint(offset)
        wire = tag & 7
        if wire == 2:
            size, offset = varint(offset)
            value = data[offset:offset + size]
            offset += size
        elif wire == 0:
            value, offset = varint(offset)
        else:
            size = {1: 8, 5: 4}[wire]
            value = data[offset:offset + size]
            offset += size
        yield tag >> 3, value


def configmap_body(data):
    if not data.startswith(b"k8s\x00"):
        return json.loads(data)
    raw = dict(protobuf_fields(data[4:]))[2]
    obj = {"apiVersion": "v1", "kind": "ConfigMap", "metadata": {}, "data": {}}
    for number, value in protobuf_fields(raw):
        if number == 1:
            for field, entry in protobuf_fields(value):
                if field in [1, 3, 5, 6]:
                    obj["metadata"][{1: "name", 3: "namespace", 5: "uid", 6: "resourceVersion"}[field]] = entry.decode()
                if field in [11, 12]:
                    pair = dict(protobuf_fields(entry))
                    obj["metadata"].setdefault({11: "labels", 12: "annotations"}[field], {})[pair[1].decode()] = pair[2].decode()
        if number == 2:
            pair = dict(protobuf_fields(value))
            obj["data"][pair[1].decode()] = pair[2].decode()
    return obj


class API(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, code, obj):
        data = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        url = urlparse(self.path)
        path = url.path
        if path == "/version":
            return self.reply(200, {"major": "1", "minor": "35", "gitVersion": "v1.35.0", "gitCommit": "owned-fixture", "platform": "linux/amd64"})
        if path == "/api":
            return self.reply(200, {"kind": "APIVersions", "apiVersion": "v1", "versions": ["v1"]})
        if path == "/apis":
            return self.reply(200, {"kind": "APIGroupList", "apiVersion": "v1", "groups": [{"name": group, "versions": [{"groupVersion": group + "/v1", "version": "v1"}], "preferredVersion": {"groupVersion": group + "/v1", "version": "v1"}} for group in ["apps", "networking.k8s.io"]]})
        if path in RESOURCES:
            group = path.removeprefix("/apis/") if path.startswith("/apis/") else "v1"
            return self.reply(200, {"kind": "APIResourceList", "apiVersion": "v1", "groupVersion": group, "resources": [{"name": name, "singularName": "", "namespaced": True, "kind": kind, "verbs": ["get", "list", "create", "update", "patch", "delete"]} for name, kind in RESOURCES[path]]})
        with LOCK:
            if path in OBJECTS:
                return self.reply(200, OBJECTS[path])
            resource = path.rsplit("/", 1)[-1]
            if resource == "replicasets":
                deployments = [obj for obj in OBJECTS.values() if obj["kind"] == "Deployment"]
                replicas = []
                for deployment in deployments:
                    meta = deployment["metadata"]
                    replicas.append({"apiVersion": "apps/v1", "kind": "ReplicaSet", "metadata": {"name": meta["name"] + "-owned", "namespace": meta["namespace"], "uid": "owned-replicaset", "labels": deployment["spec"]["template"]["metadata"]["labels"], "annotations": {"deployment.kubernetes.io/revision": "1"}, "ownerReferences": [{"apiVersion": "apps/v1", "kind": "Deployment", "name": meta["name"], "uid": meta["uid"], "controller": True}]}, "spec": {"replicas": deployment["spec"]["replicas"], "selector": deployment["spec"]["selector"], "template": deployment["spec"]["template"]}, "status": {"replicas": deployment["status"]["replicas"], "readyReplicas": deployment["status"]["readyReplicas"]}})
                return self.reply(200, {"apiVersion": "apps/v1", "kind": "ReplicaSetList", "metadata": {}, "items": replicas})
            known = next((kind for values in RESOURCES.values() for name, kind in values if name == resource), None)
            if known:
                items = [obj for key, obj in OBJECTS.items() if key.rsplit("/", 1)[0] == path]
                selector = parse_qs(url.query).get("labelSelector", [""])[0]
                for condition in filter(None, selector.split(",")):
                    key, value = condition.split("=", 1)
                    items = [obj for obj in items if obj["metadata"].get("labels", {}).get(key) == value]
                return self.reply(200, {"kind": known + "List", "apiVersion": "v1" if resource != "deployments" else "apps/v1", "metadata": {}, "items": items})
            return self.reply(404, status(404, "NotFound"))

    def body(self):
        size = int(self.headers["Content-Length"])
        assert size < 2 * 1024 * 1024
        return self.rfile.read(size)

    def do_POST(self):
        value = configmap_body(self.body())
        path = urlparse(self.path).path + "/" + value["metadata"]["name"]
        with LOCK:
            if path in OBJECTS:
                return self.reply(409, status(409, "AlreadyExists"))
            value.setdefault("metadata", {})["namespace"] = "owned-runtime"
            OBJECTS[path] = defaults(value)
            return self.reply(201, OBJECTS[path])

    def do_PUT(self):
        value = configmap_body(self.body())
        with LOCK:
            OBJECTS[urlparse(self.path).path] = defaults(value)
            return self.reply(200, OBJECTS[urlparse(self.path).path])

    def do_PATCH(self):
        data = self.body()
        path = urlparse(self.path).path
        with LOCK:
            current = OBJECTS[path]
            with tempfile.NamedTemporaryFile(mode="w") as file:
                json.dump(current, file)
                file.flush()
                kind = "strategic" if "strategic-merge-patch" in self.headers["Content-Type"] else "merge"
                patch = subprocess.run(["kubectl", "patch", "--local", "--type", kind, "-f", file.name, "-p", data.decode(), "-o", "json"], capture_output=True, text=True, timeout=10, env=ENV)
            if patch.returncode:
                return self.reply(422, status(422, patch.stderr))
            result = json.loads(patch.stdout)
            ports = result.get("spec", {}).get("ports", [])
            if result["kind"] == "Deployment":
                ports = result["spec"]["template"]["spec"]["containers"][0].get("ports", [])
            names = [p["name"] for p in ports if "name" in p]
            PATCHES.append({"kind": result["kind"], "patch": json.loads(data), "resultPorts": ports})
            if len(names) != len(set(names)):
                return self.reply(422, status(422, 'Duplicate value: "runtime"'))
            OBJECTS[path] = defaults(result)
            return self.reply(200, OBJECTS[path])

    def do_DELETE(self):
        with LOCK:
            DELETES.append(urlparse(self.path).path)
            OBJECTS.pop(urlparse(self.path).path, None)
            return self.reply(200, {"kind": "Status", "apiVersion": "v1", "status": "Success", "code": 200})


if __name__ == "__main__":
    chart = str(Path(sys.argv[1]).resolve())
    values = [str(Path(p).resolve()) for p in sys.argv[2:5]]
    options = json.loads(sys.argv[5])
    with tempfile.TemporaryDirectory(prefix="webdock-helm-port-") as private:
        ENV = {"PATH": os.environ["PATH"], "HOME": private, "HELM_CACHE_HOME": private + "/cache", "HELM_CONFIG_HOME": private + "/config", "HELM_DATA_HOME": private + "/data", "HELM_DRIVER": "configmap", "KUBECONFIG": private + "/kubeconfig"}
        server = ThreadingHTTPServer(("127.0.0.1", 0), API)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        Path(ENV["KUBECONFIG"]).write_text(json.dumps({"apiVersion": "v1", "kind": "Config", "clusters": [{"name": "owned", "cluster": {"server": "http://127.0.0.1:" + str(server.server_port)}}], "users": [{"name": "owned", "user": {}}], "contexts": [{"name": "owned", "context": {"cluster": "owned", "user": "owned", "namespace": "owned-runtime"}}], "current-context": "owned"}))
        base = ["helm", "upgrade", "--install", "owned-runtime", chart, "--namespace", "owned-runtime", "--kubeconfig", ENV["KUBECONFIG"], "--kube-context", "owned", "--disable-openapi-validation", *options, "--timeout", "500ms"]
        try:
            first = subprocess.run([*base, "--values", values[0]], capture_output=True, text=True, timeout=20, env=ENV)
            assert first.returncode == 0, first.stdout + first.stderr
            with LOCK:
                initial_claim = next(obj for obj in OBJECTS.values() if obj["kind"] == "PersistentVolumeClaim")
                initial_claim_uid = initial_claim["metadata"]["uid"]
            failed = subprocess.run([*base, "--values", values[1]], capture_output=True, text=True, timeout=20, env=ENV)
            assert failed.returncode != 0, "fixture port8081 must fail synthetic readiness"
            if "--atomic" in options:
                with LOCK:
                    rolled = next(obj for obj in OBJECTS.values() if obj["kind"] == "Deployment")
                    prior = json.loads(Path(values[0]).read_text())
                    assert rolled["spec"]["template"]["spec"]["containers"][0]["image"] == prior["image"]
                    assert rolled["spec"]["template"]["spec"]["containers"][0]["ports"] == [{"name": "runtime", "containerPort": 8080}]
                saved = subprocess.run(["helm", "get", "values", "owned-runtime", "--namespace", "owned-runtime", "--kubeconfig", ENV["KUBECONFIG"], "--revision", "1", "-o", "json"], capture_output=True, text=True, timeout=10, env=ENV)
                assert saved.returncode == 0, saved.stderr
                assert json.loads(saved.stdout) == prior, "failure overwrote previous working release values"
            restored = subprocess.run([*base, "--values", values[2]], capture_output=True, text=True, timeout=20, env=ENV)
            assert restored.returncode == 0, restored.stdout + restored.stderr + "\npatches=" + json.dumps(PATCHES)
            with LOCK:
                deployment = next(obj for obj in OBJECTS.values() if obj["kind"] == "Deployment")
                service = next(obj for obj in OBJECTS.values() if obj["kind"] == "Service")
                assert deployment["spec"]["template"]["spec"]["containers"][0]["ports"] == [{"name": "runtime", "containerPort": 8080}]
                assert service["spec"]["ports"] == [{"name": "runtime", "port": 8080, "targetPort": "runtime"}]
                assert deployment["spec"]["strategy"]["type"] == "Recreate"
                claims = [obj for obj in OBJECTS.values() if obj["kind"] == "PersistentVolumeClaim"]
                assert len(claims) == 1 and claims[0]["spec"]["volumeName"] == "owned-retained-volume"
                assert claims[0]["metadata"]["uid"] == initial_claim_uid
                assert not DELETES, "upgrade/rollback must not delete retained resources"
            print("PASS actual Helm8080 -> failed8081 -> restored8080; unique Service/container ports; prior release recoverable; no live cluster")
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=3)
