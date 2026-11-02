#!/usr/bin/env python3
"""Retain and independently verify Debian InRelease -> Packages -> every installed .deb.

The pinned base's archive keyring is the explicit bootstrap trust anchor. This is
repository authentication, not independent reproducibility of the OCI base.
"""
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys

KEYRING_SHA256 = '506b815cbb32d9b6066b4a2aa524071e071761e7e7f68c3ac74f3061ba852017'


def sha(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b''):
            h.update(chunk)
    return h.hexdigest()


def records(path):
    with path.open() as f:
        doc, field = {}, None
        for line in f:
            if line == '\n':
                if doc:
                    yield doc
                doc, field = {}, None
            elif line[0] in ' \t' and field:
                doc[field] += '\n' + line.strip()
            elif ': ' in line:
                field, value = line.rstrip('\n').split(': ', 1)
                doc[field] = value
            elif line.rstrip().endswith(':'):
                field = line.rstrip()[:-1]
                doc[field] = ''
        if doc:
            yield doc


def verify(root):
    base = root / 'os'
    keyring = base / 'archive-keyring.pgp'
    if sha(keyring) != KEYRING_SHA256:
        raise ValueError('OS archive trust anchor changed')
    inventory = json.loads((root / 'os-packages.json').read_text())
    repos = json.loads((base / 'repositories.json').read_text())
    expected = {(p['package'], p['version']): p for p in inventory}
    authenticated = {}
    for repo in repos:
        if not re.fullmatch(r'[a-zA-Z0-9_.-]+', repo['name']):
            raise ValueError('invalid repository evidence path')
        name = repo['name']
        # Do not consume a caller-supplied plaintext Release: always re-verify signature.
        result = subprocess.run(['gpgv', '--status-fd=1', '--keyring', str(keyring.resolve()),
                                 str(base / (name + '.InRelease'))], capture_output=True, check=True, timeout=30)
        if b'[GNUPG:] VALIDSIG ' not in result.stdout:
            raise ValueError('no authenticated OS repository signature')
        signed = (base / (name + '.InRelease')).read_text()
        # Cleartext OpenPGP dash-unescaping; the complete signed bytes were checked above.
        release = signed.split('\n\n', 1)[1].split('-----BEGIN PGP SIGNATURE-----', 1)[0]
        release = '\n'.join(line[2:] if line.startswith('- ') else line for line in release.splitlines())
        hashes = {}
        in_sha = False
        for line in release.splitlines():
            if line == 'SHA256:':
                in_sha = True
            elif line and not line.startswith(' '):
                in_sha = False
            elif in_sha:
                digest, size, path = line.split()
                hashes[path] = (digest, int(size))
        packages = base / (name + '.Packages')
        path = 'main/binary-amd64/Packages'
        if hashes.get(path) != (sha(packages), packages.stat().st_size):
            raise ValueError('Packages index not bound by signed InRelease: ' + name)
        for package in records(packages):
            identity = (package.get('Package'), package.get('Version'))
            if identity in expected:
                authenticated.setdefault(identity, set()).add(package['SHA256'])
    for identity, package in expected.items():
        if package['sha256'] not in authenticated.get(identity, set()):
            raise ValueError('installed package not authenticated by signed repository: ' + str(identity))
        path = base / 'debs' / package['file']
        if Path(package['file']).name != package['file'] or sha(path) != package['sha256']:
            raise ValueError('OS package content was altered: ' + package['file'])
    print(json.dumps({'installedPackagesVerified': len(inventory), 'signedRepositories': len(repos)}))


def collect(root):
    base = root / 'os'
    repos = []
    lists = Path('/var/lib/apt/lists')
    for release in sorted(lists.glob('*_InRelease')):
        prefix = release.name[:-len('_InRelease')]
        packages = next(iter(lists.glob(prefix + '_main_binary-amd64_Packages*')), None)
        if packages is None:
            raise ValueError('missing authenticated Packages index: ' + prefix)
        shutil.copyfile(release, base / (prefix + '.InRelease'))
        with (base / (prefix + '.Packages')).open('wb') as out:
            subprocess.run(['/usr/lib/apt/apt-helper', 'cat-file', str(packages)], stdout=out, check=True, timeout=60)
        repos.append({'name': prefix})
    if len(repos) != 3:
        raise ValueError('expected Debian main, updates and security signed repositories')
    (base / 'repositories.json').write_text(json.dumps(repos, indent=2) + '\n')
    output = subprocess.check_output(['dpkg-query', '-W', '-f=${db:Status-Abbrev}\t${binary:Package}\t${Version}\n'], text=True)
    installed = []
    for line in output.splitlines():
        status, package, version = line.split('\t')
        if status.startswith('ii'):
            installed.append((package, version))
    (base / 'debs').mkdir()
    # Explicit exact versions, including inherited base packages. Missing historical
    # packages fail closed; no downgrade or unsigned archive fallback is permitted.
    subprocess.run(['apt-get', 'download', *[p + '=' + v for p, v in installed]],
                   cwd=base / 'debs', check=True, timeout=600)
    inventory = []
    for deb in sorted((base / 'debs').glob('*.deb')):
        package, version = subprocess.check_output(['dpkg-deb', '-f', str(deb), 'Package', 'Version'], text=True).splitlines()
        package = package.removeprefix('Package: ')
        version = version.removeprefix('Version: ')
        inventory.append({'package': package, 'version': version, 'file': deb.name, 'sha256': sha(deb)})
    if {(p.split(':')[0], v) for p, v in installed} != {(p['package'], p['version']) for p in inventory}:
        raise ValueError('downloaded packages do not cover every installed OS package')
    (root / 'os-packages.json').write_text(json.dumps(inventory, indent=2) + '\n')
    verify(root)


def verify_installed(root):
    verify(root)
    expected = {(p['package'], p['version']) for p in json.loads((root / 'os-packages.json').read_text())}
    output = subprocess.check_output(['dpkg-query', '-W', '-f=${db:Status-Abbrev}\t${binary:Package}\t${Version}\n'], text=True)
    installed = set()
    for line in output.splitlines():
        status, package, version = line.split('\t')
        if status.startswith('ii'):
            installed.add((package.split(':')[0], version))
        elif not status.startswith('rc'):
            raise ValueError('unconfigured offline OS package: ' + package)
    if installed != expected:
        raise ValueError('offline builder differs from authenticated package lock')
    (root / 'offline-installed-packages.json').write_text(json.dumps(sorted(installed), indent=2) + '\n')


if __name__ == '__main__':
    if len(sys.argv) != 3 or sys.argv[1] not in ('collect', 'verify', 'verify-installed'):
        raise SystemExit('usage: os_evidence.py collect|verify EVIDENCE_DIRECTORY')
    {'collect': collect, 'verify': verify, 'verify-installed': verify_installed}[sys.argv[1]](Path(sys.argv[2]))
