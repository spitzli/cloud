#!/usr/bin/env python3
"""Explicit build-host-only finite fork probe; no customer code, credentials or host mounts."""
import argparse
import copy
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import threading
import time

IMAGE = "docker.io/library/webdock-build-runner@sha256:81a0228242287a8800d26dbecd68136593ed652590e62123c987e8876b197409"


def kubectl(*args, value=None, timeout=15):
    result = subprocess.run(["/usr/local/bin/k3s", "kubectl", *args],
                            input=None if value is None else json.dumps(value),
                            text=True, capture_output=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError("owned PID fixture Kubernetes operation failed: " + args[0])
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--on-build-host", action="store_true", required=True)
    parser.parse_args()
    if os.geteuid() != 0 or socket.gethostname() != "webdock-build-eu-01" or not Path("/etc/webdock-build-host").is_file():
        raise SystemExit("Requires root on the explicitly owned dedicated build host")
    config = json.loads(kubectl("get", "--raw", "/api/v1/nodes/webdock-build-eu-01/proxy/configz"))["kubeletconfig"]
    assert config["podPidsLimit"] == 512, "install the reviewed finite kubelet PID profile first"
    for key in ["systemReserved", "kubeReserved"]:
        assert int(config[key]["pid"]) >= 1000
    for namespace in ["webdock-builds", "webdock-build-qualification"]:
        assert not json.loads(kubectl("get", "pods", "-n", namespace, "-o", "json"))["items"], "active build; refusing abuse probe"
    run = secrets.token_hex(10)
    name = "wd-pid-proof-" + run
    labels = {"webdock.dev/pid-fixture": run}
    namespace_uid = None
    observer_stop = threading.Event()
    observer = None
    report = {"run": run, "podPidsLimit": 512, "finiteForkAttempts": 900,
              "runtimeClass": "webdock-build-sandbox", "kernelPidLimitHit": False,
              "apiMaxSeconds": 0, "peakHostTasks": 0, "customerBuildNamespaceUsed": False}
    try:
        ns = json.loads(kubectl("create", "-f", "-", "-o", "json", value={"apiVersion": "v1", "kind": "Namespace", "metadata": {
            # Only the pre-reviewed rootless ID-map control needs SETUID/SETGID.
            # This root-owned temporary namespace admits no customer-supplied work.
            "name": name, "labels": {**labels, "pod-security.kubernetes.io/enforce": "baseline"}}}))
        namespace_uid = ns["metadata"]["uid"]
        kubectl("create", "-f", "-", value={"apiVersion": "networking.k8s.io/v1", "kind": "NetworkPolicy",
                "metadata": {"name": "deny-all", "namespace": name}, "spec": {"podSelector": {}, "policyTypes": ["Ingress", "Egress"]}})
        pod = {"apiVersion": "v1", "kind": "Pod", "metadata": {"name": "finite-fork", "namespace": name, "labels": labels}, "spec": {
            "runtimeClassName": "webdock-build-sandbox", "restartPolicy": "Never", "activeDeadlineSeconds": 60,
            "terminationGracePeriodSeconds": 2, "automountServiceAccountToken": False,
            "securityContext": {"runAsNonRoot": True, "runAsUser": 1000, "runAsGroup": 1000, "seccompProfile": {"type": "RuntimeDefault"}},
            "containers": [{"name": "probe", "image": IMAGE, "imagePullPolicy": "Never",
                "command": ["/bin/sh", "-c", "printf 'webdock-pid-probe-started\\n'; /bin/sleep 1; i=0; while [ \"$i\" -lt 900 ]; do /bin/sleep 40 & i=$((i+1)); done; printf 'bounded_attempts=%s\\n' \"$i\"; wait"],
                "securityContext": {"allowPrivilegeEscalation": False, "readOnlyRootFilesystem": True, "capabilities": {"drop": ["ALL"]}},
                "resources": {"requests": {"cpu": "1", "memory": "512Mi", "ephemeral-storage": "16Mi"},
                              "limits": {"cpu": "1", "memory": "512Mi", "ephemeral-storage": "16Mi"}}}]}}
        control = copy.deepcopy(pod)
        control["metadata"]["name"] = "buildkit-positive-control"
        control["spec"]["securityContext"]["fsGroup"] = 1000
        control["spec"]["volumes"] = [{"name": n, "emptyDir": {"sizeLimit": "64Mi"}} for n in ["workspace", "cache", "tmp"]]
        container = control["spec"]["containers"][0]
        container["command"] = ["/usr/local/bin/webdock-build-entrypoint"]
        container["args"] = ["proxy-url", "--tools", "/etc/webdock-build/tools.json"]
        container["volumeMounts"] = [{"name": n, "mountPath": "/" + n} for n in ["workspace", "cache", "tmp"]]
        container["securityContext"]["allowPrivilegeEscalation"] = True
        container["securityContext"]["capabilities"]["add"] = ["SETUID", "SETGID"]
        created_control = json.loads(kubectl("create", "-f", "-", "-o", "json", value=control))
        report["positiveControlUID"] = created_control["metadata"]["uid"]
        control_deadline = time.monotonic() + 65
        while time.monotonic() < control_deadline:
            state = json.loads(kubectl("get", "pod", "buildkit-positive-control", "-n", name, "-o", "json"))
            assert state["metadata"]["uid"] == report["positiveControlUID"]
            if state.get("status", {}).get("phase") in ["Succeeded", "Failed"]:
                report["positiveControlPhase"] = state["status"]["phase"]
                report["positiveControlLogs"] = kubectl("logs", "buildkit-positive-control", "-n", name, "--tail=15", "--limit-bytes=2000")
                assert state["status"]["phase"] == "Succeeded", "finite PID cap prevented qualified rootless BuildKit startup"
                assert report["positiveControlLogs"].strip() == "http://10.83.0.1:3128"
                break
            time.sleep(0.25)
        else:
            raise RuntimeError("BuildKit positive control exceeded hard deadline")
        created = json.loads(kubectl("create", "-f", "-", "-o", "json", value=pod))
        uid = created["metadata"]["uid"]
        report["podUID"] = uid
        def observe_cgroup():
            group = None
            while not observer_stop.is_set():
                try:
                    if group is None:
                        groups = list(Path("/sys/fs/cgroup/kubepods.slice").glob("**/*pod" + uid.replace("-", "_") + ".slice"))
                        if len(groups) > 1:
                            report["observerError"] = "ambiguous Pod cgroup"
                            return
                        group = groups[0] if groups else None
                    if group is not None:
                        tasks = int((group / "pids.current").read_text())
                        limit = (group / "pids.max").read_text().strip()
                        # Kubelet initially creates an empty cgroup, then sets its
                        # budget. A no-task creation/teardown interval is not execution.
                        if limit == "max" and tasks == 0:
                            observer_stop.wait(0.01)
                            continue
                        if limit != "512":
                            report["observerError"] = "Pod cgroup PID limit drift"
                            return
                        report["peakHostTasks"] = max(report["peakHostTasks"], tasks)
                        # Capture child and ancestor counters before fast sandbox teardown.
                        for events_path in group.glob("**/pids.events"):
                            events = dict(line.split() for line in events_path.read_text().splitlines())
                            report["kernelPidLimitHit"] |= int(events.get("max", "0")) > 0
                except FileNotFoundError:
                    pass
                except Exception as error:
                    report["observerError"] = type(error).__name__
                    return
                observer_stop.wait(0.01 if group is not None else 0.1)
        observer = threading.Thread(target=observe_cgroup, daemon=True)
        observer.start()
        deadline = time.monotonic() + 80
        phase = "Pending"
        while time.monotonic() < deadline:
            current = json.loads(kubectl("get", "pod", "finite-fork", "-n", name, "-o", "json"))
            assert current["metadata"]["uid"] == uid
            phase = current.get("status", {}).get("phase", "Pending")
            started = time.monotonic()
            assert kubectl("get", "--raw", "/readyz").strip() == "ok"
            report["apiMaxSeconds"] = max(report["apiMaxSeconds"], round(time.monotonic() - started, 3))
            if phase in ["Failed", "Succeeded"]:
                report["containerStates"] = [s.get("state") for s in current.get("status", {}).get("containerStatuses", [])]
                break
            time.sleep(0.25)
        else:
            raise RuntimeError("finite PID probe exceeded its hard deadline")
        report["phase"] = phase
        report["logs"] = kubectl("logs", "finite-fork", "-n", name, "--tail=15", "--limit-bytes=2000")
        observer_stop.set()
        observer.join(timeout=2)
        assert not observer.is_alive() and "observerError" not in report
        assert "webdock-pid-probe-started\n" in report["logs"], "fork probe never started; runtime initialization is not a successful abuse test"
        assert report["peakHostTasks"] <= 512
        assert report["kernelPidLimitHit"], "host PID denial not observed; do not certify guest process count"
        assert not any(s.get("terminated", {}).get("reason") == "OOMKilled" for s in report["containerStates"])
        report["status"] = "host-pid-boundary-qualified; guest-process-semantics-require-separate-interpretation"
    finally:
        observer_stop.set()
        if observer is not None:
            observer.join(timeout=2)
        if namespace_uid is not None:
            current = json.loads(kubectl("get", "namespace", name, "-o", "json"))
            assert current["metadata"]["uid"] == namespace_uid and current["metadata"]["labels"]["webdock.dev/pid-fixture"] == run
            kubectl("delete", "namespace", name, "--wait=true", "--timeout=60s", timeout=70)
            report["exactOwnedNamespaceRemoved"] = True
        print(json.dumps(report, indent=2), flush=True)


if __name__ == "__main__":
    main()
