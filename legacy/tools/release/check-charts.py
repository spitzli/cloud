#!/usr/bin/env python3
"""Render candidates and reject unsafe schema inputs; never contacts a cluster."""
import json
import subprocess
import tempfile
from pathlib import Path


def render(chart, values, succeeds=True):
    with tempfile.NamedTemporaryFile(mode='w', suffix='.json') as f:
        json.dump(values, f); f.flush()
        p = subprocess.run(['helm', 'template', 'test', chart, '-f', f.name], capture_output=True, text=True)
        assert (p.returncode == 0) == succeeds, p.stderr
        return p.stdout

image = 'example.invalid/webdock@sha256:' + 'a' * 64
platform = {'components': {name: {'image': image, 'existingSecret': name + '-config'} for name in ['console', 'auth', 'api', 'worker']}}
chart = 'deploy/helm/webdock'
result = render(chart, platform)
assert result.count('kind: Deployment') == 3
with_worker = {**platform, 'components': {**platform['components'], 'worker': {**platform['components']['worker'], 'enabled': True}}}
assert render(chart, with_worker).count('kind: Deployment') == 4
assert result.count('kind: Service\n') == 3
assert 'kind: ClusterRole' not in result
assert 'kind: Secret' not in result
assert 'kind: Job' not in result
assert 'automountServiceAccountToken: false' in result
render(chart, {}, False)
render(chart, {**platform, 'externalDatabase': False}, False)
render(chart, {**platform, 'migration': {'enabled': True}}, False)
result = render(chart, {**platform, 'migration': {'enabled': True, 'image': image, 'existingSecret': 'offline-migrator', 'target': 'db.example:5432/webdock'}})
assert 'kind: Job' in result and 'helm.sh/hook' not in result
# Strict module config: runtime and Helm accept the same exclusive state forms.
for modules in [
    {'configVersion': 1, 'modules': {'mail': {'state': 'draining'}}},
    {'configVersion': 1, 'modules': {'hosting': {'enabled': True}, 'databases': {'state': 'enabled'}}},
]:
    assert 'WEBDOCK_MODULES' in render(chart, {**platform, 'modules': modules})
for modules in [
    'string-config-is-no-longer-supported',
    {'configVersion': 2, 'modules': {}},
    {'configVersion': 1, 'modules': {'unknown': {'enabled': True}}},
    {'configVersion': 1, 'modules': {'mail': {'enabled': True, 'state': 'enabled'}}},
    {'configVersion': 1, 'modules': {'mail': {'state': 'unknown'}}},
    {'configVersion': 1, 'modules': {'mail': {'enabled': 'yes'}}},
    {'configVersion': 1, 'modules': {'mail': {'enabled': False, 'secret': 'no'}}},
]:
    render(chart, {**platform, 'modules': modules}, False)
agent = 'deploy/helm/webdock-agent'
assert 'kind: Deployment' not in render(agent, {})
values = {'enabled': True, 'image': image, 'existingSecret': 'enrollment', 'existingClaim': 'agent-state', 'namespaces': ['customer-app']}
result = render(agent, values)
assert 'kind: Role\n' in result and 'cluster-admin' not in result and 'hostPath' not in result
render(agent, {**values, 'namespaces': ['kube-system']}, False)
render(agent, {**values, 'image': 'image:latest'}, False)
print('Platform and agent render/schema assertions passed')

# Optional gateway stays separate and never creates database servers or credentials.
database_chart='deploy/helm/webdock-databases'
assert 'kind: Deployment' not in render(database_chart,{})
database_values={'enabled':True,'image':image,'existingSecret':'gateway-config'}
gateway=render(database_chart,database_values)
assert gateway.count('kind: Deployment')==1 and 'kind: Secret' not in gateway and 'kind: PersistentVolume' not in gateway
assert 'runAsUser: 65532' in gateway and 'automountServiceAccountToken: false' in gateway
assert 'WEBDOCK_DATABASE_MANAGED_RUNTIMES' in gateway
render(database_chart,{**database_values,'runtimeCapacity':65},False)
render(database_chart,{**database_values,'tabularisCommit':'0'*40},False)
render(database_chart,{**database_values,'image':'gateway:latest'},False)
pin=json.loads(Path('vendor/tabularis-web-ui.json').read_text())
assert json.loads(Path(database_chart+'/values.schema.json').read_text())['properties']['tabularisCommit']['const']==pin['sourceCommit']
print('Optional database gateway chart assertions passed')

private_ca=render(database_chart,{**database_values,'runtimeCAConfigMap':'database-runtime-ca'})
assert 'NODE_EXTRA_CA_CERTS' in private_ca and 'mountPath: /runtime-trust' in private_ca
render(database_chart,{**database_values,'existingSecret':'bad\nname'},False)
