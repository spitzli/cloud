#!/usr/bin/env python3
"""Local-only Barman provenance fixture. Standard library + Docker, Git, GPG, OpenSSL.

No registries, clusters, services or user credentials are configured by this tool.
The generated signer is a LOCAL FIXTURE, never a production trust authority.
"""
import argparse
import datetime
import gzip
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tarfile
import urllib.request
import uuid

HERE = Path(__file__).resolve().parent
COMMIT = '6216b7dd552f389952d3de77712d9522a79d9548'
REPOSITORY = 'https://github.com/cloudnative-pg/plugin-barman-cloud'
SOURCE_SIGNER = '968479A1AFF927E37D1A566BB5690EEEBB952194'
GO = 'docker.io/library/golang@sha256:23fdfd3a6abc97c81e32a724cdd1cf541c06c416eb04d717815f4ed7c75623d0'
DEBIAN = 'docker.io/library/debian@sha256:4876fdea29cac1a33a3195f1eb1952feb1fca7dd6dd00be3e2f1879cf20d34d2'
PREDICATE = 'https://webdock.dev/provenance/local-barman-fixture/v1'


def sha(path):
    h = hashlib.sha256()
    with Path(path).open('rb') as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b''):
            h.update(chunk)
    return h.hexdigest()


def write_json(path, value):
    Path(path).write_text(json.dumps(value, sort_keys=True, indent=2) + '\n')


def run(args, *, log=None, timeout=900, **kwargs):
    if log:
        with Path(log).open('wb') as out:
            return subprocess.run(args, check=True, stdout=out, stderr=subprocess.STDOUT,
                                  timeout=timeout, **kwargs)
    return subprocess.run(args, check=True, capture_output=True, timeout=timeout, **kwargs).stdout


def immutable(ref):
    if not re.fullmatch(r'[a-z0-9./_-]+@sha256:[0-9a-f]{64}', ref):
        raise ValueError('base must be pinned by an immutable SHA-256 manifest')
    return ref


def material_path(root, name):
    p = (root / name).resolve()
    if not name or Path(name).is_absolute() or '..' in Path(name).parts or not p.is_relative_to(root.resolve()):
        raise ValueError('unsafe material path')
    return p


def new_fixture_key(root):
    key = root / 'fixture-private.pem'
    if key.exists():
        raise ValueError('refusing to replace a signing identity')
    run(['openssl', 'genpkey', '-algorithm', 'ED25519', '-out', str(key)])
    key.chmod(0o600)
    run(['openssl', 'pkey', '-in', str(key), '-pubout', '-out', str(root / 'fixture-public.pem')])


def statement(root, materials, subjects):
    return {
        '_type': 'https://in-toto.io/Statement/v1', 'subject': subjects,
        'predicateType': PREDICATE,
        'predicate': {
            'qualification': 'LOCAL FIXTURE ONLY', 'productionQualified': False,
            'operatorApprovedSigner': False,
            'source': {'repository': REPOSITORY, 'commit': COMMIT, 'signer': SOURCE_SIGNER},
            'builder': {'type': 'bounded-local-docker-run+webdock-oci-packager',
                        'goBase': GO, 'runtimeAndPythonBuilderBase': DEBIAN,
                        'platform': 'linux/amd64', 'networkDuringCompilation': 'none'},
            'patches': [], 'recipe': 'recipe/provenance.py',
            'materials': materials,
            'completedAt': datetime.datetime.now(datetime.timezone.utc).isoformat(),
            'status': 'source-built-local-fixture' if subjects else 'preparation-only',
        },
    }


def sign(root, document):
    write_json(root / 'provenance.json', document)
    run(['openssl', 'pkeyutl', '-sign', '-rawin', '-inkey', str(root / 'fixture-private.pem'),
         '-in', str(root / 'provenance.json'), '-out', str(root / 'provenance.sig')])


def verify_envelope(root, public_pin, require_images=False):
    if sha(root / 'fixture-public.pem') != public_pin:
        raise ValueError('fixture public key does not match separately retained pin')
    run(['openssl', 'pkeyutl', '-verify', '-rawin', '-pubin', '-inkey', str(root / 'fixture-public.pem'),
         '-in', str(root / 'provenance.json'), '-sigfile', str(root / 'provenance.sig')])
    doc = json.loads((root / 'provenance.json').read_text())
    pred = doc['predicate']
    if (doc['_type'] != 'https://in-toto.io/Statement/v1' or doc['predicateType'] != PREDICATE
            or pred['source'] != {'repository': REPOSITORY, 'commit': COMMIT, 'signer': SOURCE_SIGNER}
            or pred['qualification'] != 'LOCAL FIXTURE ONLY' or pred['productionQualified'] is not False
            or pred['operatorApprovedSigner'] is not False):
        raise ValueError('wrong source or fixture policy')
    for name, digest in pred['materials'].items():
        if sha(material_path(root, name)) != digest:
            raise ValueError('material hash mismatch: ' + name)
    if require_images and (len(doc['subject']) != 2 or {s['name'].split(':')[0].rsplit('/', 1)[-1] for s in doc['subject']}
                           != {'webdock-barman-plugin-local', 'webdock-barman-sidecar-local'}
                           or pred['status'] != 'source-built-local-fixture'):
        raise ValueError('both source-built local images are required; preparation is not qualification')
    if require_images:
        required = {'source.tar', 'source-signature.log', 'recipe/provenance.py',
                    'recipe/os_evidence.py', 'recipe/os-evidence.sh', 'recipe/python-build.sh', 'recipe/offline-os.sh',
                    'go-evidence/manager', 'go-evidence/modules-verify.txt',
                    'go-evidence/offline-modules-verify.txt', 'python-evidence/wheels.lock'}
        for component in ('plugin', 'sidecar'):
            required.update({component + '-runtime-files.json', component + '.sbom.cdx.json',
                             component + '-evidence/os-packages.json'})
        if not required.issubset(pred['materials']):
            raise ValueError('provenance is missing mandatory signed build evidence')
        if pred['builder']['goBase'] != GO or pred['builder']['runtimeAndPythonBuilderBase'] != DEBIAN or pred['patches'] != []:
            raise ValueError('unexpected builder bases or source patches')
    return doc


def make_oci(layer, dest, name, labels):
    dest.mkdir()
    blobs = dest / 'blobs' / 'sha256'
    blobs.mkdir(parents=True)
    layer_digest = sha(layer)
    compressed = blobs / 'layer.tmp'
    with layer.open('rb') as src, compressed.open('wb') as out:
        with gzip.GzipFile(fileobj=out, mode='wb', mtime=0) as gz:
            shutil.copyfileobj(src, gz)
    compressed_digest = sha(compressed)
    compressed.rename(blobs / compressed_digest)

    def blob(value, kind):
        data = (json.dumps(value, sort_keys=True, separators=(',', ':')) + '\n').encode()
        digest = hashlib.sha256(data).hexdigest()
        (blobs / digest).write_bytes(data)
        return {'mediaType': kind, 'digest': 'sha256:' + digest, 'size': len(data)}

    config = blob({'architecture': 'amd64', 'os': 'linux',
                   'config': {'User': '65532:65532', 'WorkingDir': '/',
                              'Entrypoint': ['/manager'], 'Env': ['PATH=/venv/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin', 'PYTHONDONTWRITEBYTECODE=1'],
                              'Labels': {**labels, 'dev.webdock.qualification': 'LOCAL FIXTURE ONLY'}},
                   'rootfs': {'type': 'layers', 'diff_ids': ['sha256:' + layer_digest]}},
                  'application/vnd.oci.image.config.v1+json')
    manifest = blob({'schemaVersion': 2, 'mediaType': 'application/vnd.oci.image.manifest.v1+json',
                     'config': config, 'layers': [{'mediaType': 'application/vnd.oci.image.layer.v1.tar+gzip',
                                                  'digest': 'sha256:' + compressed_digest,
                                                  'size': (blobs / compressed_digest).stat().st_size}]},
                    'application/vnd.oci.image.manifest.v1+json')
    write_json(dest / 'index.json', {'schemaVersion': 2, 'manifests': [dict(manifest, annotations={'org.opencontainers.image.ref.name': name})]})
    write_json(dest / 'oci-layout', {'imageLayoutVersion': '1.0.0'})
    return manifest['digest']


def verify_oci(dest):
    index = json.loads((dest / 'index.json').read_text())
    if len(index['manifests']) != 1:
        raise ValueError('expected exactly one local image manifest')
    def read(desc):
        digest = desc['digest']
        if not re.fullmatch(r'sha256:[0-9a-f]{64}', digest):
            raise ValueError('invalid OCI descriptor')
        path = dest / 'blobs' / 'sha256' / digest[7:]
        if path.stat().st_size != desc['size'] or sha(path) != digest[7:]:
            raise ValueError('OCI blob hash/size mismatch')
        return path
    manifest = json.loads(read(index['manifests'][0]).read_text())
    config = json.loads(read(manifest['config']).read_text())
    diffids = []
    for layer in manifest['layers']:
        path = read(layer)
        h = hashlib.sha256()
        with gzip.open(path, 'rb') as f:
            for chunk in iter(lambda: f.read(1024 * 1024), b''):
                h.update(chunk)
        diffids.append('sha256:' + h.hexdigest())
    if diffids != config['rootfs']['diff_ids']:
        raise ValueError('OCI layer diff ID mismatch')
    return index['manifests'][0]['digest']



def verify_image_contents(layout, expected):
    index = json.loads((layout / 'index.json').read_text())
    manifest = json.loads((layout / 'blobs' / 'sha256' / index['manifests'][0]['digest'][7:]).read_text())
    found = set()
    for layer in manifest['layers']:
        with tarfile.open(layout / 'blobs' / 'sha256' / layer['digest'][7:], 'r|gz') as archive:
            for member in archive:
                name = member.name.removeprefix('./')
                if name in expected:
                    wanted = expected[name]
                    if 'symlink' in wanted:
                        valid = member.issym() and member.linkname == wanted['symlink']
                    else:
                        valid = member.isfile() and hashlib.sha256(archive.extractfile(member).read()).hexdigest() == wanted['sha256']
                    if not valid or name in found:
                        raise ValueError('image content differs from built artifact: ' + name)
                    found.add(name)
                elif name.startswith('venv/') and not member.isdir():
                    raise ValueError('unrecorded virtualenv content: ' + name)
    if found != set(expected):
        raise ValueError('image is missing built artifact content')



def verify_imported_image(observed, layout):
    index = json.loads((layout / 'index.json').read_text())
    digest = index['manifests'][0]['digest']
    manifest = json.loads((layout / 'blobs/sha256' / digest[7:]).read_text())
    cfgid = manifest['config']['digest']
    config = json.loads((layout / 'blobs/sha256' / cfgid[7:]).read_text())
    # Moby's containerd store returns target manifest ID; the legacy store uses config ID.
    if observed['Id'] not in (digest, cfgid):
        raise ValueError('Docker import has an unrelated image ID')
    descriptor = observed.get('Descriptor')
    if descriptor and descriptor['digest'] != digest:
        raise ValueError('Docker descriptor differs from packaged manifest')
    if (observed['Architecture'] != config['architecture'] or observed['Os'] != config['os']
            or observed['RootFS']['Layers'] != config['rootfs']['diff_ids']
            or any(observed['Config'].get(k) != v for k, v in config['config'].items())):
        raise ValueError('Docker import changed packaged config or layer identities')
    return cfgid


def fetch(url, path):
    with urllib.request.urlopen(url, timeout=60) as response, Path(path).open('xb') as out:
        shutil.copyfileobj(response, out)


def verify_source(root):
    env = {**os.environ, 'GNUPGHOME': str(root / 'gnupg'), 'GIT_NO_LAZY_FETCH': '1',
           'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_SYSTEM': '/dev/null'}
    proc = subprocess.run(['git', '--no-replace-objects', '-C', str(root / 'source'), '-c', 'core.hooksPath=/dev/null',
                           '-c', 'gpg.program=' + shutil.which('gpg'), '-c', 'gpg.format=openpgp', 'verify-commit', '--raw', COMMIT], env=env, capture_output=True, timeout=60)
    status = proc.stderr.decode()
    if proc.returncode or '[GNUPG:] VALIDSIG ' + SOURCE_SIGNER + ' ' not in status:
        raise ValueError('source commit signature is not valid for the pinned signer')
    archive = run(['git', '--no-replace-objects', '-C', str(root / 'source'), '-c', 'core.hooksPath=/dev/null', 'archive', '--format=tar', COMMIT], env=env)
    if hashlib.sha256(archive).hexdigest() != sha(root / 'source.tar'):
        raise ValueError('build source archive is not the signed commit tree')
    if (root / 'src').exists():
        verify_extracted_source(root)
    return status


def verify_extracted_source(root):
    expected = set()
    with tarfile.open(root / 'source.tar') as archive:
        for member in archive.getmembers():
            if member.isdir():
                continue
            expected.add(member.name)
            path = root / 'src' / member.name
            if member.issym():
                if not path.is_symlink() or os.readlink(path).rstrip('/') != member.linkname.rstrip('/'):
                    raise ValueError('extracted source symlink changed: ' + member.name)
            elif member.isfile():
                if path.is_symlink() or not path.is_file():
                    raise ValueError('extracted source file missing: ' + member.name)
                if sha(path) != hashlib.sha256(archive.extractfile(member).read()).hexdigest():
                    raise ValueError('extracted source content changed: ' + member.name)
            else:
                raise ValueError('unsupported signed source entry')
    actual = {str(p.relative_to(root / 'src')) for p in (root / 'src').rglob('*') if p.is_file() or p.is_symlink()}
    if expected != actual:
        raise ValueError('extracted build source contains unexpected files')


class LocalBuild:
    def __init__(self, root):
        self.root = root
        self.prefix = 'webdock-barman-' + uuid.uuid4().hex[:12]
        self.owned = []
        self.images = []
        self.bases = []
        self.results = []
        self.deadline = datetime.datetime.now(datetime.timezone.utc) + datetime.timedelta(minutes=90)

    def ledger(self):
        write_json(self.root / 'ownership.json', {'prefix': self.prefix, 'containers': self.owned,
                    'images': self.images, 'pulledBases': self.bases, 'volumes': [], 'networks': [], 'existingServicesTouched': False,
                    'deadline': self.deadline.isoformat(), 'cleanup': 'containers removed in finally; local OCI files and imported images retained; remove only listed image IDs after review'})

    def docker(self, args, log=None, timeout=900):
        remaining = (self.deadline - datetime.datetime.now(datetime.timezone.utc)).total_seconds()
        if remaining <= 0:
            raise TimeoutError('90-minute task budget exhausted')
        with (self.root / 'commands.jsonl').open('a') as trace:
            trace.write(json.dumps({'argv': ['docker', *args], 'timeoutSeconds': min(timeout, remaining)}) + '\n')
        return run(['docker', *args], log=log, timeout=min(timeout, remaining))

    def container(self, phase, base, script, *, network=True, mounts=(), memory='2g', timeout=900):
        name = self.prefix + '-' + phase
        self.owned.append(name)
        self.ledger()
        args = ['run', '--name', name, '--platform', 'linux/amd64', '--pull', 'never',
                '--cpus', '2', '--memory', memory, '--memory-swap', memory, '--pids-limit', '512',
                '--security-opt', 'no-new-privileges', '--label', 'dev.webdock.barman-fixture=' + self.prefix,
                '--network', 'bridge' if network else 'none']
        for src, dest, readonly in mounts:
            args += ['--mount', f'type=bind,src={src},dst={dest}' + (',readonly' if readonly else '')]
        script = f"trap 'chown -R {os.getuid()}:{os.getgid()} /evidence' EXIT\n" + script
        args += [immutable(base), 'sh', '-ec', script]
        self.docker(args, log=self.root / (phase + '.log'), timeout=timeout)
        self.docker(['inspect', name], log=self.root / (phase + '.container.json'))
        return name

    def prepare(self):
        root = self.root
        (root / 'recipe').mkdir()
        for name in ('provenance.py', 'os-evidence.sh', 'os_evidence.py', 'python-build.sh', 'offline-os.sh'):
            shutil.copyfile(HERE / name, root / 'recipe' / name)
        tools = {}
        for name in ('python3', 'docker', 'git', 'gpg', 'gpgv', 'openssl'):
            binary = Path(shutil.which(name)).resolve()
            tools[name] = {'path': str(binary), 'sha256': sha(binary)}
        write_json(root / 'host-tools.json', tools)
        new_fixture_key(root)
        fetch('https://github.com/web-flow.gpg', root / 'github-web-flow.gpg')
        (root / 'gnupg').mkdir(mode=0o700)
        run(['gpg', '--batch', '--homedir', str(root / 'gnupg'), '--import', str(root / 'github-web-flow.gpg')], log=root / 'source-key-import.log')
        fetch(f'https://api.github.com/repos/cloudnative-pg/plugin-barman-cloud/commits/{COMMIT}', root / 'github-commit.json')
        run(['git', '-c', 'core.hooksPath=/dev/null', 'clone', '--no-checkout', '--filter=blob:none', '--single-branch', REPOSITORY + '.git', str(root / 'source')], log=root / 'source-clone.log')
        run(['git', '-C', str(root / 'source'), '-c', 'core.hooksPath=/dev/null', 'archive', '--format=tar', '-o', str(root / 'source.tar'), COMMIT])
        (root / 'source-signature.log').write_text(verify_source(root))
        # Only the cryptographically verified archive enters build contexts; no worktree edits.
        (root / 'src').mkdir()
        with tarfile.open(root / 'source.tar') as tar:
            tar.extractall(root / 'src', filter='data')
        for base, name in ((GO, 'go'), (DEBIAN, 'debian')):
            existed = subprocess.run(['docker', 'image', 'inspect', base], capture_output=True, timeout=30).returncode == 0
            self.bases.append({'reference': base, 'preexisting': existed, 'cleanup': 'retained shared immutable pull cache; never automatically deleted'})
            self.ledger()
            self.docker(['pull', '--platform', 'linux/amd64', immutable(base)], log=root / (name + '-pull.log'))
            self.docker(['image', 'inspect', base], log=root / (name + '-base.json'))
        for name in ('gomod', 'gocache', 'go-evidence', 'python-evidence', 'plugin-evidence', 'sidecar-evidence'):
            (root / name).mkdir()
        self.container('go-prepare', GO,
            'sh /recipe/os-evidence.sh go && cd /src && GOTOOLCHAIN=local GOPROXY=https://proxy.golang.org GOSUMDB=sum.golang.org GOFLAGS=-mod=readonly go mod download -json > /evidence/modules-download.json && go mod verify > /evidence/modules-verify.txt && go version > /evidence/go-version.txt && sha256sum /usr/local/go/bin/go > /evidence/compiler.sha256',
            mounts=[(root / 'recipe', '/recipe', True), (root / 'src', '/src', True), (root / 'gomod', '/go/pkg/mod', False), (root / 'go-evidence', '/evidence', False)], memory='4g', timeout=1500)
        self.container('python-prepare', DEBIAN,
            'sh /recipe/os-evidence.sh python && sh /recipe/python-build.sh download',
            mounts=[(root / 'recipe', '/recipe', True), (root / 'src', '/src', True), (root / 'python-evidence', '/evidence', False)], memory='3g', timeout=1500)

    def build(self):
        root = self.root
        verify_source(root)
        self.container('go-build', GO,
            'sh /recipe/offline-os.sh && cd /src && export GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off GOMODCACHE=/go/pkg/mod GOCACHE=/cache GOFLAGS=-mod=readonly && go mod verify > /evidence/offline-modules-verify.txt && CGO_ENABLED=0 GOOS=linux GOARCH=amd64 go build -p 2 -trimpath -buildvcs=false -o /evidence/manager cmd/manager/main.go && go version -m /evidence/manager > /evidence/binary-modules.txt',
            network=False, mounts=[(root / 'recipe', '/recipe', True), (root / 'src', '/src', True), (root / 'gomod', '/go/pkg/mod', True), (root / 'gocache', '/cache', False), (root / 'go-evidence', '/evidence', False)], memory='4g', timeout=1800)
        self.container('python-build', DEBIAN, 'sh /recipe/python-build.sh build',
            network=False, mounts=[(root / 'recipe', '/recipe', True), (root / 'python-evidence', '/evidence', False)],
            memory='3g', timeout=1200)
        for component in ('plugin', 'sidecar'):
            evidence = root / (component + '-evidence')
            script = 'sh /recipe/os-evidence.sh ' + component
            container = self.container(component + '-runtime', DEBIAN, script,
                mounts=[(root / 'recipe', '/recipe', True), (evidence, '/evidence', False)])
            self.docker(['cp', str(root / 'go-evidence' / 'manager'), container + ':/manager'])
            if component == 'sidecar':
                self.docker(['cp', str(root / 'python-evidence' / 'venv'), container + ':/venv'])
            expected = {'manager': {'sha256': sha(root / 'go-evidence' / 'manager')}}
            if component == 'sidecar':
                venv = root / 'python-evidence' / 'venv'
                for path in venv.rglob('*'):
                    name_in_image = 'venv/' + str(path.relative_to(venv))
                    if path.is_symlink():
                        expected[name_in_image] = {'symlink': os.readlink(path)}
                    elif path.is_file():
                        expected[name_in_image] = {'sha256': sha(path)}
            write_json(root / (component + '-runtime-files.json'), expected)
            layer = root / (component + '-rootfs.tar')
            self.docker(['export', '-o', str(layer), container])
            name = 'localhost/webdock-barman-' + component + '-local:' + self.prefix
            layout = root / (component + '.oci')
            digest = make_oci(layer, layout, name, {'org.opencontainers.image.source': REPOSITORY,
                              'org.opencontainers.image.revision': COMMIT,
                              'dev.webdock.recipe.sha256': sha(root / 'recipe' / 'provenance.py')})
            verify_image_contents(layout, expected)
            if verify_oci(layout) != digest:
                raise ValueError('packaged image failed verification')
            tar_path = root / (component + '.oci.tar')
            with tarfile.open(tar_path, 'w') as tar:
                for child in sorted(layout.iterdir()):
                    tar.add(child, arcname=child.name)
            self.docker(['load', '--input', str(tar_path)], log=root / (component + '-load.log'))
            inspect = json.loads(self.docker(['image', 'inspect', name]))[0]
            config_digest = verify_imported_image(inspect, layout)
            self.images.append({'name': name, 'id': inspect['Id'], 'manifestDigest': digest, 'configDigest': config_digest})
            self.ledger()
            # Smoke runs have no network, privileges, writable root or published ports.
            for suffix, args in ([('manager', ['--help'])] if component == 'plugin' else
                                 [('manager', ['--help']), ('barman', ['--version'])]):
                entry = '/manager' if suffix == 'manager' else '/venv/bin/barman-cloud-backup'
                smoke = self.prefix + '-' + component + '-' + suffix
                self.owned.append(smoke)
                self.ledger()
                self.docker(['run', '--name', smoke, '--network', 'none', '--read-only', '--cap-drop', 'ALL',
                             '--security-opt', 'no-new-privileges', '--cpus', '1', '--memory', '256m',
                             '--pids-limit', '64', '--entrypoint', entry, inspect['Id'], *args],
                            log=root / (component + '-' + suffix + '-smoke.log'), timeout=60)
            self.results.append({'name': name, 'digest': {'sha256': digest[7:]}})
            self.sbom(component, digest, name)
        verify_source(root)

    def sbom(self, component, digest, name):
        root = self.root
        packages = json.loads((root / (component + '-evidence') / 'os-packages.json').read_text())
        components = [{'type': 'operating-system', 'name': p['package'], 'version': p['version'],
                       'purl': f"pkg:deb/debian/{p['package']}@{p['version']}?arch={p['file'].rsplit('_', 1)[-1].removesuffix('.deb')}",
                       'hashes': [{'alg': 'SHA-256', 'content': p['sha256']}]} for p in packages]
        modules = (root / 'go-evidence' / 'binary-modules.txt').read_text().splitlines()
        for line in modules:
            fields = line.split()
            if fields and fields[0] == 'dep':
                components.append({'type': 'library', 'name': fields[1], 'version': fields[2],
                                   'purl': 'pkg:golang/' + fields[1] + '@' + fields[2]})
        if component == 'sidecar':
            components += json.loads((root / 'python-evidence' / 'python-sbom.json').read_text())
        write_json(root / (component + '.sbom.cdx.json'), {'bomFormat': 'CycloneDX', 'specVersion': '1.6', 'version': 1,
                   'metadata': {'component': {'type': 'container', 'name': name, 'version': digest,
                   'hashes': [{'alg': 'SHA-256', 'content': digest[7:]}]},
                   'properties': [{'name': 'webdock:qualification', 'value': 'LOCAL FIXTURE ONLY'},
                                  {'name': 'webdock:coverage', 'value': 'dpkg installed packages, linked Go modules, installed venv distributions; not arbitrary vendored code discovery'}]},
                   'components': components})

    def cleanup(self):
        for name in self.owned:
            try:
                # Exact UUID-scoped names only; never prune or stop an existing resource.
                self.docker(['rm', '-f', name], timeout=30)
            except (subprocess.SubprocessError, TimeoutError):
                # Cleanup must still run after the task deadline.
                subprocess.run(['docker', 'rm', '-f', name], capture_output=True, timeout=30)
        self.ledger()


def seal(build, failure=None):
    root = build.root
    materials = {}
    # Cache, private key, Git internals and extracted venv are not public evidence.
    names = ['source.tar', 'github-web-flow.gpg', 'github-commit.json', 'source-signature.log', 'ownership.json', 'host-tools.json', 'commands.jsonl']
    names += [p.name for p in root.glob('*.log')]
    names += [p.name for p in root.glob('*-base.json')]
    names += [p.name for p in root.glob('*.container.json')]
    names += [p.name for p in root.glob('prior-*')]
    names += [str(p.relative_to(root)) for p in (root / 'retired-artifacts').glob('*/manifest.json')]
    names += [str(p.relative_to(root)) for p in (root / 'preparation-recipe').glob('*')]
    names += [p.name for p in root.glob('*.sbom.cdx.json')]
    names += [p.name for p in root.glob('*-runtime-files.json')]
    names += [str(p.relative_to(root)) for p in (root / 'recipe').glob('*')]
    for directory in ('go-evidence', 'python-evidence', 'plugin-evidence', 'sidecar-evidence'):
        if (root / directory).exists():
            names += [str(p.relative_to(root)) for p in (root / directory).rglob('*')
                      if p.is_file() and 'venv' not in p.relative_to(root / directory).parts]
    for name in set(names):
        if (root / name).is_file():
            materials[name] = sha(root / name)
    doc = statement(root, materials, build.results if not failure else [])
    if failure:
        doc['predicate']['blocker'] = failure
    sign(root, doc)
    pin = sha(root / 'fixture-public.pem')
    (root / 'fixture-public.sha256').write_text(pin + '\n')
    return pin


def verify(root, pin, require_images=False):
    doc = verify_envelope(root, pin, require_images)
    verify_source(root)
    for name in ('go-evidence', 'python-evidence', 'plugin-evidence', 'sidecar-evidence'):
        directory = root / name
        if (directory / 'os-packages.json').exists():
            run(['python3', str(HERE / 'os_evidence.py'), 'verify', str(directory)])
    for subject in doc['subject']:
        component = 'plugin' if '/webdock-barman-plugin-local:' in subject['name'] else 'sidecar'
        if verify_oci(root / (component + '.oci')) != 'sha256:' + subject['digest']['sha256']:
            raise ValueError('subject image digest mismatch')
        verify_image_contents(root / (component + '.oci'), json.loads((root / (component + '-runtime-files.json')).read_text()))
        sbom = json.loads((root / (component + '.sbom.cdx.json')).read_text())
        if sbom['metadata']['component']['version'] != 'sha256:' + subject['digest']['sha256']:
            raise ValueError('SBOM subject mismatch')
    return doc


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    for command in ('prepare', 'build'):
        cmd = sub.add_parser(command)
        cmd.add_argument('--output', type=Path, required=True, help='new private directory, outside repository')
    cmd = sub.add_parser('resume-build', help='verify retained preparation with an external fixture pin, then retry offline builds')
    cmd.add_argument('--output', type=Path, required=True)
    cmd.add_argument('--public-key-sha256', required=True)
    cmd = sub.add_parser('verify')
    cmd.add_argument('--output', type=Path, required=True)
    cmd.add_argument('--public-key-sha256', required=True, help='separately retained public fixture pin')
    cmd.add_argument('--require-images', action='store_true')
    args = parser.parse_args()
    if args.command == 'verify':
        doc = verify(args.output.resolve(), args.public_key_sha256, args.require_images)
        print(json.dumps({'status': doc['predicate']['status'], 'subjects': doc['subject'], 'productionQualified': False}, indent=2))
        return
    os.umask(0o077)
    root = args.output.resolve()
    repo = HERE.parents[2]
    if root.is_relative_to(repo):
        parser.error('private output must be outside the repository')
    prior = None
    if args.command == 'resume-build':
        doc = verify(root, args.public_key_sha256)
        if doc['subject'] or doc['predicate']['status'] != 'preparation-only':
            parser.error('only a preparation-only fixture can be resumed')
        required = ('go-evidence/modules-verify.txt', 'python-evidence/downloads.sha256',
                    'python-evidence/os-packages.json', 'go-evidence/os-packages.json')
        if not all(name in doc['predicate']['materials'] for name in required):
            parser.error('preparation did not finish dependency download/authentication')
        prior = json.loads((root / 'ownership.json').read_text())
        attempt = uuid.uuid4().hex[:8]
        for name in ('provenance.json', 'provenance.sig', 'ownership.json'):
            shutil.copyfile(root / name, root / ('prior-' + attempt + '-' + name))
        retired = root / 'retired-artifacts' / attempt
        retired.mkdir(parents=True)
        shutil.copytree(root / 'recipe', retired / 'recipe')
        for component in ('plugin', 'sidecar'):
            layout = root / (component + '.oci')
            if layout.exists():
                digest = verify_oci(layout)
                name = 'localhost/webdock-barman-' + component + '-local:' + prior['prefix']
                result = subprocess.run(['docker', 'image', 'inspect', name], capture_output=True, timeout=30)
                if result.returncode == 0:
                    observed = json.loads(result.stdout)[0]
                    cfgid = verify_imported_image(observed, layout)
                    prior.setdefault('images', []).append({'name': name, 'id': observed['Id'],
                        'manifestDigest': digest, 'configDigest': cfgid, 'retiredFixture': True})
            for suffix in ('.oci', '.oci.tar', '-rootfs.tar', '-runtime-files.json', '.sbom.cdx.json', '-evidence'):
                path = root / (component + suffix)
                if path.exists():
                    path.rename(retired / path.name)
            (root / (component + '-evidence')).mkdir()
        for name in ('wheels', 'wheels.lock', 'python-sbom.json', 'venv', 'pip-check.txt'):
            path = root / 'python-evidence' / name
            if path.exists():
                path.rename(retired / ('python-' + name))
        write_json(retired / 'manifest.json', {str(p.relative_to(retired)): sha(p)
                   for p in retired.rglob('*') if p.is_file() and not p.is_symlink()})
        if not (root / 'preparation-recipe').exists():
            shutil.copytree(root / 'recipe', root / 'preparation-recipe')
        for name in ('provenance.py', 'os-evidence.sh', 'os_evidence.py', 'python-build.sh', 'offline-os.sh'):
            shutil.copyfile(HERE / name, root / 'recipe' / name)
    else:
        root.mkdir(mode=0o700, parents=False, exist_ok=False)
    build = LocalBuild(root)
    if prior:
        build.owned = prior['containers']
        build.bases = prior.get('pulledBases', [])
        build.images = prior.get('images', [])
    failure = None
    try:
        if args.command != 'resume-build':
            build.prepare()
        if args.command in ('build', 'resume-build'):
            build.build()
    except Exception as error:
        failure = type(error).__name__ + ': ' + str(error)
    finally:
        build.cleanup()
        pin = seal(build, failure)
    print(json.dumps({'privateOutputDirectory': str(root), 'fixturePublicKeySha256': pin,
                      'subjects': build.results, 'blocker': failure, 'productionQualified': False}, indent=2))
    if failure:
        raise SystemExit(1)
    verify(root, pin, require_images=args.command in ('build', 'resume-build'))


if __name__ == '__main__':
    main()
