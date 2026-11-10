#!/usr/bin/env python3
"""Build with qualified manual rootless BuildKit or the managed Blacksmith builder."""
from __future__ import annotations
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile

MAX_FAILURE_LOG_BYTES = 2 * 1024 * 1024
MAX_FAILURE_LOG_FILES = 10


class BuildFailed(ValueError):
    def __init__(self, private_log):
        super().__init__('Qualified OCI build failed')
        self.private_log = private_log


def failure_directory(home):
    home = Path(home)
    require(home.is_absolute() and home.resolve() == home and home.is_dir() and home.stat().st_uid == os.getuid(), 'Canonical owned HOME required for private diagnostics')
    directory = home / 'ci-failures'
    require(not directory.is_symlink(), 'Private diagnostic directory cannot be a symlink')
    directory.mkdir(mode=0o700, exist_ok=True)
    info = directory.stat()
    require(directory.resolve() == directory and info.st_uid == os.getuid() and stat.S_IMODE(info.st_mode) == 0o700, 'Private diagnostic directory must be owned mode 0700')
    return directory


def retain_failure_log(log, home, secrets):
    directory = failure_directory(home)
    # Read enough overlap to redact credentials before truncating the retained tail.
    with Path(log).open('rb') as source:
        size = source.seek(0, os.SEEK_END)
        offset = max(0, size - MAX_FAILURE_LOG_BYTES - 8192)
        source.seek(offset)
        raw = source.read(MAX_FAILURE_LOG_BYTES + 8192)
    if offset:
        raw = raw.split(b'\n', 1)[1] if b'\n' in raw else b''
    for secret in sorted({value.encode() for value in secrets if value}, key=len, reverse=True):
        raw = raw.replace(secret, b'[REDACTED]')
    descriptor, filename = tempfile.mkstemp(prefix='plugin-build-failure-', suffix='.log', dir=directory)
    with os.fdopen(descriptor, 'wb') as output:
        output.write(raw[-MAX_FAILURE_LOG_BYTES:])
    files = []
    for path in directory.glob('plugin-build-failure-*.log'):
        info = path.lstat()
        require(stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and info.st_nlink == 1, 'Private diagnostic files must be owned regular single-link files')
        files.append((info.st_mtime_ns, path))
    for _, path in sorted(files, reverse=True)[MAX_FAILURE_LOG_FILES:]:
        path.unlink()
    return Path(filename)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def metadata_digest(metadata):
    value = metadata.get('containerimage.digest')
    require(isinstance(value, str) and re.fullmatch(r'sha256:[a-f0-9]{64}', value)
            and not re.search(r'([a-f0-9])\1{63}$', value), 'Actual OCI image digest required')
    return value


def validate_socket(address, uid=None, runtime_root=None):
    uid = os.getuid() if uid is None else uid
    runtime_root = Path('/run/user') / str(uid) if runtime_root is None else Path(runtime_root)
    require(uid != 0 and isinstance(address, str) and address.startswith('unix:///'), 'Non-root Unix BuildKit service required')
    path = Path(address.removeprefix('unix://'))
    require(path.is_absolute() and path.resolve() == path and path.is_relative_to(runtime_root), 'BuildKit socket must be canonical under the runner runtime directory')
    info = path.stat()
    require(not path.is_symlink() and stat.S_ISSOCK(info.st_mode) and info.st_uid == uid, 'BuildKit socket must belong to the runner UID')
    return str(path)


def qualified_buildctl(config):
    executable = shutil.which('buildctl')
    require(executable, 'Preinstalled qualified buildctl required')
    path = Path(executable).resolve()
    info = path.stat()
    expected = config.get('SPITZLI_BUILDCTL_SHA256', '')
    require(re.fullmatch(r'[a-f0-9]{64}', expected) and stat.S_ISREG(info.st_mode)
            and info.st_uid in {0, os.getuid()} and info.st_mode & 0o022 == 0,
            'Pinned operator-qualified buildctl binary required')
    require(hashlib.sha256(path.read_bytes()).hexdigest() == expected, 'buildctl binary checksum mismatch')
    version = subprocess.run([str(path), '--version'], check=True, capture_output=True, text=True, timeout=10)
    require(re.search(r'\bv0\.34\.1(?:\s|$)', version.stdout), 'Qualified BuildKit v0.34.1 required')
    return str(path)


def selected_dockerfile(context, value='Dockerfile'):
    relative = Path(value)
    require(not relative.is_absolute() and '..' not in relative.parts and relative.name in {'Dockerfile', 'backend.Dockerfile'}, 'Repository Dockerfile path required')
    path = context / relative
    require(path.resolve() == path and path.is_relative_to(context) and path.is_file(), 'Real Dockerfile inside source context required')
    return relative.as_posix()


def require_blacksmith_region(config):
    region = config.get('BLACKSMITH_REGION', '')
    require(config.get('GITHUB_ACTIONS') == 'true' and region in {'us-east', 'us-west', 'eu-central', 'eu-west'}
            and region == config.get('SPITZLI_BLACKSMITH_REGION'), 'Actual Blacksmith job and matching recorded region required')


def require_fresh_blacksmith_builder(config):
    require_blacksmith_region(config)
    result = subprocess.run(['pgrep', '-x', 'buildkitd'], capture_output=True, text=True, timeout=10)
    require(result.returncode == 1, 'Unexpected pre-existing BuildKit daemon; refusing the provider local-builder path')


def validate_blacksmith_builder(raw):
    require(isinstance(raw, str) and len(raw) <= 128 * 1024, 'Bounded Buildx inspection required')
    names = re.findall(r'^Name:\s+(\S+)\s*$', raw, re.MULTILINE)
    require(len(names) == 2 and re.fullmatch(r'blacksmith-[a-z0-9]+', names[0]), 'One selected Blacksmith builder node required')
    for field, expected in [('Driver', 'remote'), ('Endpoint', 'tcp://127.0.0.1:1234'), ('Status', 'running')]:
        require(re.findall('^' + field + r':\s+(\S+)\s*$', raw, re.MULTILINE) == [expected], 'Expected running Blacksmith remote loopback builder')
    require(not re.search(r'^Error:', raw, re.MULTILINE) and 'no-process-sandbox' not in raw and 'unconfined' not in raw,
            'Failed or sandbox-bypassing builder forbidden')
    return names[0]


def qualified_blacksmith_builder(config):
    require_blacksmith_region(config)
    docker = shutil.which('docker')
    require(docker, 'Blacksmith Docker CLI and Buildx required')
    original = Path(config.get('DOCKER_CONFIG', str(Path(config['HOME']) / '.docker')))
    require(original.is_absolute() and original.resolve() == original and original.is_dir(), 'Canonical provider Docker configuration required')
    require(not config.get('BUILDX_CONFIG') or config['BUILDX_CONFIG'] == str(original / 'buildx'), 'Unexpected Buildx configuration override')
    environment = {name: config[name] for name in ['PATH', 'HOME', 'USER', 'LANG', 'LC_ALL', 'XDG_RUNTIME_DIR'] if name in config}
    environment.update(DOCKER_CONFIG=str(original), BUILDX_CONFIG=str(original / 'buildx'))
    version = subprocess.run([docker, 'buildx', 'version'], env=environment, check=True, capture_output=True, text=True, timeout=10)
    require(re.search(r'\bv0\.23\.0(?:\s|$)', version.stdout), 'Reviewed Buildx v0.23.0 required')
    inspection = subprocess.run([docker, 'buildx', 'inspect'], env=environment, check=True, capture_output=True, text=True, timeout=30)
    return docker, validate_blacksmith_builder(inspection.stdout), str(original)


def build_image(context, temporary_root, config, token, check=False, bootstrap=False):
    context = Path(context).resolve()
    temporary_root = Path(temporary_root).resolve()
    require(not temporary_root.is_relative_to(context), 'Private runner temporary files must remain outside the build context')
    require(not (check and bootstrap), 'Select one build target')
    dockerfile = selected_dockerfile(context, 'deploy/docker/backend.Dockerfile' if bootstrap else config.get('SPITZLI_PLUGIN_DOCKERFILE', 'Dockerfile'))
    archive = temporary_root / 'webdock-backend.oci.tar'
    if bootstrap:
        require(not archive.exists() and not archive.is_symlink(), 'Bootstrap archive must not already exist')
    repository, revision, actor = (config.get(name, '') for name in ['IMAGE_REPOSITORY', 'GITHUB_SHA', 'GITHUB_ACTOR'])
    require(re.fullmatch(r'ghcr\.io/[a-z0-9._-]+/[a-z0-9._-]+', repository), 'Exact GHCR image repository required')
    source_repository = config.get('GITHUB_REPOSITORY', repository.removeprefix('ghcr.io/'))
    require(re.fullmatch(r'[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+', source_repository)
            and source_repository.split('/')[0].lower() == repository.split('/')[1], 'Source repository must belong to the image owner')
    require(re.fullmatch(r'[a-f0-9]{40}', revision), 'Exact source revision required')
    require(re.fullmatch(r'[A-Za-z0-9_-]+(?:\[bot\])?', actor), 'GitHub actor required')
    require(isinstance(token, str) and (token == '' if check or bootstrap else re.fullmatch(r'[A-Za-z0-9_.-]{10,2048}', token)), 'Publishing token required only for the production image push')
    backend = config.get('SPITZLI_OCI_BACKEND', 'rootless-buildctl')
    require(backend in {'rootless-buildctl', 'blacksmith-buildx'}, 'Explicit supported OCI backend required')
    if backend == 'blacksmith-buildx':
        docker, builder, provider_config = qualified_blacksmith_builder(config)
        require(not Path(provider_config).is_relative_to(context), 'Provider builder state must stay outside the source context')
    else:
        address = config.get('SPITZLI_BUILDKIT_ADDR', '')
        validate_socket(address)
        buildctl = qualified_buildctl(config)
    with tempfile.TemporaryDirectory(prefix='plugin-build-', dir=temporary_root) as temporary:
        private = Path(temporary)
        os.chmod(private, 0o700)
        docker_config = private / 'auth'
        docker_config.mkdir(mode=0o700)
        auth = base64.b64encode((actor + ':' + token).encode()).decode() if token else None
        with open(docker_config / 'config.json', 'x', opener=lambda name, flags: os.open(name, flags, 0o600)) as output:
            authentication = {'auths': {'ghcr.io': {'auth': auth}} if auth else {}}
            if backend == 'blacksmith-buildx':
                authentication['cliPluginsExtraDirs'] = [str(Path(provider_config) / 'cli-plugins')]
            json.dump(authentication, output)
        metadata = private / 'metadata.json'
        environment = {name: config[name] for name in ['PATH', 'HOME', 'USER', 'LANG', 'LC_ALL', 'XDG_RUNTIME_DIR'] if name in config}
        environment['DOCKER_CONFIG'] = str(docker_config)
        source_label = 'org.opencontainers.image.source=https://github.com/' + source_repository
        revision_label = 'org.opencontainers.image.revision=' + revision
        image_output = 'type=image,name=' + repository + ':' + revision + ',push=true'
        if backend == 'blacksmith-buildx':
            # Keep provider selection/plugin discovery while isolating per-push registry authentication.
            environment['BUILDX_CONFIG'] = str(Path(provider_config) / 'buildx')
            arguments = [docker, 'buildx', 'build', '--builder', builder, '--progress=plain', '--file', str(context / dockerfile),
                         '--platform', 'linux/amd64', '--label', source_label, '--label', revision_label]
            if bootstrap:
                arguments += ['--target', 'backend', '--build-arg', 'SOURCE_REVISION=' + revision,
                              '--provenance=false', '--sbom=false', '--output', 'type=oci,dest=' + str(archive)]
            elif check:
                arguments += ['--target', 'check']
            else:
                arguments += ['--provenance=mode=max', '--output', image_output, '--metadata-file', str(metadata)]
            arguments.append(str(context))
        else:
            arguments = [buildctl, '--addr', address, 'build', '--progress=plain', '--frontend=dockerfile.v0',
                         '--local', 'context=' + str(context), '--local', 'dockerfile=' + str(context),
                         '--opt', 'filename=' + dockerfile, '--opt', 'platform=linux/amd64',
                         '--opt', 'label:' + source_label, '--opt', 'label:' + revision_label]
            if bootstrap:
                arguments += ['--opt', 'target=backend', '--opt', 'build-arg:SOURCE_REVISION=' + revision,
                              '--output', 'type=oci,dest=' + str(archive)]
            elif check:
                arguments += ['--opt', 'target=check']
            else:
                arguments += ['--opt', 'attest:provenance=mode=max', '--output', image_output, '--metadata-file', str(metadata)]
        # Never print source-controlled build text as workflow commands.
        try:
            with open(private / 'build.log', 'xb', opener=lambda name, flags: os.open(name, flags, 0o600)) as log:
                result = subprocess.run(arguments, env=environment, stdout=log, stderr=subprocess.STDOUT, timeout=1500)
            require(result.returncode == 0, 'Qualified OCI build/push failed')
            if bootstrap:
                require(archive.is_file() and not archive.is_symlink() and 0 < archive.stat().st_size <= 480 * 1024 * 1024, 'Bounded bootstrap OCI archive required')
                return None
            if check:
                return None
            require(metadata.is_file() and not metadata.is_symlink() and metadata.stat().st_size <= 128 * 1024, 'Bounded BuildKit image metadata required')
            return metadata_digest(json.loads(metadata.read_text()))
        except Exception:
            require(not (Path(config['HOME']) / 'ci-failures').resolve().is_relative_to(context), 'Persistent diagnostics must remain outside the source context')
            retained = retain_failure_log(private / 'build.log', config['HOME'], [token, auth, actor + ':' + token if token else ''])
            raise BuildFailed(retained) from None


def main():
    if sys.argv[1:] == ['--before-blacksmith-setup']:
        require_fresh_blacksmith_builder(os.environ)
        print('Fresh Blacksmith builder setup required; no existing daemon detected')
        return
    if sys.argv[1:] == ['--assert-blacksmith-builder']:
        _, builder, _ = qualified_blacksmith_builder(os.environ)
        print(json.dumps({'builder': builder, 'driver': 'remote', 'endpoint': 'tcp://127.0.0.1:1234', 'region': os.environ['BLACKSMITH_REGION']}))
        return
    require(sys.argv[1:] in [[], ['--check'], ['--bootstrap']], 'Select production image, --check or --bootstrap')
    check = sys.argv[1:] == ['--check']
    bootstrap = sys.argv[1:] == ['--bootstrap']
    token = sys.stdin.buffer.read(2049)
    require(len(token) <= 2048, 'GitHub job token exceeds bounds')
    digest = build_image(Path.cwd(), Path(os.environ['RUNNER_TEMP']), os.environ, token.decode().strip(), check, bootstrap)
    if bootstrap:
        print('Qualified bootstrap OCI archive exported without publishing')
        return
    if check:
        print('Qualified Dockerfile check target passed')
        return
    image = os.environ['IMAGE_REPOSITORY'] + '@' + digest
    if os.environ.get('GITHUB_OUTPUT'):
        with open(os.environ['GITHUB_OUTPUT'], 'a') as output:
            output.write('digest=' + digest + '\nimage=' + image + '\n')
    print('Pushed immutable image through qualified BuildKit: ' + image)

if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        print('OCI release failed; check qualified runner/backend, registry access and source build.', file=sys.stderr)
        if isinstance(error, BuildFailed):
            path = str(error.private_log).replace('\r', '?').replace('\n', '?')
            print('Private redacted build log: ' + path, file=sys.stderr)
        raise SystemExit(1) from None
