#!/usr/bin/env python3
"""Run tenant hostname security tests against a fresh owned local native PostgreSQL fixture."""
from pathlib import Path
import hashlib
import json
import os
import secrets
import signal
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
IMAGE = 'docker.io/library/postgres@sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336'
ROLES = ['webdock_auth_bootstrap', 'webdock_auth_runtime', 'webdock_worker_runtime',
         'webdock_admin_runtime', 'webdock_registry_runtime', 'webdock_runtime',
         'webdock_browser_website', 'webdock_cms_archive_runtime', 'webdock_offline_migrate']
LABEL = 'dev.spitzli.public-hostname-fixture'


def main():
    os.umask(0o077)
    migrator = ROOT / 'target/debug/spitzli-migrate'
    if not migrator.is_file():
        raise SystemExit('Build first: cargo build --locked -p spitzli-control-plane --bin spitzli-migrate -j 2')
    nonce = secrets.token_hex(12)
    directory = ROOT / '.runtime-test' / ('public-hostnames-' + nonce)
    directory.mkdir(mode=0o700, parents=True)
    environment = {key: value for key, value in os.environ.items()
                   if key in ['PATH', 'HOME', 'LANG', 'LC_ALL', 'TMPDIR']}
    password = secrets.token_hex(32)
    context = secrets.token_hex(48)
    auth_secret = secrets.token_hex(48)
    environment['POSTGRES_PASSWORD'] = password
    role_file = directory / 'roles.private.json'
    fixture_file = directory / 'fixture.private.json'
    passwords = {role: secrets.token_hex(32) for role in ROLES}
    role_file.write_text(json.dumps(passwords))
    container = None
    result = None

    def docker(args, **options):
        return subprocess.run(['docker', '--host', 'unix:///var/run/docker.sock', *args],
                              env=environment, capture_output=True, check=True,
                              timeout=options.pop('timeout', 30), **options)

    def interrupted(_signal, _frame):
        raise KeyboardInterrupt

    signal.signal(signal.SIGTERM, interrupted)
    try:
        container = docker(['run', '-d', '--name', 'spitzli-hostnames-' + nonce,
                            '--label', LABEL + '=' + nonce, '--user', '999:999',
                            '--cap-drop', 'ALL', '--security-opt', 'no-new-privileges',
                            '--memory', '1g', '--memory-swap', '1g', '--cpus', '1',
                            '--pids-limit', '256', '--read-only',
                            '--tmpfs', '/var/lib/postgresql:rw,nosuid,nodev,size=512m,uid=999,gid=999',
                            '--tmpfs', '/var/run/postgresql:rw,nosuid,nodev,size=16m,uid=999,gid=999',
                            '--tmpfs', '/tmp:rw,nosuid,nodev,size=32m,uid=999,gid=999',
                            '-p', '127.0.0.1::5432', '-e', 'POSTGRES_PASSWORD',
                            '-e', 'POSTGRES_DB=webdock_rls_test', IMAGE]).stdout.decode().strip()
        (directory / 'custody.json').write_text(json.dumps({'container': container, 'nonce': nonce}))
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            ready = subprocess.run(['docker', '--host', 'unix:///var/run/docker.sock', 'exec',
                                    container, 'pg_isready', '-h', '127.0.0.1', '-U', 'postgres'],
                                   env=environment, capture_output=True, timeout=10)
            if ready.returncode == 0:
                break
            time.sleep(0.25)
        else:
            raise RuntimeError('Owned PostgreSQL fixture did not become ready')
        info = json.loads(docker(['inspect', container]).stdout)[0]
        bindings = info['NetworkSettings']['Ports']['5432/tcp']
        assert len(bindings) == 1 and bindings[0]['HostIp'] == '127.0.0.1'
        port = bindings[0]['HostPort']
        base = '@127.0.0.1:' + port + '/webdock_rls_test'
        owner = 'postgresql://postgres:' + password + base
        with (directory / 'bootstrap.private.log').open('wb') as log:
            subprocess.run([str(migrator), '--target', '127.0.0.1:' + port + '/webdock_rls_test',
                            '--initialize-empty', '--provision-roles-from', str(role_file)],
                           cwd=ROOT, env={**environment, 'SPITZLI_MIGRATION_DATABASE_URL': owner,
                                          'DATABASE_RLS_SECRET': context},
                           stdout=log, stderr=subprocess.STDOUT, check=True, timeout=180)
        urls = {role: 'postgresql://' + role + ':' + value + base for role, value in passwords.items()}
        fixture_file.write_text(json.dumps({'ownerUrl': owner, 'roles': urls,
                                           'contextSecret': context, 'authSecret': auth_secret}))
        env = {**environment, 'NODE_ENV': 'test', 'SPITZLI_PUBLIC_HOST_FIXTURE': str(fixture_file),
               'DATABASE_URL': urls['webdock_auth_runtime'], 'AUTH_DATABASE_URL': urls['webdock_auth_bootstrap'],
               'DATABASE_WORKER_URL': urls['webdock_worker_runtime'], 'DATABASE_RLS_SECRET': context,
               'DATABASE_CONTEXT_SECRET': context, 'BETTER_AUTH_SECRET': auth_secret}
        with (directory / 'security-tests.private.log').open('wb') as log:
            result = subprocess.run(['bun', '--no-env-file', 'test', 'crates/hosting/tests/public-hostnames.test.ts'],
                                    cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=90)
        if result.returncode:
            raise RuntimeError('Security fixture test failed; inspect ' + str(directory / 'security-tests.private.log'))
        print(json.dumps({'passed': True, 'freshNativeMigration': True,
                          'tenantSecurityAndRaces': True, 'productionDataUsed': False,
                          'migrationSha256': hashlib.sha256((ROOT / 'crates/persistence/migrations/0066_platform_hostnames.sql').read_bytes()).hexdigest()}))
    finally:
        if container:
            info = json.loads(docker(['inspect', container]).stdout)[0]
            assert info['Config']['Labels'].get(LABEL) == nonce, 'Fixture ownership changed; refuse cleanup'
            docker(['rm', '-f', container])
            (directory / 'cleanup.json').write_text(json.dumps({'container': container, 'removed': True}))
        role_file.unlink(missing_ok=True)
        fixture_file.unlink(missing_ok=True)


if __name__ == '__main__':
    main()
