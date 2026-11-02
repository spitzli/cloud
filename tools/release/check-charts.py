#!/usr/bin/env python3
"""Render the native Core chart with disposable values. Never contacts a cluster."""
import subprocess
import yaml

chart = 'deploy/helm/webdock'
command = ['helm', 'template', 'fixture', chart, '--set-string', 'apiHost=api.example.test']
missing = subprocess.run(command, capture_output=True, text=True)
assert missing.returncode != 0, 'No implicit image or secret may be deployed'
image = 'example.invalid/core@sha256:' + 'a' * 64
for invalid in ['example.invalid/core:latest', 'example.invalid/core', '']:
    rejected = subprocess.run(command + ['--set-string', f'image={invalid}', '--set-string', 'existingSecret=fixture-runtime'], capture_output=True, text=True)
    assert rejected.returncode != 0, 'Core requires a reviewed immutable image'
rendered = subprocess.check_output(command + ['--set-string', f'image={image}', '--set-string', 'existingSecret=fixture-runtime'], text=True)
objects = list(yaml.safe_load_all(rendered))
assert {o['kind'] for o in objects} == {'Deployment', 'Service'}
deployment = next(o for o in objects if o['kind'] == 'Deployment')
assert deployment['spec']['strategy']['type'] == 'Recreate'
pod = deployment['spec']['template']['spec']
assert pod['automountServiceAccountToken'] is False
assert pod['securityContext']['runAsNonRoot'] is True
assert pod['securityContext']['runAsUser'] == 65532
assert len(pod['containers']) == 1
core = pod['containers'][0]
assert core['image'] == image
assert core['envFrom'] == [{'secretRef': {'name': 'fixture-runtime'}}]
assert core['securityContext']['readOnlyRootFilesystem'] is True
assert core['securityContext']['allowPrivilegeEscalation'] is False
assert core['securityContext']['capabilities']['drop'] == ['ALL']
for probe, path in [('readinessProbe', '/readyz'), ('livenessProbe', '/healthz'), ('startupProbe', '/healthz')]:
    assert core[probe]['httpGet'] == {'path': path, 'port': 'http', 'httpHeaders': [{'name': 'Host', 'value': 'api.example.test'}]}
assert core['ports'] == [{'name': 'http', 'containerPort': 3130}]
assert all(o['kind'] != 'Job' for o in objects), 'Schema changes require explicit offline execution'
print('Native chart passed: one Core, immutable image, private environment, probes, restricted runtime, no automatic migrations.')
