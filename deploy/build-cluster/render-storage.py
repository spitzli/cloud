#!/usr/bin/env python3
"""Offline infrastructure only; rendering never enables fixed-volume customer execution."""
import argparse
import json
import re
from pathlib import Path
import yaml

def workload_policy():
    original=next(d for d in yaml.safe_load_all((Path(__file__).parent/"foundation.yaml").read_text()) if d and d["kind"]=="ValidatingAdmissionPolicy")
    spec=original["spec"]
    spec["variables"].extend([
        {"name":"hardStorage","expression":"variables.mappingHelper && has(namespaceObject.metadata.annotations) && 'webdock.dev/storage-profile' in namespaceObject.metadata.annotations && namespaceObject.metadata.annotations['webdock.dev/storage-profile'] == 'fixed-ext4-v1'"},
        {"name":"buildLabels","expression":"request.kind.kind == 'Job' ? object.spec.template.metadata.labels : object.metadata.labels"},
        {"name":"volumeClaim","expression":"['webdock.dev/build-id','webdock.dev/generation','webdock.dev/volume-id'].all(k, k in variables.buildLabels) ? 'wd-bv-' + variables.buildLabels['webdock.dev/build-id'] + '-' + variables.buildLabels['webdock.dev/generation'] + '-' + variables.buildLabels['webdock.dev/volume-id'] : 'invalid'"},
    ])
    for validation in spec["validations"]:
        if validation["message"]=="Only bounded per-pod emptyDir volumes are allowed.":
            legacy=validation["expression"].removeprefix("variables.deletionCleanup || ")
            validation["expression"]="variables.deletionCleanup || (variables.hardStorage ? (has(variables.p.volumes) && size(variables.p.volumes) == 3 && variables.p.volumes.all(v, (v.name == 'build' && has(v.persistentVolumeClaim) && v.persistentVolumeClaim.claimName == variables.volumeClaim && (!has(v.persistentVolumeClaim.readOnly) || !v.persistentVolumeClaim.readOnly)) || (v.name == 'tmp' && has(v.emptyDir) && has(v.emptyDir.medium) && v.emptyDir.medium == 'Memory' && has(v.emptyDir.sizeLimit) && quantity(v.emptyDir.sizeLimit).compareTo(quantity('64Mi')) == 0) || (v.name == 'handoff' && has(v.secret) && v.secret.secretName == variables.handoffName && v.secret.defaultMode == 288 && has(v.secret.items) && size(v.secret.items) == 1 && v.secret.items[0].key == 'attempt.json' && v.secret.items[0].path == 'attempt.json'))) : "+legacy+")"
            validation["message"]="Execution disk must use the exact attempt PVC and bounded scratch."
        elif validation["message"].startswith("Qualified mount paths"):
            legacy=validation["expression"].removeprefix("variables.deletionCleanup || ")
            mounts="has(variables.p.volumes) && size(variables.p.volumes) == 3 && variables.containers.all(c, has(c.volumeMounts) && size(c.volumeMounts) == (c.name == 'runner' ? 4 : 3) && c.volumeMounts.all(m, !has(m.subPathExpr) && !has(m.mountPropagation) && ((m.name == 'build' && m.mountPath == '/workspace' && has(m.subPath) && m.subPath == 'workspace' && (!has(m.readOnly) || !m.readOnly)) || (c.name == 'runner' && m.name == 'build' && m.mountPath == '/cache' && has(m.subPath) && m.subPath == 'cache' && (!has(m.readOnly) || !m.readOnly)) || (m.name == 'tmp' && m.mountPath == '/tmp' && !has(m.subPath) && (!has(m.readOnly) || !m.readOnly)) || (m.name == 'handoff' && m.mountPath == '/handoff/attempt.json' && m.subPath == 'attempt.json' && m.readOnly))))"
            validation["expression"]="variables.deletionCleanup || (variables.hardStorage ? ("+mounts+") : "+legacy+")"
    spec["validations"].extend([
        {"expression":"variables.deletionCleanup || !variables.hardStorage || request.kind.kind != 'Job' || request.operation != 'CREATE' || (has(dyn(object).spec.suspend) && dyn(object).spec.suspend)","message":"Fixed builds must be created suspended until result authority is UID-bound."},
        {"expression":"variables.deletionCleanup || !variables.hardStorage || (['webdock.dev/customer-id','webdock.dev/project-id','webdock.dev/build-id','webdock.dev/volume-id'].all(k,k in variables.buildLabels && variables.buildLabels[k].matches('^[1-9][0-9]{0,18}$')) && 'webdock.dev/generation' in variables.buildLabels && variables.buildLabels['webdock.dev/generation'].matches('^[1-9][0-9]{0,9}$') && (request.kind.kind != 'Job' || ['webdock.dev/customer-id','webdock.dev/project-id','webdock.dev/build-id','webdock.dev/volume-id','webdock.dev/generation'].all(k,k in object.metadata.labels && object.metadata.labels[k] == variables.buildLabels[k])))","message":"Fixed volumes are bound to exact tenant, build and attempt labels."},
        {"expression":"variables.deletionCleanup || variables.containers.all(c,!has(c.volumeMounts) || c.volumeMounts.all(m,!has(m.subPathExpr) && !has(m.mountPropagation)))","message":"Dynamic subpaths and mount propagation are forbidden."},
    ])
    return original

def render(node="webdock-build-eu-01",pool_root="/var/lib/webdock-build-storage",execution_namespace="webdock-build-qualification"):
    if not re.fullmatch(r"[a-z0-9][a-z0-9.-]{0,252}",node) or not re.fullmatch(r"/var/lib/webdock-build-[a-z0-9/-]+",pool_root) or ".." in pool_root or not re.fullmatch(r"webdock-build-[a-z0-9-]{1,48}",execution_namespace):
        raise ValueError("Explicit build-only node, pool and namespace required")
    system="webdock-build-storage-system";requests="webdock-build-storage-requests";receipts="webdock-build-storage-receipts"
    result=[]
    for namespace,channel,capacity in [(system,"system",0),(requests,"request",128),(receipts,"reply",256)]:
        result.append({"apiVersion":"v1","kind":"Namespace","metadata":{"name":namespace,"labels":{"webdock.dev/build-storage-channel":channel,"pod-security.kubernetes.io/enforce":"restricted"}}})
        if capacity:
            result.append({"apiVersion":"v1","kind":"ResourceQuota","metadata":{"name":"bounded-storage-metadata","namespace":namespace},"spec":{"hard":{"configmaps":str(capacity),"pods":"0","secrets":"0","persistentvolumeclaims":"0","services":"0"}}})
    result.append({"apiVersion":"v1","kind":"ServiceAccount","metadata":{"name":"webdock-build-storage-node","namespace":system},"automountServiceAccountToken":False})
    def rule(resources,verbs,group=""):
        return {"apiGroups":[group],"resources":resources,"verbs":verbs}
    def role(name,namespace,rules,account,account_ns):
        result.extend([
            {"apiVersion":"rbac.authorization.k8s.io/v1","kind":"Role","metadata":{"name":name,"namespace":namespace},"rules":rules},
            {"apiVersion":"rbac.authorization.k8s.io/v1","kind":"RoleBinding","metadata":{"name":name,"namespace":namespace},"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"Role","name":name},"subjects":[{"kind":"ServiceAccount","name":account,"namespace":account_ns}]},
        ])
    role("build-volume-node",requests,[rule(["configmaps"],["get","list"])],"webdock-build-storage-node",system)
    role("build-volume-node",receipts,[rule(["configmaps"],["get","list","create","delete"])],"webdock-build-storage-node",system)
    role("build-volume-node",execution_namespace,[rule(["persistentvolumeclaims"],["get","create","delete"]),rule(["pods"],["list"]),rule(["jobs"],["get"],"batch")],"webdock-build-storage-node",system)
    role("build-volume-producer",requests,[rule(["configmaps"],["get","create","delete"])],"webdock-build-dispatcher",execution_namespace)
    role("build-volume-producer",receipts,[rule(["configmaps"],["get","delete"])],"webdock-build-dispatcher",execution_namespace)
    role("build-volume-observer",execution_namespace,[rule(["persistentvolumeclaims"],["get"]),rule(["jobs"],["patch"],"batch")],"webdock-build-dispatcher",execution_namespace)
    result.extend([
        {"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRole","metadata":{"name":"webdock-build-volume-node"},"rules":[rule(["persistentvolumes"],["get","create","delete"])]},
        {"apiVersion":"rbac.authorization.k8s.io/v1","kind":"ClusterRoleBinding","metadata":{"name":"webdock-build-volume-node"},"roleRef":{"apiGroup":"rbac.authorization.k8s.io","kind":"ClusterRole","name":"webdock-build-volume-node"},"subjects":[{"kind":"ServiceAccount","name":"webdock-build-storage-node","namespace":system}]},
    ])
    def policy(name,spec):
        result.extend([
            {"apiVersion":"admissionregistration.k8s.io/v1","kind":"ValidatingAdmissionPolicy","metadata":{"name":name},"spec":{"failurePolicy":"Fail",**spec}},
            {"apiVersion":"admissionregistration.k8s.io/v1","kind":"ValidatingAdmissionPolicyBinding","metadata":{"name":name},"spec":{"policyName":name,"validationActions":["Deny"]}},
        ])
    policy("webdock-build-storage-metadata",{
        "matchConstraints":{"namespaceSelector":{"matchExpressions":[{"key":"webdock.dev/build-storage-channel","operator":"In","values":["request","reply"]}]},"resourceRules":[{"apiGroups":[""],"apiVersions":["v1"],"operations":["CREATE","UPDATE"],"resources":["configmaps"]}]},
        "variables":[{"name":"isRequest","expression":"namespaceObject.metadata.labels['webdock.dev/build-storage-channel'] == 'request'"}],
        "validations":[
            {"expression":"has(object.immutable) && object.immutable && object.metadata.name.matches('^wd-vq-[1-9][0-9]{0,18}-[1-9][0-9]{0,15}-[a-f0-9]{16}$')","message":"Storage metadata must be immutable and operation-scoped."},
            {"expression":"(!has(object.binaryData) || size(object.binaryData) == 0) && has(object.data) && size(object.data) == 1 && (variables.isRequest ? ('request.json' in object.data && size(object.data['request.json']) <= 8192) : ('reply.json' in object.data && size(object.data['reply.json']) <= 16384))","message":"One bounded typed storage frame is required."},
            {"expression":"has(object.metadata.annotations) && 'webdock.dev/request-sha256' in object.metadata.annotations && object.metadata.annotations['webdock.dev/request-sha256'].matches('^[a-f0-9]{64}$')","message":"The full operation fingerprint is required."},
        ]})
    path_pattern="^"+re.escape(pool_root)+"/[1-9][0-9]{0,18}/mount/data$"
    # json.dumps produces a CEL-compatible escaped literal for the path regex.
    policy("webdock-build-storage-local-volume",{
        "matchConstraints":{"resourceRules":[{"apiGroups":[""],"apiVersions":["v1"],"operations":["CREATE","UPDATE"],"resources":["persistentvolumes"]}]},
        "matchConditions":[{"name":"dedicated-node-identity","expression":"request.userInfo.username == "+json.dumps("system:serviceaccount:"+system+":webdock-build-storage-node")}],
        "validations":[
            {"expression":"has(object.spec.local) && object.spec.local.path.matches("+json.dumps(path_pattern)+") && object.spec.volumeMode == 'Filesystem' && object.spec.accessModes == ['ReadWriteOnce'] && object.spec.persistentVolumeReclaimPolicy == 'Retain' && (!has(object.spec.storageClassName) || object.spec.storageClassName == '')","message":"Only fixed build-pool filesystem volumes may be created."},
            {"expression":"has(object.spec.claimRef) && object.spec.claimRef.namespace == "+json.dumps(execution_namespace)+" && object.spec.claimRef.name == object.metadata.name && has(object.spec.claimRef.uid) && object.spec.claimRef.uid.matches('^[A-Za-z0-9_-]{1,128}$')","message":"The exact namespaced claim UID must be prebound."},
            {"expression":"has(object.spec.nodeAffinity) && has(object.spec.nodeAffinity.required) && size(object.spec.nodeAffinity.required.nodeSelectorTerms) == 1 && size(object.spec.nodeAffinity.required.nodeSelectorTerms[0].matchExpressions) == 1 && object.spec.nodeAffinity.required.nodeSelectorTerms[0].matchExpressions[0].key == 'kubernetes.io/hostname' && object.spec.nodeAffinity.required.nodeSelectorTerms[0].matchExpressions[0].operator == 'In' && object.spec.nodeAffinity.required.nodeSelectorTerms[0].matchExpressions[0].values == ["+json.dumps(node)+"]","message":"The verified local node is required."},
            {"expression":"'storage' in dyn(object).spec.capacity && quantity(string(dyn(object).spec.capacity['storage'])).isInteger() && quantity(string(dyn(object).spec.capacity['storage'])).compareTo(quantity('64Mi')) >= 0 && quantity(string(dyn(object).spec.capacity['storage'])).compareTo(quantity('32Gi')) <= 0","message":"Fixed volume capacity must remain bounded."},
        ]})
    result.append(workload_policy())
    return result

if __name__=="__main__":
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument("--node",default="webdock-build-eu-01");parser.add_argument("--pool-root",default="/var/lib/webdock-build-storage");args=parser.parse_args()
    try:print(json.dumps({"apiVersion":"v1","kind":"List","items":render(args.node,args.pool_root)}))
    except ValueError as error:parser.error(str(error))
