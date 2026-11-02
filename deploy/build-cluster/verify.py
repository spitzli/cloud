#!/usr/bin/env python3
"""Explicit build-cluster checks; negative tests are server-side dry runs only."""
import argparse
import copy
import json
from pathlib import Path
import subprocess
import sys


def run(cli, args, data=None, ok=True):
    result = subprocess.run(cli + args, input=json.dumps(data) if data else None,
                            text=True, capture_output=True, timeout=35)
    if ok and result.returncode:
        raise RuntimeError(result.stderr)
    return result


def main():
    parser=argparse.ArgumentParser()
    parser.add_argument('--on-build-host',action='store_true')
    parser.add_argument('--kubeconfig')
    parser.add_argument('--job-json',required=True,help='JSON rendering of smoke.yaml')
    args=parser.parse_args()
    if args.on_build_host == bool(args.kubeconfig):
        parser.error('Select exactly one explicit connection: --on-build-host or --kubeconfig')
    cli=['k3s','kubectl'] if args.on_build_host else ['kubectl','--kubeconfig',args.kubeconfig]
    nodes=json.loads(run(cli,['get','nodes','-o','json']).stdout)['items']
    if len(nodes)!=1 or nodes[0]['metadata']['name']!='webdock-build-eu-01':
        raise RuntimeError('This is not the expected standalone build cluster')
    warnings=json.loads(run(cli,['get','validatingadmissionpolicy','webdock-build-template','-o','json']).stdout).get('status',{}).get('typeChecking',{}).get('expressionWarnings',[])
    if warnings: raise RuntimeError('Admission expressions have type-checking warnings')
    baseline=json.loads(Path(args.job_json).read_text())
    baseline['metadata']['name']='policy-dry-run'
    run(cli,['create','--dry-run=server','-f','-'],baseline)
    cases={
        'privileged':lambda j: j['spec']['template']['spec']['containers'][0]['securityContext'].update(privileged=True,allowPrivilegeEscalation=True),
        'host-network':lambda j: j['spec']['template']['spec'].update(hostNetwork=True),
        'host-path':lambda j: j['spec']['template']['spec'].update(volumes=[{'name':'host','hostPath':{'path':'/'}}]),
        'service-token':lambda j: j['spec']['template']['spec'].update(automountServiceAccountToken=True),
        'root':lambda j: j['spec']['template']['spec']['containers'][0]['securityContext'].update(runAsUser=0),
        'seccomp-unconfined':lambda j: j['spec']['template']['spec']['containers'][0]['securityContext'].update(seccompProfile={'type':'Unconfined'}),
        'apparmor-unconfined':lambda j: j['spec']['template']['spec']['containers'][0]['securityContext'].update(appArmorProfile={'type':'Unconfined'}),
        'mutable-image':lambda j: j['spec']['template']['spec']['containers'][0].update(image='busybox:latest'),
        'secret-env':lambda j: j['spec']['template']['spec']['containers'][0].update(envFrom=[{'secretRef':{'name':'platform-secret'}}]),
        'unsafe-init':lambda j: j['spec']['template']['spec'].update(initContainers=[{'name':'unsafe','image':'busybox:latest','command':['true']}]),
        'fanout':lambda j: j['spec'].update(parallelism=2),
        'ttl-before-capture':lambda j: j['spec'].update(ttlSecondsAfterFinished=1),
        'node-override':lambda j: j['spec']['template']['spec'].update(nodeName='webdock-build-eu-01'),
        'unbounded-deadline':lambda j: j['spec'].update(activeDeadlineSeconds=999999),
    }
    results={}
    for name,mutate in cases.items():
        job=copy.deepcopy(baseline); mutate(job)
        denied=run(cli,['create','--dry-run=server','-f','-'],job,ok=False)
        if denied.returncode==0 or not any(word in denied.stderr.lower() for word in ['denied','forbidden']):
            raise RuntimeError(f'{name}: expected admission denial, got {denied.stderr}')
        results[name]='denied'
    actor='system:serviceaccount:webdock-builds:webdock-build-dispatcher'
    for verb,resource,expected in [('create','jobs',True),('get','pods/log',True),('get','secrets',False),('create','pods/exec',False),('create','pods',False),('get','nodes',False)]:
        result=run(cli,['auth','can-i',verb,resource,'-n','webdock-builds','--as='+actor],ok=False)
        if (result.stdout.strip()=='yes')!=expected:
            raise RuntimeError(f'Unexpected dispatcher authority: {verb} {resource}')
        results[f'rbac:{verb}:{resource}']='allowed' if expected else 'denied'
    results['admission-type-warnings']=warnings
    print(json.dumps(results,indent=2))


if __name__=='__main__':
    try: main()
    except (RuntimeError,subprocess.TimeoutExpired) as error:
        print(str(error),file=sys.stderr); sys.exit(1)
