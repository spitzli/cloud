"""Read-only PostgreSQL preflight. Never exports pod environments or Kubernetes Secrets."""
import argparse
from datetime import datetime, timezone
from decimal import Decimal, InvalidOperation, ROUND_CEILING
import json
from pathlib import Path
import re
import subprocess
import sys

DIMENSIONS = {'cpuMillicores': 'cpu', 'memoryBytes': 'memory', 'ephemeralBytes': 'ephemeral-storage'}
FACTORS = {'': 1, 'n': Decimal('1e-9'), 'u': Decimal('1e-6'), 'm': Decimal('.001'),
           'k': 1000, 'M': 10**6, 'G': 10**9, 'T': 10**12, 'P': 10**15, 'E': 10**18,
           **{unit + 'i': 1024**i for i, unit in enumerate('KMGTPE', 1)}}


def quantity(value, cpu=False):
    match = re.fullmatch(r'(\d+(?:\.\d*)?|\.\d+)([eE][+-]?\d+|[numkMGTPE]|[KMGTPE]i)?', str(value))
    if not match:
        raise ValueError('Invalid Kubernetes resource quantity')
    number, suffix = match.groups()
    suffix = suffix or ''
    try:
        factor = FACTORS[suffix] if suffix in FACTORS else Decimal('1' + suffix)
        result = Decimal(number) * factor * (1000 if cpu else 1)
        return int(result.to_integral_value(rounding=ROUND_CEILING))
    except (InvalidOperation, OverflowError) as error:
        raise ValueError('Invalid Kubernetes resource quantity') from error


def empty():
    return dict.fromkeys(DIMENSIONS, 0)


def resources(value):
    return {key: quantity(value.get(resource, '0'), cpu=resource == 'cpu')
            for key, resource in DIMENSIONS.items()}


def add(left, right):
    return {key: left[key] + right[key] for key in DIMENSIONS}


def maximum(left, right):
    return {key: max(left[key], right[key]) for key in DIMENSIONS}


def pod_resources(pod, kind):
    spec = pod.get('spec', {})
    regular = empty()
    for container in spec.get('containers', []):
        regular = add(regular, resources(container.get('resources', {}).get(kind, {})))
    sidecars, init_peak = empty(), empty()
    for container in spec.get('initContainers', []):
        demand = resources(container.get('resources', {}).get(kind, {}))
        if container.get('restartPolicy') == 'Always':
            sidecars = add(sidecars, demand)
            init_peak = maximum(init_peak, sidecars)
        else:
            init_peak = maximum(init_peak, add(sidecars, demand))
    result = maximum(add(regular, sidecars), init_peak)
    for key, resource in DIMENSIONS.items():
        override = spec.get('resources', {}).get(kind, {}).get(resource)
        if override is not None:
            result[key] = quantity(override, cpu=resource == 'cpu')
    return add(result, resources(spec.get('overhead', {})))


def summarize_cluster(nodes, pods):
    summaries = {}
    for node in nodes['items']:
        status, spec = node.get('status', {}), node.get('spec', {})
        allocatable = status.get('allocatable', {})
        if not {'cpu', 'memory'} <= allocatable.keys():
            raise ValueError('Node allocatable CPU/memory missing')
        name = node['metadata']['name']
        summaries[name] = {
            'name': name, 'kubeletVersion': status.get('nodeInfo', {}).get('kubeletVersion'),
            'allocatable': resources(allocatable), 'unschedulable': bool(spec.get('unschedulable')),
            'taints': [{'key': t['key'], 'effect': t['effect']} for t in spec.get('taints', [])],
            'conditions': {c['type']: c['status'] for c in status.get('conditions', [])},
            'requests': empty(), 'declaredLimits': empty(), 'unboundedLimitDimensions': [],
        }
    unplaced, workloads = empty(), []
    for pod in pods['items']:
        if pod.get('status', {}).get('phase') in ('Succeeded', 'Failed'):
            continue
        node = pod.get('spec', {}).get('nodeName')
        request, limit = pod_resources(pod, 'requests'), pod_resources(pod, 'limits')
        spec = pod.get('spec', {})
        unbounded = [key for key, resource in DIMENSIONS.items()
                     if quantity(spec.get('resources', {}).get('limits', {}).get(resource, '0')) == 0
                     and any(quantity(c.get('resources', {}).get('limits', {}).get(resource, '0')) == 0
                             for c in spec.get('containers', []) + spec.get('initContainers', []))]
        if node in summaries:
            summaries[node]['requests'] = add(summaries[node]['requests'], request)
            summaries[node]['declaredLimits'] = add(summaries[node]['declaredLimits'], limit)
            summaries[node]['unboundedLimitDimensions'] = sorted(set(summaries[node]['unboundedLimitDimensions'] + unbounded))
        else:
            unplaced = add(unplaced, request)
        workloads.append({'namespace': pod['metadata']['namespace'], 'name': pod['metadata']['name'],
                          'node': node, 'phase': pod.get('status', {}).get('phase'),
                          'requests': request, 'declaredLimits': limit, 'unboundedLimitDimensions': unbounded,
                          'images': [c.get('image') for c in pod.get('spec', {}).get('containers', [])]})
    for node in summaries.values():
        # Guard once against allocatable; approved control-plane limits are separate ceilings.
        node['guardedHeadroom'] = {key: node['allocatable'][key] * 80 // 100 - node['requests'][key]
                                   for key in DIMENSIONS}
    return {'admissionApproved': False, 'nodes': list(summaries.values()),
            'unplacedRequests': unplaced, 'workloads': workloads}


# Runs in memory on the explicitly selected host. No file writes, service calls,
# exec into customer containers, credentials, or reads of customer volume contents.
HOST_INVENTORY = r'''
import hashlib,json,os,pathlib,subprocess
root=pathlib.Path('/opt/webdock-hosting-agent')
config=json.loads(pathlib.Path('/etc/webdock-storage.json').read_text())
journal=json.loads((pathlib.Path(config['root'])/'_pool/state.json').read_text())
st=os.statvfs(config['root'])
volumes=[]
for key,record in journal['volumes'].items():
    volumes.append({k:record.get(k) for k in ['appID','projectID','clusterID','bytes','uuid','state']})
files=['daemon.py','executor.py','observer.py','storage.py','storage_client.py','storage_service.py','storage_router.py','storage_rpc.py']
hashes={name:hashlib.sha256((root/name).read_bytes()).hexdigest() for name in files if (root/name).is_file()}
mounts=json.loads(subprocess.run(['findmnt','--json','--list','-o','TARGET,FSTYPE,SIZE,AVAIL','--bytes'],check=True,capture_output=True,text=True,timeout=10).stdout)
mounts={'filesystems':[m for m in mounts['filesystems'] if m['target']=='/' or m['target']==config['root'] or m['target'].startswith(config['root']+'/')]}
services={name:subprocess.run(['systemctl','is-active',name],capture_output=True,text=True,timeout=10).stdout.strip() for name in ['k3s','spitzli-storage','webdock-hosting-agent']}
print(json.dumps({'storage':{k:config[k] for k in ['node','clusterID','poolID','capacityBytes','reserveBytes']},'journalPoolID':journal['poolID'],'volumes':volumes,'filesystemAvailableBytes':st.f_bavail*st.f_frsize,'agentSourceSHA256':hashes,'services':services,'mounts':mounts}))
'''


def ssh(config, node, command, source=None):
    result = subprocess.run(['ssh', '-F', str(config), '-o', 'BatchMode=yes', '-o',
                             'StrictHostKeyChecking=yes', '-o', 'ConnectTimeout=8', node, command],
                            input=source, capture_output=True, text=True, timeout=60)
    if result.returncode:
        # SSH/server output can contain credentials; keep errors bounded and neutral.
        raise RuntimeError(f'Read-only inventory failed for {node} (exit {result.returncode})')
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ssh-config', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    nodes = ['wd-eu-01', 'wd-eu-02', 'wd-eu-03']
    def kubectl(resource):
        return json.loads(ssh(args.ssh_config, 'wd-eu-02', f'k3s kubectl get {resource} -A -o json'))
    observed = datetime.now(timezone.utc).isoformat()
    # Reduce pod objects on the server: environments and Secret references never
    # cross SSH, even transiently. This source is executed in memory, not installed.
    result = json.loads(ssh(args.ssh_config, 'wd-eu-02', 'python3 - --collect-cluster',
                            Path(__file__).read_text()))
    if sorted(n['name'] for n in result['nodes']) != nodes:
        raise ValueError('Unexpected cluster membership; review the intended target')
    result['observedAt'] = observed
    result['hosts'] = {node: json.loads(ssh(args.ssh_config, node, 'python3 -', HOST_INVENTORY)) for node in nodes}
    for node, host in result['hosts'].items():
        if host['storage']['node'] != node or host['storage']['clusterID'] != '102835722657992704' or host['storage']['poolID'] != host['journalPoolID']:
            raise ValueError('Unexpected storage ownership; refusing baseline')
    result['resourceQuotas'] = [{'namespace': q['metadata']['namespace'], 'name': q['metadata']['name'],
                                'hard': q.get('status', {}).get('hard'), 'used': q.get('status', {}).get('used')}
                               for q in kubectl('resourcequotas')['items']]
    result['networkPolicies'] = [{'namespace': p['metadata']['namespace'], 'name': p['metadata']['name']}
                                 for p in kubectl('networkpolicies')['items']]
    result['admissionPolicies'] = [{'name': p['metadata']['name'], 'spec': p['spec']}
                                   for p in kubectl('validatingadmissionpolicies')['items']]
    result['admissionBindings'] = [{'name': p['metadata']['name'], 'spec': p['spec']}
                                   for p in kubectl('validatingadmissionpolicybindings')['items']]
    result['customResourceDefinitions'] = [p['metadata']['name'] for p in kubectl('customresourcedefinitions')['items']]
    result['nodePorts'] = [{'namespace': s['metadata']['namespace'], 'name': s['metadata']['name'],
                            'port': p['nodePort']} for s in kubectl('services')['items']
                           for p in s['spec'].get('ports', []) if p.get('nodePort')]
    result['completedAt'] = datetime.now(timezone.utc).isoformat()
    # Exclusive creation avoids silently replacing a previous evidence snapshot.
    with args.output.open('x') as output:
        json.dump(result, output, indent=2)
        output.write('\n')
    print(f'Read-only baseline written to {args.output}; admission remains unapproved.')


if __name__ == '__main__':
    if sys.argv[1:] == ['--collect-cluster']:
        def read(resource):
            return json.loads(subprocess.run(['k3s', 'kubectl', 'get', resource, '-A', '-o', 'json'],
                                             check=True, capture_output=True, text=True, timeout=30).stdout)
        print(json.dumps(summarize_cluster(read('nodes'), read('pods'))))
    else:
        main()
