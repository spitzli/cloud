#!/usr/bin/env python3
"""Manual, frozen-source Core checks on the previously qualified EU BuildKit only."""
from __future__ import annotations
import argparse
import hashlib
import importlib.util
import json
import os
import posixpath
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib
import core_tooling

spec=importlib.util.spec_from_file_location('frozen_agent_build',Path(__file__).with_name('agent_build.py'))
frozen=importlib.util.module_from_spec(spec);spec.loader.exec_module(frozen)
require=frozen.require
REQUIRED=frozen.SOURCE_REQUIRED|{'clippy.toml','package.json','bun.lock','bunfig.toml','tools/quality/Cargo.toml','tools/quality/Cargo.lock','tools/quality/src/main.rs','apps/console/package.json','plugins/lumaserv-compute/Cargo.toml','plugins/lumaserv-compute/Cargo.lock','plugins/lumaserv-compute/tests/process.rs','plugins/lumaserv-compute/tests/fixtures/servers-panel.json'}
REQUIRED|={
    'docs/verification/2026-10-25-native-postgresql-backup-transitions.json',
    'docs/superpowers/plans/2026-10-26-postgresql-restore-0056.draft.sql',
}
BUN_IMAGE='docker.io/oven/bun:1.4.3-slim@sha256:14a223fa35747ec728bce300ef85888bc5f7958164d15bbba6a54b22b52a3288'
NODE_IMAGE='docker.io/library/node:24.21.0-bookworm-slim@sha256:d6aa754f16b3197301076f047b5def2f02ea1dbbc2ca920407d46d7ec7f87b20'
DOCKER_CLI_IMAGE='docker.io/library/docker:29.9.2-cli@sha256:5e6baf67325a79f64ed3bed6863954d5e05d0f3d4e57cbdb14809346814bbfdb'
POSTGRES_IMAGE='docker.io/library/postgres:18-bookworm@sha256:5c4783a8cc3f0074912164f94dfa1be4e043438f0f707cf968d7af9a0d31a2c8'
HARNESS_FILES={'core_quality.py','agent_build.py','core_local_fixtures.py','website_fixture_seed.ts','core_tooling.py','core_native_pg.py','Dockerfile'}


CP_CARRY_SOURCE='11d5ab79cb87e88e305408cf30e3c2f45dec4456f1f7b093d2596dd5ef7b0755'
CP_CARRY_MANIFEST_SHA='5f32e3ed0aee2c6d2e989957f5ca2126c8b71242158244f335d472f2780537c8'
CP_CARRY_PROOF_SHA='3f43dbc05c5780467b2915083f88ea122d8b5894b100bf945fe4b7afdb1810f6'
CP_CARRY_ASSESSMENT_SHA='29771c420ad640e981aeed75957129a904ba8bc1132e4323bb2287cccf84e591'
CP_COMPILE_COMMAND=['cargo','test','--locked','-p','spitzli-control-plane','--lib','--bins','--tests','--no-run','--message-format=json','--jobs','3']
CP_CARRY_ALLOWED={'crates/plugin-host/tests/support/local_postgres.rs','tools/plugin-ci/core_quality.py','tools/plugin-ci/core_native_pg.py','tools/plugin-ci/test_core_quality.py','tools/plugin-ci/test_core_native_pg.py','tools/plugin-ci/CORE-QUALITY.md'}


def manifest_sha(entries):return hashlib.sha256(json.dumps(entries,sort_keys=True,separators=(',',':')).encode()).hexdigest()


def control_plane_carry_delta(before,after):
    changes=sorted(p for p in before.keys()|after.keys() if list(before.get(p,[]))!=list(after.get(p,[])))
    require(set(changes)<=CP_CARRY_ALLOWED,'CP compile carry rejected changed application/library/lock/compiler/other fixture input')
    return changes


def validate_control_plane_carry_provenance(carry,command):
    require(carry.get('originalSourceSHA256')==CP_CARRY_SOURCE,'Only the reviewed r3 CP compile may be carried')
    for key,expected in [('compileProofRaw',CP_CARRY_PROOF_SHA),('assessmentRaw',CP_CARRY_ASSESSMENT_SHA)]:
        require(isinstance(carry.get(key),str) and hashlib.sha256(carry[key].encode()).hexdigest()==expected,'CP carry proof substitution rejected')
    require(manifest_sha(carry.get('originalEntries',{}))==CP_CARRY_MANIFEST_SHA,'CP carry original source inventory substitution rejected')
    proof=json.loads(carry['compileProofRaw']);assessment=json.loads(carry['assessmentRaw'])
    require(proof['sourceArchiveSHA256']==CP_CARRY_SOURCE and proof['compileChecksPassed'] and assessment['actualCompileProof']==proof,'Original CP compile did not pass')
    require(not assessment['unmappedLocalArtifacts'] and not assessment['changedFixtureInControlPlaneArtifactClosure'] and len(assessment['localCargoArtifactDepInfo'])==66,'Incomplete CP fixture dependency proof')
    record=next(x for x in proof['checks'] if x['id']=='control-plane-compile')
    require(record['command']==command==CP_COMPILE_COMMAND and record['exitCode']==0,'CP carry command/compiler flags changed')
    for field,log in [('stdoutSHA256','stdoutRaw'),('stderrSHA256','stderrRaw')]:require(isinstance(carry.get(log),str) and hashlib.sha256(carry[log].encode()).hexdigest()==record[field],'CP carry log substitution rejected')
    return record


def validate_control_plane_carry(carry,archive,source_sha,command):
    record=validate_control_plane_carry_provenance(carry,command);frozen.source_hash(archive,source_sha)
    changes=control_plane_carry_delta(carry['originalEntries'],frozen.archive_files(archive))
    return {'originalExecutedSourceSHA256':CP_CARRY_SOURCE,'reexecuted':False,'originalStdoutSHA256':record['stdoutSHA256'],'originalStderrSHA256':record['stderrSHA256'],'exactAllowedSourceDelta':changes,'allCPApplicationLibraryLockCompilerInputsUnchanged':True,'localArtifactDepInfoMappings':66}


def prepare_control_plane_carry(archive,source_sha,original_archive,evidence):
    frozen.source_hash(original_archive,CP_CARRY_SOURCE)
    carry={'originalSourceSHA256':CP_CARRY_SOURCE,'originalEntries':frozen.archive_files(original_archive)}
    for key,name in [('compileProofRaw','native-pg-compile-proof.json'),('assessmentRaw','compile-carry-assessment.json'),('stdoutRaw','control-plane-compile.stdout'),('stderrRaw','control-plane-compile.stderr.log')]:carry[key]=(evidence/name).read_bytes().decode()
    carry['verifiedInputProof']=validate_control_plane_carry(carry,archive,source_sha,CP_COMPILE_COMMAND)
    return carry


def build_output_path(path):
    p=PurePosixPath(path);name=p.name.lower()
    return bool(re.fullmatch(r'core(?:\.[0-9]+)?',name) or (name.endswith('.zip') and ('public' in p.parts or re.fullmatch(r'(?:hosting[-_]agent|webdock[-_](?:native[-_])?agent|native[-_]agent|build[-_]runner)(?:[-_.].*)?\.zip',name))))


def select_full_source(archive):
    """Use the frozen extractor's path/link/secret checks, retaining all safe source roots."""
    roots=set();files=set()
    for m in archive.getmembers():
        p=PurePosixPath(m.name)
        require(p.parts and not p.is_absolute() and '..' not in p.parts,'Unsafe frozen source path')
        name=p.name.lower()
        require(m.isdir() or not build_output_path(p),'Build output forbidden: '+str(p))
        require(not (name.startswith('.env') and name!='.env.example') and not name.endswith(('.env','.private.json')) and not any(x in name for x in ['kubeconfig','credentials.json','secrets.json']) and name not in {'.npmrc','.netrc'},'Ambient/private configuration forbidden: '+str(p))
        roots.add(p.parts[0]);files.add(str(p))
    require(REQUIRED<=files,'Frozen source lacks required quality/frontend/compute closure')
    frozen.SOURCE_ROOTS=roots
    frozen.SOURCE_ROOT_FILES=files
    selected=list(frozen.members(archive))
    selected_names={member.name for member,_ in selected}
    def local_dependencies(table):
        for key,value in table.items():
            if not isinstance(value,dict):continue
            if key in {'dependencies','dev-dependencies','build-dependencies'}:
                for dep in value.values():
                    if isinstance(dep,dict) and 'path' in dep:yield dep['path']
            else:yield from local_dependencies(value)
    for member,resolved in selected:
        parts=PurePosixPath(member.name).parts
        if parts[-1]!='Cargo.toml' or not (len(parts)==1 or parts[0] in {'crates','plugins'} or member.name=='tools/quality/Cargo.toml'):continue
        table=tomllib.loads(archive.extractfile(resolved).read().decode())
        for path in local_dependencies(table):
            require(isinstance(path,str) and not PurePosixPath(path).is_absolute(),'Local Cargo dependency must stay inside frozen source')
            target=posixpath.normpath(str(PurePosixPath(member.name).parent/path/'Cargo.toml'))
            require(target in selected_names,'Missing frozen local Cargo dependency: '+member.name+' -> '+target)
    return selected


def unpack(archive,digest,output):
    frozen.source_hash(archive,digest)
    with tarfile.open(archive) as tar:select_full_source(tar)
    frozen.unpack(archive,digest,output)


def prepare(archive,digest,output,review_ref,reuse_evidence=None,reuse_archive=None):
    frozen.source_hash(archive,digest)
    require(review_ref and review_ref not in {'HEAD','main','latest','WIP'},'Frozen reviewed source reference required')
    require(not output.exists() and not output.is_symlink(),'New isolated context required')
    with tarfile.open(archive) as tar:selected=select_full_source(tar)
    require((reuse_evidence is None)==(reuse_archive is None),'Both reviewed CP carry evidence and original source archive required')
    carry=prepare_control_plane_carry(archive,digest,reuse_archive,reuse_evidence) if reuse_evidence is not None else None
    output.parent.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.core-quality-',dir=output.parent) as td:
        stage=Path(td)/'context';stage.mkdir(mode=0o700)
        shutil.copyfile(archive,stage/'source.tar')
        here=Path(__file__).parent
        for name in sorted(HARNESS_FILES-{'Dockerfile'}):shutil.copyfile(here/name,stage/name)
        shutil.copyfile(here/'core-quality.Dockerfile',stage/'Dockerfile')
        proof={'sourceArchiveSHA256':digest,'sourceArchiveBytes':archive.stat().st_size,'frozenSourceRef':review_ref,'sourceFileCount':len(selected),'builderImage':frozen.BASE_IMAGE,'bunImage':BUN_IMAGE,'nodeImage':NODE_IMAGE,'dockerCLIImage':DOCKER_CLI_IMAGE,'postgresFixtureImage':POSTGRES_IMAGE,'cargoJobs':3,'testThreads':3,'guestUID':1001,'manualEUQualificationOnly':True,'githubWorkflowTriggered':False,'productionActivationPerformed':False,'harnessSHA256':{n:hashlib.sha256((stage/n).read_bytes()).hexdigest() for n in sorted(HARNESS_FILES)}}
        proof['controlPlaneCompileCarry']=carry
        (stage/'checkpoint.json').write_text(json.dumps(proof,indent=2)+'\n');stage.rename(output)
    return {**proof,'controlPlaneCompileCarry':carry['verifiedInputProof'] if carry else None}


def plan(mode):
    focused=[
        {'id':'waitlist-access','args':['cargo','test','--locked','-p','spitzli-access','--lib','waitlist'],'requiredTests':['public_input_cannot_set_status_or_skip_consent','public_contact_names_reject_controls_before_database_intake']},
        {'id':'waitlist-http','args':['cargo','test','--locked','-p','spitzli-control-plane','--lib','waitlist'],'requiredTests':['public_intake_requires_json_with_explicit_consent_and_no_admission_fields','waitlist_writes_reject_foreign_or_missing_browser_origins','public_intake_bounds_untrusted_request_bodies_before_service_access']},
        {'id':'compute-process','args':['cargo','test','--manifest-path','plugins/lumaserv-compute/Cargo.toml','--locked','--test','process'],'requiredTests':['standalone_private_rpc_scoped_https_redaction_and_revocation']},
        {'id':'hosting-repository-projection','args':['cargo','test','--locked','-p','spitzli-build','--lib','repository_projection_exposes_the_authoritative_binding_not_metadata_ids'],'requiredTests':['repository_projection_exposes_the_authoritative_binding_not_metadata_ids']},
    ]
    if mode=='focused':return focused
    if mode=='consumer':return [{'id':'compute-console-consumer','args':['bun','--no-env-file','test','plugins/lumaserv-compute/tests/ui.test.ts'],'testFormat':'bun','requiredTests':['the exact compute subprocess panel conforms to the current Console contract']}]
    require(mode=='full','Explicit focused/full/consumer mode required')
    return [
        {'id':'german-catalog-check','args':['python3','packages/i18n/scripts/compile.py','--check'],'requiredTests':None},
        {'id':'quality-checker-tests','args':['cargo','test','--manifest-path','tools/quality/Cargo.toml','--locked'],'requiredTests':[]},
        {'id':'full-quality','args':['bun','run','quality'],'requiredTests':None},
        {'id':'root-build','args':['npm','run','build'],'requiredTests':None},
        {'id':'compose-distribution','args':['python3','tools/release/check-compose.py'],'requiredTests':None},
        {'id':'helm-distribution','args':['python3','tools/release/check-charts.py'],'requiredTests':None},
        {'id':'native-probes','args':['cargo','test','--locked','-p','spitzli-control-plane','--test','probes'],'requiredTests':[]},
    ]


def test_counts(raw,item):
    if item.get('testFormat')=='bun':
        passed=re.search(r'\b(\d+) pass\b',raw);failed=re.search(r'\b(\d+) fail\b',raw)
        require(passed and failed and int(passed[1])>0 and int(failed[1])==0,'Nonzero successful Bun tests required')
        for name in item['requiredTests']:require(name in raw,'Missing successful consumer contract test')
        return {'passed':int(passed[1]),'failed':0,'ignored':0}
    matches=re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;',raw)
    require(matches and sum(int(x[0]) for x in matches)>0 and all(x[1]=='0' for x in matches),'Nonzero passed tests required: '+item['id'])
    for name in item['requiredTests'] or []:require(re.search(re.escape(name)+r'\s+\.\.\. ok',raw),'Missing successful regression: '+name)
    return {'passed':sum(int(x[0]) for x in matches),'failed':0,'ignored':sum(int(x[2]) for x in matches)}


def run_checks(mode,output,checkpoint):
    require(os.getuid()==1001,'Isolated build checks require nonroot guest UID1001')
    require(not output.exists(),'Never replace prior check output')
    output.mkdir(mode=0o700)
    env={k:v for k,v in os.environ.items() if k in {'PATH','HOME','USER','LANG','CI','TMPDIR','CARGO_HOME','RUSTUP_HOME','CARGO_TARGET_DIR','CARGO_BUILD_JOBS','RUST_TEST_THREADS','PLAYWRIGHT_BROWSERS_PATH','NODE_OPTIONS'}}
    tooling=core_tooling.verify(output,compose=mode=='full')
    env['DOCKER_HOST']=core_tooling.NO_DAEMON
    env['WEBDOCK_COMPUTE_PANEL_EVIDENCE']=str(output/'compute-servers-panel.json')
    records=[]
    for item in plan(mode):
        if item['id']=='root-build':require(not Path('package-lock.json').exists(),'Root npm install/package-lock is outside the build-only gate')
        log=output/(item['id']+'.log')
        with log.open('xb') as out:
            try:result=subprocess.run(item['args'],env=env,stdout=out,stderr=subprocess.STDOUT,timeout=10800)
            except subprocess.TimeoutExpired:result=subprocess.CompletedProcess(item['args'],124)
        log.chmod(0o600);raw=log.read_text(errors='replace')
        record={**item,'exitCode':result.returncode,'logSHA256':hashlib.sha256(log.read_bytes()).hexdigest()}
        if result.returncode==0 and item['requiredTests'] is not None:record['testCounts']=test_counts(raw,item)
        if item['id']=='root-build':
            require(not Path('package-lock.json').exists(),'Root npm run build unexpectedly created package-lock.json')
            record['rootPackageLockGenerated']=False
            record['npmInstallInvoked']=False
        records.append(record)
        (output/'checks.json').write_text(json.dumps(records,indent=2)+'\n')
        if result.returncode:
            print(raw[-131072:],flush=True)
            raise ValueError('Manual EU source check failed: '+item['id'])
    for name,args in [('rustc-version.txt',['rustc','--version','--verbose']),('bun-version.txt',['bun','--version']),('builder-libc.txt',['getconf','GNU_LIBC_VERSION']),('builder-packages.txt',['dpkg-query','-W'])]:
        (output/name).write_bytes(subprocess.check_output(args,env=env,timeout=30))
    panel=output/'compute-servers-panel.json'
    if panel.exists():require(json.loads(panel.read_text())==json.loads(Path('plugins/lumaserv-compute/tests/fixtures/servers-panel.json').read_text()),'Real compute panel output differs from consumer fixture')
    proof={**json.loads(checkpoint.read_text()),'mode':mode,'checks':records,'tooling':tooling,'computePanelEvidenceExported':panel.exists(),'allPlannedChecksPassed':True,'liveServiceCredentialsUsed':False,'productionActivationPerformed':False}
    (output/'core-quality-proof.json').write_text(json.dumps(proof,indent=2)+'\n')
    return proof


def build(context,digest,mode,output):
    context=context.resolve();output=output.absolute().resolve()
    checkpoint=json.loads((context/'checkpoint.json').read_text());frozen.source_hash(context/'source.tar',digest)
    require(checkpoint['sourceArchiveSHA256']==digest,'Frozen checkpoint mismatch')
    require(mode in {'focused','full','consumer','migration','website','native-pg','recovery'} and not output.exists() and not output.is_relative_to(context),'New private output and explicit check mode required')
    for name,h in checkpoint['harnessSHA256'].items():require(name in HARNESS_FILES and hashlib.sha256((context/name).read_bytes()).hexdigest()==h,'Recipe changed after preparation')
    client,address=frozen.qualified_client();home=Path(os.environ['HOME'])
    require(home.resolve()==home and home.stat().st_uid==1001,'Owned canonical BUILD HOME required')
    output.parent.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='core-quality-',dir=home) as td:
        work=Path(td);auth=work/'auth';auth.mkdir(mode=0o700);(auth/'config.json').write_text('{"auths":{}}\n');(auth/'config.json').chmod(0o600)
        env={k:os.environ[k] for k in ['HOME','PATH','USER','LANG','XDG_RUNTIME_DIR'] if k in os.environ};env['DOCKER_CONFIG']=str(auth)
        args=[client,'--addr',address,'build','--progress=plain','--frontend=dockerfile.v0','--local','context='+str(context),'--local','dockerfile='+str(context),'--opt','filename=Dockerfile','--opt','target='+mode+'-artifact','--opt','platform=linux/amd64','--opt','build-arg:SOURCE_SHA256='+digest,'--output','type=local,dest='+str(output)]
        log=work/'build.log'
        with log.open('wb') as out:
            try:result=subprocess.run(args,env=env,stdout=out,stderr=subprocess.STDOUT,timeout=21600)
            except subprocess.TimeoutExpired:result=subprocess.CompletedProcess(args,124)
        if result.returncode:
            failure=home/'core-quality-failures';require(not failure.is_symlink(),'Private diagnostic directory cannot be a symlink');failure.mkdir(mode=0o700,exist_ok=True)
            require(failure.stat().st_uid==1001 and failure.stat().st_mode&0o077==0,'Owned private diagnostic directory required')
            fd,name=tempfile.mkstemp(prefix=mode+'-',suffix='.log',dir=failure)
            with os.fdopen(fd,'wb') as out,log.open('rb') as source:source.seek(max(0,log.stat().st_size-2*1024*1024));out.write(source.read(2*1024*1024))
            for old in sorted(failure.glob('*.log'),key=lambda p:p.stat().st_mtime,reverse=True)[4:]:
                require(not old.is_symlink() and old.stat().st_uid==1001 and old.stat().st_nlink==1,'Unexpected diagnostic entry')
                old.unlink()
            raise ValueError('Manual EU quality failed; private diagnostic '+name)
    proof=json.loads((output/'core-quality-proof.json').read_text())
    require(proof['sourceArchiveSHA256']==digest and proof['mode']==mode and (proof.get('fixturePassed') and proof.get('ownedDatabaseProcessStoppedAndRootRemoved') if mode in {'website','migration','native-pg','recovery'} else proof.get('allPlannedChecksPassed')),'Exact source/mode successful proof required')
    return {'sourceArchiveSHA256':digest,'mode':mode,'passed':True,'proof':str(output/'core-quality-proof.json'),'githubWorkflowTriggered':False,'productionActivationPerformed':False}


def main():
    p=argparse.ArgumentParser(description=__doc__);s=p.add_subparsers(dest='command',required=True)
    for name in ['prepare','unpack']:
        c=s.add_parser(name);c.add_argument('--archive',type=Path,required=True);c.add_argument('--source-sha256',required=True);c.add_argument('--output',type=Path,required=True)
        if name=='prepare':
            c.add_argument('--review-ref',required=True);c.add_argument('--reuse-control-plane-evidence',type=Path);c.add_argument('--reuse-control-plane-source-archive',type=Path)
    c=s.add_parser('run');c.add_argument('--mode',choices=['focused','full','consumer','migration','website','native-pg','recovery'],required=True);c.add_argument('--output',type=Path,required=True);c.add_argument('--checkpoint',type=Path,required=True)
    c=s.add_parser('build');c.add_argument('--context',type=Path,required=True);c.add_argument('--source-sha256',required=True);c.add_argument('--mode',choices=['focused','full','consumer','migration','website','native-pg','recovery'],required=True);c.add_argument('--output',type=Path,required=True)
    a=p.parse_args()
    if a.command=='prepare':r=prepare(a.archive,a.source_sha256,a.output,a.review_ref,a.reuse_control_plane_evidence,a.reuse_control_plane_source_archive)
    elif a.command=='unpack':unpack(a.archive,a.source_sha256,a.output);r={'unpacked':True}
    elif a.command=='run':r=run_checks(a.mode,a.output,a.checkpoint)
    else:r=build(a.context,a.source_sha256,a.mode,a.output)
    print(json.dumps(r))

if __name__=='__main__':
    try:main()
    except Exception as e:raise SystemExit(str(e)) from None
