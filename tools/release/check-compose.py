#!/usr/bin/env python3
"""Render native Compose with empty disposable env files; never starts a container."""
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

with tempfile.TemporaryDirectory() as directory:
    path = Path(directory)
    shutil.copy('deploy/compose/compose.yaml', path / 'compose.yaml')
    (path / 'core.env').touch()
    env = {k: v for k, v in os.environ.items() if not k.startswith('WEBDOCK_')}
    env['WEBDOCK_CORE_IMAGE'] = 'example.invalid/webdock@sha256:' + 'a' * 64
    command = ['docker', 'compose', '-f', str(path / 'compose.yaml')]
    services = json.loads(subprocess.check_output(command + ['config', '--format', 'json'], env=env, text=True))['services']
    assert set(services) == {'core'}
    core = services['core']
    assert core['read_only'] and core['user'] == '65532:65532'
    assert core['cap_drop'] == ['ALL']
    assert core['security_opt'] == ['no-new-privileges:true']
    assert core['ports'][0]['host_ip'] == '127.0.0.1'
    assert core['ports'][0]['target'] == 3130
    assert core['tmpfs'] == ['/tmp:rw,nosuid,size=64m,mode=1777']

    shutil.copy('deploy/compose/compose.migration.yaml', path / 'migration.yaml')
    (path / 'migrator.env').touch()
    env['WEBDOCK_MIGRATE_IMAGE'] = 'example.invalid/migrate@sha256:' + 'b' * 64
    env['WEBDOCK_MIGRATION_TARGET'] = 'fixture.invalid:5432/webdock'
    services = json.loads(subprocess.check_output(['docker', 'compose', '-f', str(path / 'migration.yaml'), 'config', '--format', 'json'], env=env, text=True))['services']
    assert set(services) == {'migrate'}
    migrate = services['migrate']
    assert migrate['command'] == ['--target', 'fixture.invalid:5432/webdock']
    assert migrate['restart'] == 'no' and not migrate.get('ports')
    assert migrate['read_only'] and migrate['cap_drop'] == ['ALL']
    assert migrate['user'] == '65532:65532'
    print('Native Compose passed: Core only, restricted runtime, explicit offline migration.')
