#!/usr/bin/env python3
"""Read-only k3s observer. Emits allowlisted node metadata, never kubeconfig or annotations."""
import json
import os
import re
import subprocess
import sys
from decimal import Decimal
from datetime import datetime, timezone


def quantity(value, cpu=False):
    match = re.fullmatch(r'(\d+(?:\.\d+)?)([KMGTPE]i|[kKMGTPE]|m|u|n)?', str(value))
    if not match:
        raise ValueError('Unsupported Kubernetes quantity')
    number, unit = match.groups()
    binary = {f'{s}i': 1024 ** i for i, s in enumerate('KMGTPE', 1)}
    decimal = {s: 1000 ** i for i, s in enumerate('kMGTPE', 1)}
    factors = {**binary, **decimal, 'K': 1000, 'm': Decimal('0.001'), 'u': Decimal('0.000001'), 'n': Decimal('0.000000001'), None: 1}
    result = Decimal(number) * factors[unit] * (1000 if cpu else 1)
    if result != result.to_integral_value() or result < 0 or result > 9007199254740991:
        raise ValueError('Quantity is outside the exact supported range')
    return int(result)


def observation(document, sequence=1, generation=1, strict_ready=True):
    nodes = []
    versions = set()
    for item in document['items']:
        status, metadata = item['status'], item['metadata']
        if strict_ready and not any(c['type'] == 'Ready' and c['status'] == 'True' for c in status.get('conditions', [])):
            raise ValueError('A cluster node is not Ready')
        info = status['nodeInfo']
        versions.add(info['kubeletVersion'])
        architecture = info['architecture']
        if architecture not in ('amd64', 'arm64'):
            raise ValueError('Unsupported node architecture')
        nodes.append({'id': metadata['name'], 'role': 'server' if 'node-role.kubernetes.io/control-plane' in metadata.get('labels', {}) else 'agent', 'architecture': architecture, 'cpuMillicores': quantity(status['allocatable']['cpu'], cpu=True), 'memoryBytes': quantity(status['allocatable']['memory'])})
    if not nodes or len(nodes) > 200:
        raise ValueError('Unexpected node count')
    # Advertise no provisionable capacity until scheduling reserves and all capabilities are verified.
    return {'sequence': sequence, 'generation': generation, 'observedAt': datetime.now(timezone.utc).isoformat(timespec='milliseconds').replace('+00:00', 'Z'), 'version': ', '.join(sorted(versions)), 'nodes': nodes,
            'capacity': dict.fromkeys(['apps','cpuMillicores','memoryBytes','volumeBytes','ephemeralBytes','replicasPerApp','concurrentDeployments'], 0),
            'capabilities': {'networkIsolation': False, 'restrictedPods': False, 'storage': False, 'ingress': False, 'environment': True}}


def collect_observation():
    command=['k3s','kubectl']
    if os.environ.get('SPITZLI_KUBECTL_CACHE_DIR'):
        command+=['--cache-dir',os.environ['SPITZLI_KUBECTL_CACHE_DIR']]
    result=subprocess.run(command+['get','nodes','-o','json'],check=True,capture_output=True,text=True,timeout=15)
    observed=observation(json.loads(result.stdout))
    from storage_client import capability
    capacity=capability([node['id'] for node in observed['nodes']])
    if capacity is not None:
        observed['capacity']['volumeBytes']=capacity['capacityBytes']
        observed['capabilities'].update(storage=capacity['ready'],storageVersion=1)
    if capacity is not None and capacity['ready']:
        try:
            from mail_executor import mail_capability
            mail = mail_capability([node['id'] for node in observed['nodes']])
        except ImportError:
            # The observer-only SSH bridge and BYOK installer do not ship managed Mail execution.
            mail = None
        if mail is not None:
            observed['capabilities']['nativeMail'] = mail

    try:
        from databases import database_capability
        databases=database_capability([node['id'] for node in observed['nodes']])
        if databases:observed['capabilities'].update(databases)
    except ImportError:pass
    try:
        import service_bindings
        observed['capabilities']['serviceBindingsVersion']=1
    except ImportError:pass
    import registry
    if registry.capability(command):observed['capabilities']['registryVersion']=1
    return observed


def main():
    try:
        print(json.dumps(collect_observation()))
    except Exception:
        print('Node observation failed; no cluster state was changed.', file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
