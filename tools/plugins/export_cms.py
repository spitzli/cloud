#!/usr/bin/env python3
"""Export only the reviewed CMS miniworkspace. No Git, publication, or credentials."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import tempfile

APPLICATIONS = ['extensions/managed-cms', 'apps/cms-instance']
PACKAGES = ['plugin-sdk', 'cms-ui', 'cms-bridge', 'payload-sso', 'oauth-session', 'instance-kit', 'i18n', 'database-runtime']
ROOT_FILES = ['LICENSE', 'COMMERCIAL-LICENSING.md', 'NOTICE']
SOURCE_DIRS = {'src', 'ui', 'tests', 'compiled', 'locales'}
SOURCE_FILES = {'package.json', 'tsconfig.json', 'README.md', 'build.ts', 'manifest.ts', 'next.config.ts', 'build-migration.mjs'}
EXCLUDED_DIRS = {'node_modules', 'dist', '.next', '.git', 'coverage', '__pycache__', '.cache', 'private', 'secrets', 'data', 'runtime'}
WITHHELD = {'extensions/managed-cms/tests/image-smoke.mjs', 'packages/cms-bridge/tests/postgres.test.ts'}
SOURCE_SUFFIXES = {'.ts', '.tsx', '.mjs', '.json', '.css', '.html', '.md', '.po', '.pot'}
SECRET = re.compile(rb'-----BEGIN (?:RSA |EC |OPENSSH |ENCRYPTED )?PRIVATE KEY-----|github_pat_[A-Za-z0-9_]{20,}|gh[pousr]_[A-Za-z0-9]{30,}|AKIA[A-Z0-9]{16}')
BUN_IMAGE = 'oven/bun:1.4.3-slim@sha256:14a223fa35747ec728bce300ef85888bc5f7958164d15bbba6a54b22b52a3288'


def package_paths():
    return APPLICATIONS + ['packages/' + name for name in PACKAGES]


def checked_bytes(path: Path) -> bytes:
    if path.is_symlink() or not path.is_file():
        raise ValueError('Only regular source files are allowed: ' + str(path))
    if path.stat().st_nlink != 1:
        raise ValueError('Hard-linked source files are forbidden: ' + str(path))
    raw = path.read_bytes()
    if len(raw) > 8 * 1024 * 1024 or SECRET.search(raw):
        raise ValueError('Secret-shaped or oversized source file: ' + str(path))
    return raw


def copy_source(source: Path, output: Path, relative: Path):
    raw = checked_bytes(source / relative)
    target = output / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(raw)


def select_files(source: Path):
    for relative in package_paths():
        base = source / relative
        if base.is_symlink() or not (base / 'package.json').is_file():
            raise ValueError('Missing real CMS package: ' + relative)
        for entry in sorted(base.iterdir()):
            if entry.name in SOURCE_FILES:
                yield entry.relative_to(source)
            elif entry.name in SOURCE_DIRS:
                if entry.is_symlink():
                    raise ValueError('Source directory symlink: ' + str(entry))
                for current, directories, files in os.walk(entry, followlinks=False):
                    directories[:] = sorted(name for name in directories if name not in EXCLUDED_DIRS and not name.startswith('.'))
                    for name in directories:
                        if (Path(current) / name).is_symlink():
                            raise ValueError('Nested source directory symlink')
                    for name in sorted(files):
                        item = Path(current) / name
                        rel = item.relative_to(source)
                        if name.startswith('.') or str(rel) in WITHHELD:
                            continue
                        if item.is_symlink():
                            raise ValueError('Source file symlink: ' + str(rel))
                        if item.suffix in SOURCE_SUFFIXES:
                            yield rel


def rewrite_workspace_packages(output: Path):
    names = {'@webdock/' + name for name in PACKAGES}
    names.update(json.loads((output / path / 'package.json').read_text())['name'] for path in APPLICATIONS)
    for relative in package_paths():
        path = output / relative / 'package.json'
        package = json.loads(path.read_text())
        for field in ['dependencies', 'devDependencies', 'peerDependencies', 'optionalDependencies']:
            for name in package.get(field, {}):
                value = package[field][name]
                if name.startswith('@webdock/'):
                    if name not in names:
                        raise ValueError('Dependency outside CMS closure: ' + name)
                    package[field][name] = 'workspace:*'
                elif isinstance(value, str) and (value.startswith(('file:', 'link:', 'workspace:')) or '../' in value):
                    raise ValueError('External path dependency: ' + name)
        # i18n's repository-wide extraction/compile tools are not part of a CMS runtime export.
        if relative == 'packages/i18n':
            package.pop('scripts', None)
        path.write_text(json.dumps(package, indent=2) + '\n')


def dockerfile(kind: str) -> str:
    prefix = f'FROM {BUN_IMAGE} AS build\nWORKDIR /app\nCOPY . .\nRUN bun install --frozen-lockfile --ignore-scripts\n'
    if kind == 'plugin':
        return prefix + f'''RUN bun run --filter @webdock/plugin-sdk build && bun extensions/managed-cms/build.ts
FROM {BUN_IMAGE}
WORKDIR /app
COPY --from=build --chown=65532:65532 /app/extensions/managed-cms/dist ./
COPY --from=build /app/LICENSE /app/COMMERCIAL-LICENSING.md /app/NOTICE /usr/share/licenses/webdock/
ENV NODE_ENV=production
USER 65532:65532
EXPOSE 3443 3444
CMD ["bun","runtime.js"]
'''
    return prefix + f'''RUN bun run --filter @webdock/cms-instance build
RUN rm -rf apps/cms-instance/.next/standalone/node_modules/drizzle-kit apps/cms-instance/.next/standalone/node_modules/esbuild apps/cms-instance/.next/standalone/node_modules/@esbuild apps/cms-instance/.next/standalone/node_modules/payload/node_modules/esbuild apps/cms-instance/.next/standalone/node_modules/payload/node_modules/@esbuild apps/cms-instance/.next/standalone/node_modules/payload/node_modules/tsx
FROM {BUN_IMAGE}
ENV NODE_ENV=production PORT=3000 HOSTNAME=0.0.0.0 NEXT_TELEMETRY_DISABLED=1
WORKDIR /app
COPY --from=build --chown=65532:65532 /app/apps/cms-instance/.next/standalone ./
COPY --from=build --chown=65532:65532 /app/apps/cms-instance/.next/static ./apps/cms-instance/.next/static
COPY --from=build --chown=65532:65532 /app/apps/cms-instance/.next/migrate.mjs ./migrate.mjs
COPY --from=build /app/LICENSE /app/COMMERCIAL-LICENSING.md /app/NOTICE /usr/share/licenses/webdock/
RUN mkdir -p /data/media /data/transfers && chown -R 65532:65532 /data
USER 65532:65532
VOLUME ["/data"]
EXPOSE 3000
HEALTHCHECK --interval=30s --timeout=5s --start-period=30s CMD ["bun","-e","fetch('http://127.0.0.1:3000/api/health').then(r=>{{if(!r.ok)process.exit(1)}}).catch(()=>process.exit(1))"]
CMD ["bun","apps/cms-instance/server.js"]
'''


def export_cms(source: Path, output: Path) -> dict:
    source, output = source.resolve(), output.absolute()
    if output.exists() or output.is_symlink() or output.is_relative_to(source):
        raise ValueError('Destination must be new and outside the source repository')
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.cms-export-', dir=output.parent) as temporary:
        staging = Path(temporary) / 'source'
        staging.mkdir()
        for relative in select_files(source):
            copy_source(source, staging, relative)
        for name in ROOT_FILES:
            copy_source(source, staging, Path(name))
        rewrite_workspace_packages(staging)
        upstream = json.loads(checked_bytes(source / 'package.json'))
        patched = {}
        for name, relative in upstream.get('patchedDependencies', {}).items():
            if name.split('@')[0] in {'drizzle-orm'} or name.startswith(('@payloadcms/storage-s3@', '@payloadcms/plugin-cloud-storage@')):
                path = Path(relative)
                if path.is_absolute() or '..' in path.parts or path.parts[0] != 'patches':
                    raise ValueError('Patch path outside reviewed source')
                copy_source(source, staging, path)
                patched[name] = relative
        plugin = json.loads((staging / APPLICATIONS[0] / 'package.json').read_text())
        root = {'name': 'webdock-extension-managed-cms', 'version': plugin['version'], 'private': True,
                'type': 'module', 'packageManager': 'bun@1.4.3', 'workspaces': package_paths(),
                'scripts': {'build:sdk': 'bun run --filter @webdock/plugin-sdk build',
                            'build:plugin': 'bun run build:sdk && bun extensions/managed-cms/build.ts',
                            'build:instance': 'bun run --filter @webdock/cms-instance build',
                            'build': 'bun run build:plugin && bun run build:instance',
                            'check': 'bun run build:sdk && bun run --filter @webdock/extension-managed-cms check',
                            'test': 'bun run build:sdk && bun test --isolate ./extensions/managed-cms/tests ./apps/cms-instance/tests ./packages/cms-bridge/src ./packages/cms-ui/src'},
                'devDependencies': {'typescript': '^5', '@types/bun': '^1.3.0', '@types/node': '^24', '@types/react': '^19', '@types/react-dom': '^19', '@types/pg': '^8.15.5', 'happy-dom': '20.14.5'},
                'overrides': upstream.get('overrides', {}), 'patchedDependencies': patched, 'license': 'AGPL-3.0-only'}
        (staging / 'package.json').write_text(json.dumps(root, indent=2) + '\n')
        (staging / 'bunfig.toml').write_text('[install]\nlinker = "hoisted"\n')
        ignored = 'node_modules\n.next\ndist\n.git\n.env*\n!**/.env.example\n/data\n/private\n/secrets\n/release\n'
        (staging / '.gitignore').write_text(ignored)
        (staging / '.dockerignore').write_text(ignored + '**/node_modules\n**/.next\n**/dist\n**/.env*\n**/*.pem\n**/*.key\n**/*.sqlite\n**/*.db\n')
        (staging / 'extensions/managed-cms/Dockerfile').write_text(dockerfile('plugin'))
        (staging / 'apps/cms-instance/Dockerfile').write_text(dockerfile('instance'))
        (staging / 'README.md').write_text('''# Managed CMS

Independent source workspace for the existing managed-cms TypeScript protocol-v1
plugin and its separate per-project CMS application. This is not a Rust v2 release.

Run `bun install --ignore-scripts` once and commit the generated `bun.lock`.
Thereafter use `bun install --frozen-lockfile --ignore-scripts`, `bun run check`,
`bun run test`, and `bun run build`. No production credentials are needed to build.
Docker contexts are this repository root with the respective component Dockerfile.
Builds must run locally/in verified EU infrastructure, following owner policy.

The plugin and CMS instance are separate images. Generate a plugin manifest with
`bun extensions/managed-cms/manifest.ts <real-image@sha256:digest> <output.json>`.
Publication, registry signatures, image access and installer compatibility must
be verified separately; this export does not publish or activate anything.

Only the eight listed support packages are included. Core, provider credentials,
runtime state and environment files are excluded. The upstream plugin image-smoke
requires the Core installer, and the bridge PostgreSQL test requires the website
migration: those platform tests are deliberately withheld. Database/image/backup
acceptance remains explicit and uses disposable local fixtures only. Existing
per-project CMS database and retained media must never be reset during export.

See component READMEs for runtime contracts. They retain historical Core-relative
operator links; those paths are not part of this independent repository.
''')
        # Catalog paths resolve from the independent repository root.
        catalog = source / 'extensions/managed-cms/catalog'
        if catalog.is_symlink():
            raise ValueError('Catalog directory symlink')
        if catalog.exists():
            for path in sorted(catalog.rglob('*')):
                if path.is_symlink() or (path.is_file() and path.suffix not in {'.md', '.svg', '.png'}):
                    raise ValueError('Unreviewed catalog asset: ' + str(path))
                if path.is_file():
                    copy_source(catalog, staging, path.relative_to(catalog))
        files = {str(path.relative_to(staging)): hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(staging.rglob('*')) if path.is_file()}
        receipt = {'formatVersion': 1, 'plugin': 'managed-cms', 'protocolVersion': 1,
                   'applications': APPLICATIONS, 'supportPackages': PACKAGES,
                   'withheldPlatformTests': sorted(WITHHELD), 'lockfileRequiresGeneration': True,
                   'published': False, 'files': files}
        (staging / 'export-lock.json').write_text(json.dumps(receipt, indent=2) + '\n')
        staging.rename(output)
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    try:
        receipt = export_cms(args.source, args.output)
    except (ValueError, OSError, KeyError, json.JSONDecodeError) as error:
        parser.exit(1, str(error) + '\n')
    print(json.dumps({'exported': str(args.output.absolute()), 'files': len(receipt['files']), 'published': False}))

if __name__ == '__main__':
    main()
