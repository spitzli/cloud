#!/usr/bin/env python3
"""Validate Compose with disposable empty env files; never starts a service."""
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

with tempfile.TemporaryDirectory() as directory:
    path = Path(directory)
    shutil.copy('deploy/compose/compose.yaml', path / 'compose.yaml')
    for name in ['api', 'worker', 'console', 'auth', 'migrator']:
        (path / f'{name}.env').touch()
    env = os.environ.copy()
    for name in ['API', 'WORKER', 'CONSOLE', 'AUTH', 'MIGRATE']:
        env[f'WEBDOCK_{name}_IMAGE'] = 'example.invalid/webdock@sha256:' + 'a' * 64
    env['WEBDOCK_MODULES'] = '{"configVersion":1,"modules":{"mail":{"enabled":false}}}'
    env['WEBDOCK_MIGRATION_TARGET'] = 'db.example:5432/webdock'
    output = subprocess.check_output(['docker', 'compose', '-f', str(path / 'compose.yaml'), '--profile', 'migration', '--profile', 'workers', 'config', '--format', 'json'], env=env, text=True)
    services = json.loads(output)['services']
    minimal = subprocess.check_output(['docker', 'compose', '-f', str(path / 'compose.yaml'), 'config', '--format', 'json'], env=env, text=True)
    assert set(json.loads(minimal)['services']) == {'console', 'auth', 'api'}
    assert set(services) == {'console', 'auth', 'api', 'worker', 'migrate', 'jobs-redis'}
    for name in ['console', 'auth', 'api', 'worker', 'migrate']:
        service = services[name]
        assert service['read_only'] and service['cap_drop'] == ['ALL']
        assert service['tmpfs'][0] == '/tmp:rw,nosuid,size=64m,mode=1777'
        assert all(port['host_ip'] == '127.0.0.1' for port in service.get('ports', []))
    assert services['api']['environment']['WEBDOCK_MODULES'] == services['worker']['environment']['WEBDOCK_MODULES']
    assert services['migrate']['restart'] == 'no'
    redis = services['jobs-redis']
    assert redis['profiles'] == ['workers'] and not redis.get('ports')
    assert redis['command'] == ['redis-server', '--appendonly', 'yes', '--maxmemory-policy', 'noeviction']
    assert redis['volumes'][0]['target'] == '/data'
    assert services['worker']['depends_on']['jobs-redis']['condition'] == 'service_healthy'
    print('Compose configuration and runtime restrictions passed')

with tempfile.TemporaryDirectory() as directory:
    path = Path(directory)
    shutil.copy('deploy/compose/compose.core.yaml', path / 'compose.yaml')
    for name in ['api', 'console', 'auth', 'migrator']:
        (path / f'{name}.env').touch()
    env = {key: value for key, value in os.environ.items() if not key.startswith('WEBDOCK_')}
    for name in ['API_CORE', 'CONSOLE_CORE', 'AUTH', 'MIGRATE']:
        env[f'WEBDOCK_{name}_IMAGE'] = 'example.invalid/webdock@sha256:' + 'a' * 64
    env['WEBDOCK_MIGRATION_TARGET'] = 'db.example:5432/webdock'
    command = ['docker', 'compose', '-f', str(path / 'compose.yaml')]
    minimal = json.loads(subprocess.check_output(command + ['config', '--format', 'json'], env=env, text=True))
    assert set(minimal['services']) == {'console', 'auth', 'api'}
    services = json.loads(subprocess.check_output(command + ['--profile', 'migration', 'config', '--format', 'json'], env=env, text=True))['services']
    assert set(services) == {'console', 'auth', 'api', 'migrate'}
    for service in services.values():
        assert service['read_only'] and service['cap_drop'] == ['ALL']
        assert service['security_opt'] == ['no-new-privileges:true']
        assert service['tmpfs'][0] == '/tmp:rw,nosuid,size=64m,mode=1777'
        assert all(port['host_ip'] == '127.0.0.1' for port in service.get('ports', []))
    assert '/app/.core-build/console/.next/cache:rw,nosuid,size=128m,mode=1777' in services['console']['tmpfs']
    assert services['api']['environment']['WEBDOCK_MODULES'] == '{"configVersion":1,"modules":{}}'
    assert services['api']['environment']['WEBDOCK_API_DOMAINS'] == 'auth,portal,registry'
    assert services['migrate']['restart'] == 'no'
    print('Core Compose requires no worker images and preserves runtime restrictions')

with tempfile.TemporaryDirectory() as directory:
    path=Path(directory)
    shutil.copy('deploy/compose/compose.databases.yaml',path/'compose.yaml')
    (path/'database-gateway.env').touch()
    env={**os.environ,'WEBDOCK_DATABASE_GATEWAY_IMAGE':'example.invalid/gateway@sha256:'+'a'*64,'TABULARIS_EXPECTED_COMMIT':json.loads(Path('vendor/tabularis-web-ui.json').read_text())['sourceCommit']}
    rendered=json.loads(subprocess.check_output(['docker','compose','-f',str(path/'compose.yaml'),'--profile','databases','config','--format','json'],env=env,text=True))
    gateway=rendered['services']['database-gateway']
    assert gateway['user']=='65532:65532' and gateway['read_only'] and gateway['cap_drop']==['ALL']
    assert gateway['ports'][0]['host_ip']=='127.0.0.1'
    assert 'databases' in gateway['profiles']
    print('Optional database gateway Compose assertions passed')
