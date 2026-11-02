#!/usr/bin/env python3
"""Run the ignored storage lifecycle test inside one explicitly owned Kind node.
Copies private fixture files; never mounts host storage or touches default contexts.
"""
import argparse
import json
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import uuid

parser = argparse.ArgumentParser()
parser.add_argument('--kubeconfig', required=True)
parser.add_argument('--context', required=True)
parser.add_argument('--image', required=True)
parser.add_argument('--database', action='store_true', help='Run the native database Apply fixture instead of app storage')
parser.add_argument('--domain', action='store_true', help='Run fresh restricted-domain/Core/daemon/CNPG acceptance')
args = parser.parse_args()
os.environ.pop('DOCKER_CONTEXT', None)
if args.domain:
    args.database = True
assert re.fullmatch(r'kind-webdock-rust-[a-z0-9-]+', args.context)
assert re.fullmatch(r'docker.io/library/busybox@sha256:[0-9a-f]{64}', args.image)
config_path = pathlib.Path(args.kubeconfig).resolve(strict=True)
cli = ['kubectl', '--kubeconfig', str(config_path), '--context', args.context]
nodes = json.loads(subprocess.run(cli + ['get', 'nodes', '-o', 'json'], check=True, capture_output=True).stdout)['items']
assert len(nodes) == 1
node = nodes[0]['metadata']['name']
assert node == args.context.removeprefix('kind-') + '-control-plane'
engine = ['docker', '--host', 'unix:///var/run/docker.sock']
inspect = json.loads(subprocess.run(engine + ['inspect', node], check=True, capture_output=True).stdout)[0]
assert inspect['Config']['Labels']['io.x-k8s.kind.cluster'] == args.context.removeprefix('kind-')

def docker(*words, check=True):
    return subprocess.run(engine + list(words), check=check, capture_output=True)

target = 'native_database' if args.domain else ('database_runtime' if args.database else 'legacy')
package = 'webdock-agent-control' if args.domain else 'webdock-agent'
built = subprocess.run(['cargo', 'test', '-p', package, '--test', target, '--no-run', '--message-format=json'], check=True, capture_output=True, text=True)
executables = [row['executable'] for line in built.stdout.splitlines() if (row := json.loads(line)).get('reason') == 'compiler-artifact' and row.get('target', {}).get('name') == target and row.get('executable')]
assert len(executables) == 1
fixture = '/var/local/webdock-rust-tests/allocation-' + uuid.uuid4().hex
docker('exec', node, 'mkdir', '-p', fixture + '/lib')
docker('exec', node, 'chmod', '700', fixture)
config = json.loads(subprocess.run(cli + ['config', 'view', '--raw', '-o', 'json'], check=True, capture_output=True).stdout)
for cluster in config['clusters']:
    cluster['cluster']['server'] = 'https://127.0.0.1:6443'
with tempfile.TemporaryDirectory(prefix='webdock-private-config-') as temporary:
    source = pathlib.Path(temporary) / 'kubeconfig'
    source.write_text(json.dumps(config))
    source.chmod(0o600)
    docker('cp', str(source), node + ':' + fixture + '/kubeconfig')
docker('exec', node, 'chown', '0:0', fixture + '/kubeconfig')
docker('exec', node, 'chmod', '600', fixture + '/kubeconfig')
for source, target in [(executables[0], 'legacy-test'), ('/usr/lib/libc.so.6', 'lib/libc.so.6'), ('/usr/lib/libgcc_s.so.1', 'lib/libgcc_s.so.1'), ('/usr/lib/libm.so.6', 'lib/libm.so.6'), ('/usr/lib64/ld-linux-x86-64.so.2', 'lib/ld-linux-x86-64.so.2')]:
    with tempfile.TemporaryDirectory(prefix='webdock-test-library-') as temporary:
        copy = pathlib.Path(temporary) / pathlib.Path(target).name
        shutil.copyfile(source, copy)
        copy.chmod(0o755)
        docker('cp', str(copy), node + ':' + fixture + '/' + target)
# Kind may lack /dev/loopN entries even with loop-control delegated. Create only
# the currently free device's node inside this disposable container, never detach it.
free = docker('exec', node, '/usr/sbin/losetup', '--find').stdout.decode().strip()
match = re.fullmatch(r'(/dev/loop([0-9]+))(?: \(lost\))?', free)
assert match, free
device, minor = match.groups()
created = docker('exec', node, 'test', '-e', device, check=False).returncode != 0
if created:
    docker('exec', node, 'mknod', '--mode=0600', device, 'b', '7', minor)
extra = []
if args.database:
    assert args.context.startswith('kind-webdock-rust-native-')
    versions = dict(line.split('=', 1) for line in pathlib.Path('infra/postgres/versions.env').read_text().splitlines() if line and not line.startswith('#'))
    ca = subprocess.run(cli + ['get', 'secret', 'webdock-runtime-database-ca', '-n', 'cert-manager', '-o', 'jsonpath={.data.tls\\.crt}'],check=True,capture_output=True).stdout
    import base64
    public_ca = base64.b64decode(ca,validate=True)
    assert b'BEGIN CERTIFICATE' in public_ca and b'PRIVATE KEY' not in public_ca
    with tempfile.TemporaryDirectory(prefix='webdock-public-ca-') as temporary:
        source=pathlib.Path(temporary)/'ca.crt';source.write_bytes(public_ca);source.chmod(0o644);docker('cp',str(source),node+':'+fixture+'/ca.crt')
    docker('exec',node,'chown','0:0',fixture+'/ca.crt')
    api_service=json.loads(subprocess.run(cli+['get','service','kubernetes','-n','default','-o','json'],check=True,capture_output=True).stdout)
    addresses=[api_service['spec']['clusterIP']]+[a['address'] for a in nodes[0]['status']['addresses'] if a['type']=='InternalIP']
    import ipaddress
    cidrs=[address+('/32' if ipaddress.ip_address(address).version==4 else '/128') for address in addresses]
    extra=['-e','WEBDOCK_DATABASE_TEST_NODE_IP='+addresses[1],'-e','WEBDOCK_DISPOSABLE_DATABASE_TEST=yes','-e','WEBDOCK_DATABASE_TEST_CA='+fixture+'/ca.crt','-e','WEBDOCK_DATABASE_TEST_IMAGE='+versions['POSTGRES_IMAGE'],'-e','WEBDOCK_DATABASE_TEST_POOLER_IMAGE='+versions['PGBOUNCER_IMAGE'],'-e','WEBDOCK_DATABASE_TEST_API_CIDRS='+json.dumps(cidrs)]
attached_database = None
if args.domain:
    import hashlib
    import urllib.parse
    metadata = json.loads(pathlib.Path('legacy/control-plane/.api-fixture.json').read_text())
    original = pathlib.Path('legacy/control-plane/.env.test.local').read_bytes()
    assert hashlib.sha256(original).hexdigest() == metadata['envHash']
    assert re.fullmatch(r'webdock-api-fixture-[a-f0-9]{12}', metadata['name'])
    pg = json.loads(docker('inspect', metadata['name']).stdout)[0]
    assert pg['Config']['Labels']['webdock.api-fixture'] == metadata['token']
    env_file = pathlib.Path('.runtime-test/identity-restricted.env').resolve(strict=True)
    assert env_file.stat().st_uid == os.geteuid() and env_file.stat().st_mode & 0o777 == 0o600
    node24 = pathlib.Path.home() / '.nvm/versions/node/v24.21.0/bin/node'
    parse = 'process.stdout.write(JSON.stringify(require("node:util").parseEnv(require("node:fs").readFileSync(process.argv[1], "utf8"))))'
    private = json.loads(subprocess.check_output([str(node24), '-e', parse, str(env_file)]))
    source = json.loads(subprocess.check_output([str(node24), '-e', parse, str(pathlib.Path('legacy/control-plane/.env.test.local').resolve())]))
    owner = urllib.parse.urlsplit(private['WEBDOCK_TEST_MIGRATION_DATABASE_URL'])
    original_url = urllib.parse.urlsplit(source['API_TEST_DATABASE_URL'])
    assert owner.hostname == '127.0.0.1' and owner.path == '/webdock_rls_test' and not owner.query
    assert (owner.hostname, owner.port, owner.username, owner.password) == (original_url.hostname, original_url.port, original_url.username, original_url.password)
    assert pg['NetworkSettings']['Ports']['5432/tcp'] == [{'HostIp': '127.0.0.1', 'HostPort': str(owner.port)}]
    assert 'kind' in inspect['NetworkSettings']['Networks']
    if 'kind' not in pg['NetworkSettings']['Networks']:
        docker('network', 'connect', '--gw-priority', '-1', 'kind', metadata['name'])
        attached_database = metadata['name']
        pg = json.loads(docker('inspect', metadata['name']).stdout)[0]
    address = pg['NetworkSettings']['Networks']['kind']['IPAddress']
    assert ipaddress.ip_address(address).is_private
    docker('cp', '/usr/local/bin/helm', node + ':' + fixture + '/helm')
    docker('cp', str(pathlib.Path('deploy/helm/webdock-plugin').resolve()), node + ':' + fixture + '/chart')
    extra += ['--env-file', str(env_file), '-e', 'WEBDOCK_DATABASE_TEST_PG_TARGET=' + address + ':5432', '-e', 'WEBDOCK_DATABASE_TEST_HELM=' + fixture + '/helm', '-e', 'WEBDOCK_DATABASE_TEST_CHART=' + fixture + '/chart']
try:
    result = subprocess.run(engine + ['exec', *extra, '-e', 'WEBDOCK_DISPOSABLE_NODE_TEST=yes', '-e', 'WEBDOCK_KUBE_TEST_CONFIG=' + fixture + '/kubeconfig', '-e', 'WEBDOCK_KUBE_TEST_CONTEXT=' + args.context, '-e', 'WEBDOCK_KUBE_TEST_IMAGE=' + args.image, node, fixture + '/lib/ld-linux-x86-64.so.2', '--library-path', fixture + '/lib', fixture + '/legacy-test', 'owned_database_domain' if args.domain else ('disposable_database' if args.database else 'disposable_node'), '--ignored', '--nocapture'])
finally:
    if attached_database:
        docker('network', 'disconnect', 'kind', attached_database)


if created:
    loops = json.loads(docker('exec', node, '/usr/sbin/losetup', '--json', '--list', '--output', 'NAME,BACK-FILE', device).stdout)['loopdevices']
    if all(row.get('back-file') is None for row in loops):
        docker('exec', node, 'rm', '--', device)
    else:
        raise RuntimeError('Fixture device is still attached; preserve it for exact-image reconciliation')
docker('exec', node, 'rm', '-r', '--', fixture)
raise SystemExit(result.returncode)
