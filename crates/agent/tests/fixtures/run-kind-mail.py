#!/usr/bin/env python3
"""Run one quota-backed Mail fixture inside the explicitly owned disposable Kind node."""
import argparse
import json
import os
import pathlib
import re
import shutil
import subprocess
import tempfile
import urllib.parse
import uuid

parser = argparse.ArgumentParser()
parser.add_argument('--kubeconfig', required=True)
parser.add_argument('--context', required=True)
parser.add_argument('--image', required=True)
parser.add_argument('--probe-image', required=True)
parser.add_argument('--case', choices=['lifecycle', 'configuration'], default='lifecycle')
args = parser.parse_args()
os.environ.pop('DOCKER_CONTEXT', None)
assert args.context == 'kind-webdock-rust-native-3c8064'
for image in [args.image, args.probe_image]:
    assert re.fullmatch(r'[A-Za-z0-9._:/-]+@sha256:[a-f0-9]{64}', image)
engine = ['docker', '--host', 'unix:///var/run/docker.sock']
path = pathlib.Path(args.kubeconfig).resolve(strict=True)
assert path.stat().st_mode & 0o077 == 0
cli = ['kubectl', '--kubeconfig', str(path), '--context', args.context]
node = args.context.removeprefix('kind-') + '-control-plane'
node_record = json.loads(subprocess.check_output(engine + ['inspect', node]))[0]
assert node_record['Id'].startswith('6c1603f267dd')
node_id = node_record['Id']
config = json.loads(subprocess.check_output(cli + ['config', 'view', '--raw', '-o', 'json']))
context = next(row['context'] for row in config['contexts'] if row['name'] == args.context)
cluster = next(row['cluster'] for row in config['clusters'] if row['name'] == context['cluster'])
identity = next(row['user'] for row in config['users'] if row['name'] == context['user'])
assert cluster.get('certificate-authority-data') and not cluster.get('insecure-skip-tls-verify') and not identity.get('exec')
origin = urllib.parse.urlsplit(cluster['server'])
assert origin.scheme == 'https' and origin.hostname in ['127.0.0.1', 'localhost']
assert any(binding['HostPort'] == str(origin.port) for binding in node_record['NetworkSettings']['Ports']['6443/tcp'])
nodes = json.loads(subprocess.check_output(cli + ['get', 'nodes', '-o', 'json']))['items']
assert len(nodes) == 1 and nodes[0]['metadata']['name'] == node
node_uid = nodes[0]['metadata']['uid']

def verify():
    current = json.loads(subprocess.check_output(engine + ['inspect', node]))[0]
    assert current['Id'] == node_id and current['State']['Running']
    assert current['Config']['Labels']['io.x-k8s.kind.cluster'] == args.context.removeprefix('kind-')
    current = json.loads(subprocess.check_output(cli + ['get', 'node', node, '-o', 'json']))
    assert current['metadata']['uid'] == node_uid
    assert any(row['type'] == 'Ready' and row['status'] == 'True' for row in current['status']['conditions'])

def execute(*words, check=True):
    verify()
    return subprocess.run(engine + ['exec', node, *words], check=check, capture_output=True, timeout=180)

def copy(source, target):
    verify()
    subprocess.run(engine + ['cp', str(source), node + ':' + target], check=True, capture_output=True)

verify()
daemonsets = json.loads(subprocess.check_output(cli + ['get', 'daemonsets', '-A', '-o', 'json']))['items']
calico = next(row for row in daemonsets if row['metadata']['name'] == 'calico-node')
assert calico['status']['numberReady'] == calico['status']['desiredNumberScheduled'] == 1
assert int(execute('df', '-B1', '--output=avail', '/var/local').stdout.splitlines()[1]) >= 12 * 1024 ** 3
fixture = '/var/local/webdock-rust-tests/mail-' + uuid.uuid4().hex
execute('mkdir', '--mode=0700', fixture)
execute('mkdir', '--mode=0700', fixture + '/lib')
completed = False
started = False
created_device = None
try:
    for image in [args.image, args.probe_image]:
        cached = execute('crictl', 'inspecti', image, check=False)
        if cached.returncode:
            metadata = json.loads(subprocess.check_output(engine + ['image', 'inspect', image]))[0]
            assert image in metadata['RepoDigests']
            with tempfile.TemporaryDirectory(prefix='webdock-mail-reviewed-image-') as temporary:
                archive = pathlib.Path(temporary) / 'image.tar'
                subprocess.run(engine + ['image', 'save', '--output', str(archive), image], check=True, capture_output=True)
                copy(archive, fixture + '/image.tar')
                execute('ctr', '--namespace', 'k8s.io', 'images', 'import', '--base-name', image.split('@', 1)[0], '--digests', fixture + '/image.tar')
                execute('rm', '--', fixture + '/image.tar')
            # Saving a digest-only Docker reference may import an unnamed OCI index.
            # Add the reviewed name only after checking the descriptor's exact digest.
            digest = image.split('@', 1)[1]
            descriptors = [line.split() for line in execute('ctr', '--namespace', 'k8s.io', 'images', 'ls').stdout.decode().splitlines()[1:]]
            sources = [row[0] for row in descriptors if len(row) >= 3 and row[2] == digest]
            assert sources, 'Imported descriptor does not match the reviewed immutable image'
            if image not in sources:
                execute('ctr', '--namespace', 'k8s.io', 'images', 'tag', sources[0], image)
            cached = execute('crictl', 'inspecti', image)
        assert image in json.loads(cached.stdout)['status']['repoDigests']
    built = subprocess.run(['cargo', 'test', '-p', 'webdock-agent', '--test', 'mail_kubernetes_owned', '--no-run', '--locked', '--message-format=json'], check=True, capture_output=True, text=True)
    executables = [row['executable'] for line in built.stdout.splitlines() if (row := json.loads(line)).get('reason') == 'compiler-artifact' and row.get('target', {}).get('name') == 'mail_kubernetes_owned' and row.get('executable')]
    assert len(executables) == 1
    selected = {'apiVersion': 'v1', 'kind': 'Config', 'clusters': [{'name': context['cluster'], 'cluster': {**cluster, 'server': 'https://127.0.0.1:6443'}}], 'users': [{'name': context['user'], 'user': identity}], 'contexts': [{'name': args.context, 'context': context}], 'current-context': args.context}
    with tempfile.TemporaryDirectory(prefix='webdock-mail-private-config-') as temporary:
        source = pathlib.Path(temporary) / 'kubeconfig'
        source.write_text(json.dumps(selected)); source.chmod(0o600)
        copy(source, fixture + '/kubeconfig')
    execute('chown', '0:0', fixture + '/kubeconfig')
    execute('chmod', '600', fixture + '/kubeconfig')
    for source, target in [(executables[0], 'mail-test'), ('/usr/lib/libc.so.6', 'lib/libc.so.6'), ('/usr/lib/libgcc_s.so.1', 'lib/libgcc_s.so.1'), ('/usr/lib/libm.so.6', 'lib/libm.so.6'), ('/usr/lib64/ld-linux-x86-64.so.2', 'lib/ld-linux-x86-64.so.2')]:
        with tempfile.TemporaryDirectory(prefix='webdock-mail-test-library-') as temporary:
            local = pathlib.Path(temporary) / pathlib.Path(target).name
            shutil.copyfile(source, local); local.chmod(0o755)
            copy(local, fixture + '/' + target)
    free = execute('/usr/sbin/losetup', '--find').stdout.decode().strip()
    match = re.fullmatch(r'/dev/loop([0-9]+)', free)
    assert match
    if execute('test', '-e', free, check=False).returncode:
        execute('mknod', '--mode=0600', free, 'b', '7', match.group(1)); created_device = free
    assert int(execute('df', '-B1', '--output=avail', fixture).stdout.splitlines()[1]) >= 12 * 1024 ** 3
    verify()
    started = True
    result = subprocess.run(engine + ['exec', '-e', 'WEBDOCK_MAIL_KIND_TEST=yes', '-e', 'WEBDOCK_MAIL_KIND_CASE=' + args.case, '-e', 'WEBDOCK_MAIL_TEST_ROOT=' + fixture, '-e', 'WEBDOCK_KUBE_TEST_CONFIG=' + fixture + '/kubeconfig', '-e', 'WEBDOCK_KUBE_TEST_CONTEXT=' + args.context, '-e', 'WEBDOCK_MAIL_RUNTIME_IMAGE=' + args.image, '-e', 'WEBDOCK_MAIL_PROBE_IMAGE=' + args.probe_image, node, fixture + '/lib/ld-linux-x86-64.so.2', '--library-path', fixture + '/lib', fixture + '/mail-test', 'owned_mail_kubernetes_' + args.case, '--exact', '--ignored', '--nocapture'], check=False)
    # The test writes this marker only after guarded namespace/PV deletion and quota-backed storage purge.
    completed = execute('test', '-f', fixture + '/cleanup-complete', check=False).returncode == 0
    if completed:
        copy_target = pathlib.Path('.runtime-test/runtime-native/mail-configuration-runtime-evidence.json' if args.case == 'configuration' else '.runtime-test/runtime-native/mail-runtime-evidence.json')
        verify(); subprocess.run(engine + ['cp', node + ':' + fixture + '/evidence.json', str(copy_target)], check=True, capture_output=True)
    assert completed, 'Mail fixture cleanup was not proven; preserve the exact fixture and do not allocate another volume'
finally:
    if completed or not started:
        if created_device:
            loops = json.loads(execute('/usr/sbin/losetup', '--json', '--list', '--output', 'NAME,BACK-FILE', created_device).stdout)['loopdevices']
            assert all(row.get('back-file') is None for row in loops)
            execute('rm', '--', created_device)
        execute('rm', '-r', '--', fixture)
    else:
        print('Preserved unverified fixture:', fixture)
if completed:
    print('Owned Mail fixture cleaned; evidence:', copy_target)
raise SystemExit(result.returncode)
