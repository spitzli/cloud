#!/usr/bin/env python3
"""Admin-only server dry runs of the exact qualified template; creates no workloads."""
import argparse
import copy
import json
from pathlib import Path
import subprocess

NS = 'webdock-build-qualification'
BUILD_KIT = 'docker.io/moby/buildkit:v0.34.1-rootless@sha256:f68752a5a94acfcdad2d07867b7075601a421638e17fc5f28efc1c814d0c4f89'


def templates(observed):
    assert observed['kind'] == 'Job' and observed['metadata']['namespace'] == NS
    job = {'apiVersion': 'batch/v1', 'kind': 'Job',
           'metadata': {key: copy.deepcopy(observed['metadata'][key]) for key in ['name','namespace','labels','annotations'] if key in observed['metadata']},
           'spec': copy.deepcopy(observed['spec'])}
    for key in ['selector','manualSelector']:
        job['spec'].pop(key,None)
    metadata=job['spec']['template']['metadata']
    for key in ['uid','resourceVersion','creationTimestamp','managedFields']:
        metadata.pop(key,None)
    for key in ['batch.kubernetes.io/controller-uid','batch.kubernetes.io/job-name','controller-uid','job-name']:
        metadata.get('labels',{}).pop(key,None)
    return job


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    connection=parser.add_mutually_exclusive_group(required=True)
    connection.add_argument('--on-build-host',action='store_true')
    connection.add_argument('--kubeconfig')
    parser.add_argument('--job-json',required=True,help='Observed exact runner Job, retained privately after qualification')
    args=parser.parse_args()
    cli=['k3s','kubectl'] if args.on_build_host else ['kubectl','--kubeconfig',args.kubeconfig]
    def call(parts,body=None):
        return subprocess.run(cli+parts,input=None if body is None else json.dumps(body),text=True,capture_output=True,timeout=30)
    def get(parts):
        result=call(parts)
        if result.returncode:raise RuntimeError('Explicit build-cluster inspection failed')
        return json.loads(result.stdout)
    nodes=get(['get','nodes','-o','json'])['items']
    if len(nodes)!=1 or nodes[0]['metadata']['name']!='webdock-build-eu-01':raise RuntimeError('Refusing a foreign cluster')
    for name in ['webdock-build-template','webdock-build-handoff']:
        policy=get(['get','validatingadmissionpolicy',name,'-o','json'])
        if policy.get('status',{}).get('typeChecking',{}).get('expressionWarnings',[]):raise RuntimeError('Admission type-check warnings')
    job=templates(json.loads(Path(args.job_json).read_text()))
    def extra_container(p):
        container=copy.deepcopy(p['containers'][0])|{'name':'other'}
        container['resources']={key:{'cpu':'100m','memory':'512Mi','ephemeral-storage':'64Mi'} for key in ['requests','limits']}
        p['containers'].append(container)
    probes={
        'diagnostic-bypasses-command-pin': lambda p:p['containers'][0].update(image=BUILD_KIT,command=['buildctl-daemonless.sh'],args=['--oci-worker-no-process-sandbox']),
        'runner-process-sandbox-override': lambda p:p['containers'][0].update(args=['--oci-worker-no-process-sandbox']),
        'init-idmap-escalation': lambda p:p['initContainers'][0]['securityContext'].update(allowPrivilegeEscalation=True),
        'init-idmap-capabilities': lambda p:p['initContainers'][0]['securityContext']['capabilities'].update(add=['SETUID','SETGID']),
        'shadow-runner-executable': lambda p:p['containers'][0]['volumeMounts'][0].update(mountPath='/usr/local/bin'),
        'shadow-tools-policy': lambda p:p['initContainers'][0]['volumeMounts'][0].update(mountPath='/etc/webdock-build'),
        'additional-main-container': extra_container,
        'host-network':lambda p:p.update(hostNetwork=True),
        'host-path':lambda p:p['volumes'].append({'name':'host','hostPath':{'path':'/'}}),
        'service-token':lambda p:p.update(automountServiceAccountToken=True),
        'unconfined':lambda p:p['containers'][0]['securityContext'].update(seccompProfile={'type':'Unconfined'}),
        'privileged':lambda p:p['containers'][0]['securityContext'].update(privileged=True),
        'foreign-runtime':lambda p:p.update(runtimeClassName='runc'),
    }
    results={}
    def dry_run(candidate,expected,label):
        for kind in ['Job','Pod']:
            body=candidate
            if kind=='Pod':
                metadata=copy.deepcopy(candidate['spec']['template']['metadata'])
                metadata.update(name='qualified-admission-dry-run',namespace=NS)
                metadata.setdefault('labels',{})['batch.kubernetes.io/job-name']=candidate['metadata']['name']
                body={'apiVersion':'v1','kind':'Pod','metadata':metadata,'spec':candidate['spec']['template']['spec']}
            result=call(['create','--dry-run=server','-f','-'],body)
            if (result.returncode==0)!=expected:
                raise RuntimeError(f'{label}/{kind}: unexpected admission result: {result.stderr[-2000:]}')
            if not expected and label in list(probes)[:7] and 'webdock-build-template' not in result.stderr:
                raise RuntimeError(f'{label}/{kind}: denial did not establish the template admission boundary')
            results[f'{label}/{kind}']='allowed' if expected else 'denied'
    dry_run(job,True,'approved-template')
    for label,mutation in probes.items():
        candidate=copy.deepcopy(job);mutation(candidate['spec']['template']['spec'])
        dry_run(candidate,False,label)
    print(json.dumps(results,indent=2))


if __name__=='__main__':
    main()
