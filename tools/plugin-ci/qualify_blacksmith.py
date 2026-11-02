#!/usr/bin/env python3
"""Maintainer-only managed-builder probe; no registry push or source release."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tempfile

SPEC=importlib.util.spec_from_file_location('build_oci',Path(__file__).resolve().parents[1]/'plugin-release/build_oci.py')
BUILD=importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)
BASE='docker.io/library/debian:trixie-slim@sha256:f6b08b09a05725f9431502d3b5714f5c4576e82c1a84fa1debe0c24a7e0c3ec4'


def verify_oci_export(archive, metadata):
    BUILD.require(archive.is_file() and not archive.is_symlink() and 0 < archive.stat().st_size <= 128*1024*1024,'Bounded qualification OCI export required')
    BUILD.require(metadata.is_file() and not metadata.is_symlink() and metadata.stat().st_size <= 128*1024,'Bounded qualification digest metadata required')
    digest=BUILD.metadata_digest(json.loads(metadata.read_text()))
    with tarfile.open(archive) as source:
        def read(name):
            members=[member for member in source.getmembers() if member.name==name]
            BUILD.require(len(members)==1 and members[0].isfile() and members[0].size <= 128*1024,'One bounded OCI metadata file required')
            return source.extractfile(members[0]).read(128*1024+1)
        index=json.loads(read('index.json'))
        manifests=index.get('manifests',[])
        BUILD.require(index.get('schemaVersion')==2 and len(manifests)==1 and manifests[0].get('digest')==digest,'OCI index must match actual exported image digest')
        raw=read('blobs/sha256/'+digest.removeprefix('sha256:'))
        BUILD.require('sha256:'+hashlib.sha256(raw).hexdigest()==digest,'Exported OCI manifest digest mismatch')
    return digest


def qualify(config):
    docker,builder,provider_config=BUILD.qualified_blacksmith_builder(config)
    revision=config.get('GITHUB_SHA','')
    BUILD.require(re.fullmatch(r'[a-f0-9]{40}',revision) and config.get('GITHUB_REPOSITORY')=='spitzli/cloud','Canonical qualification source required')
    root=Path(config['RUNNER_TEMP']).resolve()
    with tempfile.TemporaryDirectory(prefix='blacksmith-qualification-',dir=root) as temporary:
        private=Path(temporary);context=private/'context';context.mkdir()
        (context/'Dockerfile').write_text('FROM '+BASE+' AS check\nRUN printf "blacksmith-check-ok\\n" > /qualification\nFROM check AS backend\nCMD ["/bin/cat","/qualification"]\n')
        BUILD.build_image(context,root,{**config,'IMAGE_REPOSITORY':'ghcr.io/spitzli/cloud'},'',check=True)
        archive=private/'probe.oci.tar';metadata=private/'metadata.json'
        environment={name:config[name] for name in ['PATH','HOME','USER','LANG','LC_ALL','XDG_RUNTIME_DIR'] if name in config}
        environment.update(DOCKER_CONFIG=provider_config,BUILDX_CONFIG=str(Path(provider_config)/'buildx'))
        arguments=[docker,'buildx','build','--builder',builder,'--progress=plain','--file',str(context/'Dockerfile'),
                   '--platform','linux/amd64','--target','backend','--no-cache','--provenance=false','--sbom=false',
                   '--output','type=oci,dest='+str(archive),'--metadata-file',str(metadata),str(context)]
        log=private/'export.log'
        with open(log,'xb',opener=lambda name,flags:os.open(name,flags,0o600)) as output:
            result=subprocess.run(arguments,env=environment,stdout=output,stderr=subprocess.STDOUT,timeout=600)
        if result.returncode:
            raise BUILD.BuildFailed(BUILD.retain_failure_log(log,config['HOME'],[]))
        digest=verify_oci_export(archive,metadata)
        receipt={'schemaVersion':1,'scope':'maintainer qualification probe only; no plugin source-check claim',
                 'repository':config['GITHUB_REPOSITORY'],'sourceSha':revision,'observedRegion':config['BLACKSMITH_REGION'],
                 'runnerName':config.get('RUNNER_NAME'),'builder':builder,'driver':'remote','endpoint':'tcp://127.0.0.1:1234',
                 'checkTargetPassed':True,'imageDigest':digest,'archiveSha256':hashlib.file_digest(archive.open('rb'),'sha256').hexdigest(),
                 'archiveBytes':archive.stat().st_size,'pushPerformed':False}
        run_id=config.get('GITHUB_RUN_ID','')
        BUILD.require(re.fullmatch(r'[0-9]+',run_id),'Actual qualification run ID required')
        receipt['runUrl']='https://github.com/spitzli/cloud/actions/runs/'+run_id
        with (root/'blacksmith-qualification.json').open('x') as output:
            json.dump(receipt,output,indent=2);output.write('\n')
    return receipt


if __name__=='__main__':
    try:
        print(json.dumps(qualify(os.environ),sort_keys=True))
    except Exception:
        raise SystemExit('Blacksmith qualification failed; inspect the private runner diagnostics. No release was published.') from None
