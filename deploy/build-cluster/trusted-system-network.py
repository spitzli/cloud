#!/usr/bin/env python3
"""Version 1: offline render or explicitly targeted trusted BUILD network cutover.

Only CoreDNS and metrics-server use host networking. This is outside customer
admission. It preserves the K3s unit, log mount, kubelet budgets and proxy UID guard.
"""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess

ROOT = Path(__file__).resolve().parent
HOST = 'webdock-build-eu-01'
PRIVATE = '10.83.0.1'
PUBLIC = '167.86.75.144'
VERSION = 'v1.35.9+k3s1'
SKIP_FILES = ('coredns.yaml.skip', 'metrics-server/metrics-server-deployment.yaml.skip')
MARKER = Path('/etc/rancher/k3s/trusted-system-network-v1.json')
COREFILE = '''.:53 {
    bind 10.83.0.1
    errors
    health 127.0.0.1:8080
    ready 127.0.0.1:8181
    kubernetes cluster.local in-addr.arpa ip6.arpa {
        pods insecure
        fallthrough in-addr.arpa ip6.arpa
    }
    hosts /etc/coredns/NodeHosts {
        ttl 60
        reload 15s
        fallthrough
    }
    prometheus 127.0.0.1:9153
    cache 30
    loop
    reload
    loadbalance
    forward . /etc/resolv.conf
}
'''
IMAGES = {
    'coredns': 'docker.io/rancher/mirrored-coredns-coredns@sha256:7efd3c635b03efd68c4e8398fc45f0d993d0e9ab016f72c1cefb0fd6d01aa286',
    'metrics-server': 'docker.io/rancher/mirrored-metrics-server@sha256:d9862115e7c7881280d3d75ca26bda8ffc0fc213315979575bf23ce9826205c0',
}


def profile():
    patches = {}
    for name in IMAGES:
        container = {'name': name, 'image': IMAGES[name], 'securityContext': {'allowPrivilegeEscalation': False, 'readOnlyRootFilesystem': True}}
        if name == 'coredns':
            container.update({'livenessProbe': {'httpGet': {'host': '127.0.0.1', 'path': '/health', 'port': 8080}}, 'readinessProbe': {'httpGet': {'host': '127.0.0.1', 'path': '/ready', 'port': 8181}}})
        else:
            container.update({'args': ['--cert-dir=/tmp', '--bind-address=10.83.0.1', '--secure-port=10251', '--kubelet-preferred-address-types=InternalIP', '--kubelet-use-node-status-port', '--metric-resolution=15s'], 'ports': [{'$patch': 'replace'}, {'name': 'https', 'containerPort': 10251, 'protocol': 'TCP'}], 'livenessProbe': {'httpGet': {'host': PRIVATE, 'path': '/livez', 'port': 'https', 'scheme': 'HTTPS'}}, 'readinessProbe': {'httpGet': {'host': PRIVATE, 'path': '/readyz', 'port': 'https', 'scheme': 'HTTPS'}}})
        patches[name] = {'metadata': {'annotations': {'webdock.dev/trusted-network-profile': 'v1'}}, 'spec': {'strategy': {'$patch': 'replace', 'type': 'Recreate'}, 'template': {'metadata': {'annotations': {'webdock.dev/trusted-network-profile': 'v1'}}, 'spec': {'hostNetwork': True, 'dnsPolicy': 'Default' if name == 'coredns' else 'ClusterFirstWithHostNet', 'nodeSelector': {'kubernetes.io/hostname': HOST}, 'containers': [container]}}}}
    return patches


def dns_policy(namespace):
    if namespace not in {'webdock-builds', 'webdock-build-qualification', 'webdock-network-diagnostics'}:
        raise ValueError('unowned DNS policy namespace')
    return {'apiVersion': 'networking.k8s.io/v1', 'kind': 'NetworkPolicy', 'metadata': {'name': 'trusted-host-dns-v1', 'namespace': namespace, 'labels': {'webdock.dev/owner': 'trusted-system-network-v1'}}, 'spec': {'podSelector': {}, 'policyTypes': ['Egress'], 'egress': [{'to': [{'ipBlock': {'cidr': PRIVATE + '/32'}}], 'ports': [{'protocol': 'UDP', 'port': 53}, {'protocol': 'TCP', 'port': 53}]}]}}


def run(args, **kwargs):
    return subprocess.run(args, capture_output=True, text=True, check=True, timeout=330, **kwargs)


def kubectl(*args, **kwargs):
    return run(['/usr/local/bin/k3s', 'kubectl', '--kubeconfig=/etc/rancher/k3s/k3s.yaml', '--context=default', *args], **kwargs)


def get(kind, name, namespace='kube-system'):
    return json.loads(kubectl('get', kind, name, '-n', namespace, '-o', 'json').stdout)


def preflight(idle=False):
    if os.geteuid() != 0 or socket.gethostname() != HOST:
        raise RuntimeError('requires root on the owned BUILD host')
    addresses = {entry['local'] for link in json.loads(run(['ip', '-j', 'address']).stdout) for entry in link.get('addr_info', [])}
    nodes = json.loads(kubectl('get', 'nodes', '-o', 'json').stdout)['items']
    if not {PRIVATE, PUBLIC}.issubset(addresses) or [node['metadata']['name'] for node in nodes] != [HOST] or nodes[0]['status']['nodeInfo']['kubeletVersion'] != VERSION:
        raise RuntimeError('wrong build node/address/version')
    if run(['id', '-u', 'proxy']).stdout.strip() != '13':
        raise RuntimeError('package proxy identity drift')
    run(['nft', 'list', 'table', 'inet', 'webdock_package_proxy'])
    if idle:
        pods = json.loads(kubectl('get', 'pods', '-A', '-o', 'json').stdout)['items']
        if any(pod['metadata']['namespace'] != 'kube-system' and pod.get('status', {}).get('phase') not in {'Succeeded', 'Failed'} for pod in pods):
            raise RuntimeError('non-system Pod is active; drain owned builds first')
    return nodes[0]


def assert_subset(expected, observed, path='profile'):
    if isinstance(expected, dict):
        for key, value in expected.items():
            if key == '$patch': continue
            if key not in observed: raise RuntimeError('missing ' + path + '.' + key)
            assert_subset(value, observed[key], path + '.' + key)
    elif isinstance(expected, list):
        for value in expected:
            if isinstance(value, dict) and '$patch' in value: continue
            if isinstance(value, dict) and 'name' in value:
                matches = [entry for entry in observed if entry.get('name') == value['name']]
                if len(matches) != 1: raise RuntimeError('list identity drift ' + path)
                assert_subset(value, matches[0], path)
            elif value not in observed: raise RuntimeError('list drift ' + path)
    elif expected != observed:
        raise RuntimeError('value drift ' + path)


def verify():
    preflight()
    for name, patch in profile().items():
        assert_subset(patch, get('deployment', name))
        kubectl('rollout', 'status', 'deployment/' + name, '-n', 'kube-system', '--timeout=180s')
    if get('configmap', 'coredns')['data']['Corefile'] != COREFILE:
        raise RuntimeError('CoreDNS listener/config drift')
    for relative in SKIP_FILES:
        if not (Path('/var/lib/rancher/k3s/server/manifests') / relative).is_file():
            raise RuntimeError('missing stock addon skip guard')
    endpoints = json.loads(kubectl('get', 'endpointslices', '-n', 'kube-system', '-l', 'kubernetes.io/service-name=kube-dns', '-o', 'json').stdout)['items']
    ready = [endpoint for item in endpoints for endpoint in item['endpoints'] if endpoint.get('conditions', {}).get('ready')]
    if not ready or any(endpoint['addresses'] != [PRIVATE] for endpoint in ready):
        raise RuntimeError('DNS service endpoint is not the private host')
    rules = run(['nft', 'list', 'table', 'inet', 'webdock_build_host']).stdout
    if 'system_pods' in rules or 'iifname "cni0" counter' not in rules:
        raise RuntimeError('host firewall still admits IP authority or is missing')
    listeners = run(['ss', '-H', '-lntup']).stdout
    private_ports = {53, 3128, 6443, 6444, 8080, 8181, 9153, 10248, 10249, 10250, 10251, 10256, 10257, 10258, 10259}
    for line in listeners.splitlines():
        authority = line.split()[4]
        address, port = authority.rsplit(':', 1)
        if port.isdigit() and int(port) in private_ports and address not in {'127.0.0.1', '127.0.0.53%lo', '127.0.0.54', '[::1]', PRIVATE}:
            raise RuntimeError('non-private sensitive listener: ' + authority)
    metrics = json.loads(kubectl('get', '--raw', '/apis/metrics.k8s.io/v1beta1/nodes').stdout)
    if not metrics.get('items'): raise RuntimeError('metrics API has no node data')
    return {'profile': 'trusted-system-network-v1', 'node': HOST, 'dnsEndpoint': PRIVATE, 'listeners': listeners, 'metricsNodes': len(metrics['items']), 'hostFirewall': rules}


def install():
    preflight(idle=True)
    # Retain rollback inputs for operator recovery; never automatically reopen Pod APIs.
    backup = Path('/etc/rancher/k3s/trusted-network-before-v1.json')
    if not backup.exists():
        backup.write_text(json.dumps({'deployments': {name: get('deployment', name) for name in IMAGES}, 'coredns': get('configmap', 'coredns')}, indent=2))
        backup.chmod(0o600)
    if not MARKER.exists():
        for address, port in [(PRIVATE, 53), (PRIVATE, 10251), ('127.0.0.1', 8080), ('127.0.0.1', 8181), ('127.0.0.1', 9153)]:
            for kind in ([socket.SOCK_STREAM, socket.SOCK_DGRAM] if port == 53 else [socket.SOCK_STREAM]):
                with socket.socket(socket.AF_INET, kind) as listener:
                    listener.bind((address, port))
    source = ROOT / 'host.nft'
    run(['nft', '--check', '-f', str(source)])
    for relative in SKIP_FILES:
        (Path('/var/lib/rancher/k3s/server/manifests') / relative).touch(mode=0o600)
    # Revoke the legacy authority before replacing networking. Local API remains available.
    run(['systemctl', 'disable', '--now', 'webdock-build-system-pods.timer'])
    run(['systemctl', 'stop', 'webdock-build-system-pods.service'])
    run(['install', '-m', '0700', str(ROOT / 'sync-system-pods.py'), '/usr/local/libexec/webdock-build-system-pods'])
    run(['install', '-m', '0600', str(source), '/etc/rancher/k3s/host.nft'])
    run(['nft', '-f', '/etc/rancher/k3s/host.nft'])
    for namespace in ['webdock-builds', 'webdock-build-qualification']:
        kubectl('apply', '-f', '-', input=json.dumps(dns_policy(namespace)))
    kubectl('patch', 'configmap', 'coredns', '-n', 'kube-system', '--type=merge', '-p', json.dumps({'data': {'Corefile': COREFILE}}))
    for name, patch in profile().items():
        kubectl('patch', 'deployment', name, '-n', 'kube-system', '--type=strategic', '-p', json.dumps(patch))
        kubectl('rollout', 'status', 'deployment/' + name, '-n', 'kube-system', '--timeout=180s')
    MARKER.write_text(json.dumps({'profile': 'trusted-system-network-v1', 'images': IMAGES}, indent=2))
    MARKER.chmod(0o600)
    return verify()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--render', action='store_true')
    mode.add_argument('--install', action='store_true')
    mode.add_argument('--verify', action='store_true')
    parser.add_argument('--on-build-host', action='store_true')
    args = parser.parse_args()
    if args.render:
        print(json.dumps({'version': 1, 'deployments': profile(), 'Corefile': COREFILE, 'policies': [dns_policy(ns) for ns in ['webdock-builds', 'webdock-build-qualification']], 'skipFiles': SKIP_FILES}, indent=2))
    else:
        if not args.on_build_host: parser.error('--on-build-host is required for an explicit BUILD connection')
        print(json.dumps(install() if args.install else verify(), indent=2))


if __name__ == '__main__': main()
