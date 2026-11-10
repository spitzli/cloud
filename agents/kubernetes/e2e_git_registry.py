#!/usr/bin/env python3
"""Opt-in real registry/agent smoke test; only the disposable kind-webdock-git-e2e context."""
import argparse
import base64
import copy
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import subprocess
import sys
from urllib.parse import urlparse


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--workspace', type=Path, required=True)
    parser.add_argument('--image', type=Path, required=True, help='image.tar produced by the real VM test')
    args = parser.parse_args()
    workspace = args.workspace.resolve()
    kubeconfig = workspace / 'kubeconfig'
    kubectl = ['/usr/bin/kubectl', '--kubeconfig', str(kubeconfig)]
    config = json.loads(subprocess.check_output([*kubectl, 'config', 'view', '-o', 'json']))
    if config['current-context'] != 'kind-webdock-git-e2e' or any(urlparse(c['cluster']['server']).hostname != '127.0.0.1' for c in config['clusters']):
        raise SystemExit('Refusing non-disposable or non-loopback Kubernetes target')
    os.environ['SPITZLI_E2E_KUBECONFIG'] = str(kubeconfig)
    os.environ['SPITZLI_REGISTRY_CONFIG'] = str(workspace / 'registry-policy.json')
    os.environ['PATH'] = str(workspace.parent / 'bin') + ':' + os.environ['PATH']
    os.environ['TMPDIR'] = str(workspace)
    import tempfile
    tempfile.tempdir = str(workspace)
    sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'build-worker'))
    import publisher
    import worker
    import executor
    import registry

    image = args.image.resolve()
    if image.name != 'image.tar' or not image.is_file():
        raise SystemExit('An existing image.tar from the VM is required')
    certs = workspace / 'certs'
    certs.mkdir(exist_ok=True)
    (certs / 'ca.crt').write_bytes((workspace / 'tls.crt').read_bytes())

    def real_skopeo(command, **options):
        if command[:2] != ['/usr/bin/skopeo', 'copy']:
            raise AssertionError('Unexpected publisher command')
        digest_file = Path(command[command.index('--digestfile') + 1])
        auth_file = Path(command[command.index('--authfile') + 1])
        mounts = [(image.parent, True), (digest_file.parent, False), (auth_file, True), (certs, True)]
        launch = ['docker', 'run', '--rm', '--network', 'kind']
        for path, readonly in mounts:
            launch += ['--mount', f'type=bind,source={path},target={path}' + (',readonly' if readonly else '')]
        launch += ['quay.io/skopeo/stable:v1.20.0', 'copy', '--dest-cert-dir', str(certs), *command[2:]]
        return worker.run_bounded(launch, **options)

    repository = 'webdock-git-e2e-registry:5000/customers/2026101000/projects/2026101001'
    published = publisher.publish_oci(image.parent, repository, workspace / 'publish.auth', '2026101010', runner=real_skopeo)
    packet = {
        'appID': '2026101002', 'projectID': '2026101001', 'clusterID': '2026101003',
        'namespace': 'wd-2026101001', 'operationID': '2026101010', 'generation': 1,
        'leaseUntil': (datetime.now(timezone.utc) + timedelta(minutes=10)).isoformat(),
        'revision': 1, 'action': 'apply', 'expectedUID': None,
        'quota': {'apps': 1, 'cpuMillicores': 500, 'memoryBytes': 268435456, 'ephemeralBytes': 268435456},
        'spec': {'template': 'custom', 'image': published['image'], 'args': [], 'port': 8080, 'healthPath': '/',
                 'replicas': 1, 'cpuMillicores': 100, 'memoryBytes': 67108864, 'ephemeralBytes': 67108864},
    }
    packet['finalQuota'] = packet['quota']
    credentials = json.loads((workspace / 'credentials.json').read_text())
    dockerconfig = {'auths': {'webdock-git-e2e-registry:5000': {'auth': base64.b64encode(('puller:' + credentials['puller']).encode()).decode()}}}
    for namespace in [packet['namespace'], 'wdv-' + packet['projectID']]:
        executor.prepare_namespace(packet, namespace)
        secret = {'apiVersion': 'v1', 'kind': 'Secret', 'metadata': {'name': 'git-e2e-pull', 'namespace': namespace},
                  'type': 'kubernetes.io/dockerconfigjson', 'data': {'.dockerconfigjson': base64.b64encode(json.dumps(dockerconfig).encode()).decode()}}
        subprocess.run([*kubectl, 'apply', '-f', '-'], input=json.dumps(secret), text=True, check=True, capture_output=True)
    assert registry.capability(kubectl), 'Real registry capability not observed'
    ready = executor.execute(packet)
    assert ready['ready'] and ready['replicas'] == 1
    body = executor.kubectl('get', '--raw', '/api/v1/namespaces/' + packet['namespace'] + '/services/app-' + packet['appID'] + ':8080/proxy/')
    assert body.strip() == 'webdock-real-kvm-e2e', 'Unexpected real application response'
    deployment = executor.get('deployment', 'app-' + packet['appID'], packet['namespace'])
    pod_spec = deployment['spec']['template']['spec']
    assert pod_spec['containers'][0]['imagePullPolicy'] == 'IfNotPresent'
    assert pod_spec['imagePullSecrets'] == [{'name': 'git-e2e-pull'}]
    assert pod_spec['securityContext']['runAsNonRoot']
    assert pod_spec['containers'][0]['securityContext']['readOnlyRootFilesystem']
    logs = executor.execute({**packet, 'action': 'logs', 'expectedUID': ready['uid']})
    unhealthy = copy.deepcopy(packet)
    unhealthy.update(revision=2, operationID='2026101011', expectedUID=ready['uid'])
    unhealthy['spec']['healthPath'] = '/deliberately-missing-healthcheck'
    try:
        executor.execute(unhealthy)
        raise AssertionError('Failing healthcheck incorrectly became ready')
    except executor.ExecutionError as error:
        if str(error) != 'rollout_timeout':
            raise
    rollback = copy.deepcopy(packet)
    rollback.update(revision=3, operationID='2026101012', expectedUID=ready['uid'])
    restored = executor.execute(rollback)
    assert restored['ready'] and restored['revision'] == 3
    evidence = {'realVMImage': str(image), 'published': published, 'ready': ready, 'logs': logs,
                'applicationResponse': body.strip(), 'failingHealthcheck': 'rollout_timeout', 'rollback': restored,
                'runtime': 'disposable local kind, not production k3s; NetworkPolicy enforcement not asserted'}
    (workspace / 'agent-evidence.json').write_text(json.dumps(evidence, indent=2))
    print(json.dumps({'registryPush': True, 'immutablePull': True, 'readiness': True, 'logs': True,
                      'failingHealthcheck': True, 'rollback': True, 'evidence': str(workspace / 'agent-evidence.json')}))


if __name__ == '__main__':
    main()
