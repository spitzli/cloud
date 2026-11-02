#!/bin/sh
set -eu
export PIP_DISABLE_PIP_VERSION_CHECK=1 PIP_NO_CACHE_DIR=1 DEBIAN_FRONTEND=noninteractive
case "$1" in
    download)
        python3 -m venv /tmp/download-venv
        /tmp/download-venv/bin/python -m pip --version > /evidence/pip-version.txt
        mkdir /evidence/downloads
        # No mutable pip/wheel upgrade; build tooling comes from authenticated Debian packages.
        PYTHONPATH=/usr/lib/python3/dist-packages /tmp/download-venv/bin/python -m pip download --require-hashes --no-build-isolation \
            --index-url https://pypi.org/simple -r /src/containers/sidecar-requirements.txt \
            --dest /evidence/downloads
        cd /evidence/downloads
        sha256sum * > /evidence/downloads.sha256
        ;;
    build)
        # This entire container has --network none. All build tools are the exact
        # packages independently verified during preparation, including venv's pip seed.
        sh /recipe/offline-os.sh
        cd /evidence/downloads
        sha256sum -c /evidence/downloads.sha256
        python3 -m venv /venv
        mkdir /evidence/wheels
        PYTHONPATH=/usr/lib/python3/dist-packages /venv/bin/python -m pip wheel \
            --no-index --no-deps --no-build-isolation --wheel-dir /evidence/wheels /evidence/downloads/*
        python3 - <<'PYCODE'
import hashlib,pathlib
p=pathlib.Path('/evidence')
lines=[]
for wheel in sorted((p/'wheels').glob('*.whl')):
    lines.append(str(wheel)+' --hash=sha256:'+hashlib.sha256(wheel.read_bytes()).hexdigest())
(p/'wheels.lock').write_text('\n'.join(lines)+'\n')
PYCODE
        /venv/bin/python -m pip install --no-index --no-deps --require-hashes -r /evidence/wheels.lock
        /venv/bin/python -m pip check > /evidence/pip-check.txt
        /venv/bin/python - <<'PYCODE'
import hashlib,importlib.metadata,json,pathlib
p=pathlib.Path('/evidence'); components=[]
for dist in importlib.metadata.distributions():
    files=[]
    for f in dist.files or []:
        path=pathlib.Path(dist.locate_file(f))
        if path.is_file():
            files.append({'file':str(f),'sha256':hashlib.sha256(path.read_bytes()).hexdigest()})
    components.append({'type':'library','name':dist.metadata['Name'],'version':dist.version,
        'purl':'pkg:pypi/'+dist.metadata['Name'].lower().replace('_','-')+'@'+dist.version,
        'properties':[{'name':'webdock:installed-files-sha256','value':json.dumps(files,sort_keys=True)}]})
(p/'python-sbom.json').write_text(json.dumps(components,indent=2)+'\n')
PYCODE
        # docker cp preserves the absolute /venv/bin/python3 symlink for the identical pinned Debian runtime.
        cp -a /venv /evidence/venv
        ;;
    *) exit 2 ;;
esac
