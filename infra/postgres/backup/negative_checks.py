#!/usr/bin/env python3
"""Run reversible tamper checks against an idle local fixture, restoring every byte."""
import argparse
import json
from pathlib import Path
import subprocess

import os_evidence
import provenance as p


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--public-key-sha256', required=True)
    args = parser.parse_args()
    root = args.output.resolve()
    p.verify(root, args.public_key_sha256, require_images=True)
    results = []

    def rejects(label, action):
        try:
            action()
        except (ValueError, subprocess.CalledProcessError):
            results.append({'case': label, 'result': 'rejected'})
        else:
            raise AssertionError('tamper was accepted: ' + label)

    def corrupt(label, path, action):
        # Change one byte, never rewrite an entire image/cache artifact.
        with path.open('r+b') as f:
            original = f.read(1)
            if not original:
                raise ValueError('empty tamper target')
            f.seek(0)
            f.write(bytes([original[0] ^ 1]))
        try:
            rejects(label, action)
        finally:
            with path.open('r+b') as f:
                f.write(original)

    envelope = lambda: p.verify_envelope(root, args.public_key_sha256, require_images=True)
    rejects('foreign public fixture pin', lambda: p.verify_envelope(root, '0' * 64))
    corrupt('signed provenance altered', root / 'provenance.json', envelope)
    corrupt('source archive altered', root / 'source.tar', envelope)
    corrupt('extracted compiler input altered', root / 'src/cmd/manager/main.go', lambda: p.verify_source(root))
    for component in ('plugin', 'sidecar'):
        layout = root / (component + '.oci')
        index = json.loads((layout / 'index.json').read_text())
        manifest = json.loads((layout / 'blobs/sha256' / index['manifests'][0]['digest'][7:]).read_text())
        blob = layout / 'blobs/sha256' / manifest['layers'][0]['digest'][7:]
        corrupt(component + ' OCI layer altered', blob, lambda: p.verify_oci(layout))
    osroot = root / 'sidecar-evidence'
    inrelease = next((osroot / 'os').glob('*.InRelease'))
    packages = next((osroot / 'os').glob('*.Packages'))
    deb = next((osroot / 'os/debs').glob('*.deb'))
    corrupt('OS repository signature altered', inrelease, lambda: os_evidence.verify(osroot))
    corrupt('OS Packages index altered', packages, lambda: os_evidence.verify(osroot))
    corrupt('OS package payload altered', deb, lambda: os_evidence.verify(osroot))
    wheel = next((root / 'python-evidence/downloads').iterdir())
    corrupt('Python dependency artifact altered', wheel, envelope)
    p.verify(root, args.public_key_sha256, require_images=True)
    print(json.dumps({'baselineAndRestoredBundle': 'verified', 'negativeCases': results}, indent=2))


if __name__ == '__main__':
    main()
