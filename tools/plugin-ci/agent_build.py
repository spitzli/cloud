#!/usr/bin/env python3
"""Package/build a parent-reviewed frozen Core archive without Git or production operations."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
import posixpath
from pathlib import Path,PurePosixPath
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile

BASE_IMAGE='docker.io/library/rust:1.98.1-slim-bookworm@sha256:ff521445a372125ed4f76e1453a1f8098f2d05332d1601d30db1c1f62757e730'
BUILDCTL_SHA256='8e64362dceba989827b23af567915feda10426ebd40611772318662e33bf2bfd'
SOURCE_REQUIRED={'Cargo.toml','Cargo.lock','rust-toolchain.toml','crates/agent/Cargo.toml','crates/agent/src/main.rs'}
REQUIRED_TESTS={
    'storage-attachment-tests.log':['cross_node_apply_and_stop_prove_attachment_without_altering_objects','local_storage_verification_is_not_rebound_to_a_foreign_node','foreign_owner_uid_capacity_path_mode_or_affinity_cannot_be_reused','no_expected_uid_storage_lifecycle_extra_sources_subpaths_or_unready_node','deleting_missing_or_nonstatic_objects_cannot_be_reused','proof_detects_object_and_physical_node_changes_but_not_normal_node_heartbeat'],
    'management-tests.log':['remote_authorization_latency_cannot_starve_a_live_runtime_read','periodic_revocation_cancels_pending_runtime_reads_without_releasing_output','fresh_final_guard_rejects_output_when_authority_changes_at_read_completion'],
    'full-agent-tests.log':[],
}
PREFLIGHT_STORAGE_TESTS=tuple(REQUIRED_TESTS['storage-attachment-tests.log'])
REQUIRED_TESTS['storage-attachment-tests.log']+=['privileged_initialization_or_unproved_public_routing_is_rejected_before_reuse','service_account_credentials_cannot_survive_a_retained_update']
FORBIDDEN={'.git','.runtime-test','node_modules','target','.next','.cache','private','secrets','__pycache__'}
SOURCE_ROOTS={'crates','proto','migrations','agents','deploy','infra','vendor','.cargo'}
SOURCE_ROOT_FILES=SOURCE_REQUIRED|{'LICENSE','COMMERCIAL-LICENSING.md','NOTICE','tools/migrations/baseline.sha256'}
SOURCE_PREFIXES=('legacy/control-plane/drizzle/','tools/tests/')
MAX_ARCHIVE=256*1024*1024
MAX_SOURCE=512*1024*1024
SECRET=re.compile(rb'-----BEGIN (?:RSA |EC |OPENSSH |ENCRYPTED )?PRIVATE KEY-----[\r\n]+[A-Za-z0-9+/=\r\n]{40,}-----END (?:RSA |EC |OPENSSH |ENCRYPTED )?PRIVATE KEY-----|github_pat_[A-Za-z0-9_]{20,}|gh[pousr]_[A-Za-z0-9]{30,}|AKIA[A-Z0-9]{16}')


def require(condition,message):
    if not condition:raise ValueError(message)


def source_hash(path,expected):
    require(re.fullmatch(r'[a-f0-9]{64}',expected or '') and not re.fullmatch(r'([a-f0-9])\1{63}',expected),'Exact frozen archive SHA256 required')
    require(path.is_file() and not path.is_symlink() and path.stat().st_size<=MAX_ARCHIVE,'Bounded regular frozen archive required')
    with path.open('rb') as source:actual=hashlib.file_digest(source,'sha256').hexdigest()
    require(actual==expected,'Frozen source archive checksum mismatch')
    return actual


def members(archive):
    found=set();total=0;index={}
    for member in archive.getmembers():
        name=PurePosixPath(member.name)
        if str(name)=='.' and member.isdir():continue
        require(not name.is_absolute() and '..' not in name.parts and '\\' not in member.name,'Source archive path escape')
        require(not (set(name.parts)&{'.git','.runtime-test','node_modules','target','.next','.cache','__pycache__'}) and name.parts[0] not in {'private','secrets'},'Runtime/private/build state cannot enter a frozen source archive')
        if member.issym():
            require(not PurePosixPath(member.linkname).is_absolute(),'External source link forbidden')
            destination=posixpath.normpath(str(name.parent/member.linkname))
            require(destination!='..' and not destination.startswith('../'),'External source link forbidden')
        if str(name) not in SOURCE_ROOT_FILES and name.parts[0] not in SOURCE_ROOTS and not str(name).startswith(SOURCE_PREFIXES):
            continue
        if name.name=='.env.example':
            continue
        require(not member.islnk() and (member.isdir() or member.isfile() or member.issym()),'Only regular source files/directories and internal source links allowed')
        require(not name.name.startswith('.env') and name.suffix.lower() not in {'.pem','.key','.p12','.pfx','.db','.sqlite','.dump'},'Credential/database material forbidden: '+str(name))
        if member.isdir():continue
        require(str(name) not in index,'Duplicate source archive entry')
        index[str(name)]=member
    def resolve_member(name,seen):
        require(name in index and name not in seen and len(seen)<32,'Invalid/cyclic internal source link')
        member=index[name]
        if not member.issym():return member
        require(not PurePosixPath(member.linkname).is_absolute() and '\\' not in member.linkname,'External source link forbidden')
        target=posixpath.normpath(str(PurePosixPath(name).parent/member.linkname))
        require(target!='..' and not target.startswith('../'),'Source link escaped archive root')
        return resolve_member(target,seen|{name})
    for name,member in index.items():
        resolved=resolve_member(name,set())
        require(resolved.isfile() and resolved.size<=16*1024*1024,'Oversized/nonregular source file')
        total+=resolved.size;require(total<=MAX_SOURCE,'Expanded source exceeds budget')
        found.add(name);raw=archive.extractfile(resolved).read()
        require(not SECRET.search(raw),'Secret-shaped material in source archive: '+name)
        yield member,resolved
    require(SOURCE_REQUIRED<=found,'Frozen Core archive is missing required source/lock files')


def unpack(path,expected,output):
    source_hash(path,expected)
    output=output.resolve();output.mkdir(parents=True,exist_ok=True)
    with tarfile.open(path,'r:*') as archive:
        selected=list(members(archive))
        for member,resolved in selected:
            target=output/str(PurePosixPath(member.name))
            target.parent.mkdir(parents=True,exist_ok=True)
            require(target.resolve().is_relative_to(output),'Extraction escaped source root')
            with archive.extractfile(resolved) as source,target.open('xb') as destination:shutil.copyfileobj(source,destination)
            target.chmod(0o755 if resolved.mode&0o111 else 0o644)


def archive_files(path):
    with tarfile.open(path,'r:*') as archive:
        return {member.name:(member.type.decode('ascii'),member.mode,member.linkname,hashlib.sha256(archive.extractfile(member).read()).hexdigest() if member.isfile() else '') for member in archive.getmembers() if not member.isdir()}


def fixture_only_change(previous,current):
    old,new=archive_files(previous),archive_files(current)
    changed=sorted(name for name in old.keys()|new.keys() if old.get(name)!=new.get(name))
    require(changed==['crates/agent/tests/legacy_attachment.rs'],'Software-proof reuse requires exactly the reviewed fixture-only change')
    return {'changedFiles':changed,'allOtherSourceFilesUnchanged':True,'unchangedSourceFileCount':len(old)-1}


def prepare(path,expected,output,review_ref,mode='preflight',previous_archive=None,previous_sha=None,software_proof=None):
    source_hash(path,expected)
    require(mode in {'preflight','final','fixture-refresh','fixture-only'},'Explicit source verification mode required')
    require(review_ref and review_ref not in {'HEAD','main','latest','WIP'},'Parent frozen checkpoint/review reference required')
    require(not output.exists() and not output.is_symlink(),'New candidate context required; existing data is never overwritten')
    with tarfile.open(path,'r:*') as archive:list(members(archive))
    reused=None
    if mode in {'fixture-refresh','fixture-only'}:
        require(previous_archive is not None and software_proof is not None,'Previously accepted frozen source/proof required for fixture refresh')
        source_hash(previous_archive,previous_sha)
        prior=json.loads(software_proof.read_text())
        require(prior['sourceArchiveSHA256']==previous_sha and prior['testTranscripts']['full-agent-tests.log']['passed']>0,'Previous complete software proof required')
        reused={'sourceRef':prior['sourceRef'],'artifactSHA256':prior['artifactSHA256'],'testTranscripts':prior['testTranscripts'],'sourceComparison':fixture_only_change(previous_archive,path)}
    output.parent.mkdir(parents=True,exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.agent-context-',dir=output.parent) as temporary:
        stage=Path(temporary)/'context';stage.mkdir(mode=0o700)
        shutil.copyfile(path,stage/'source.tar')
        here=Path(__file__).resolve().parent
        for name in ['agent_build.py','agent_elf.py']:shutil.copyfile(here/name,stage/name)
        shutil.copyfile(here/'agent-candidate.Dockerfile',stage/'Dockerfile')
        hashes={name:hashlib.sha256((stage/name).read_bytes()).hexdigest() for name in ['agent_build.py','agent_elf.py','Dockerfile']}
        checkpoint={'schemaVersion':1,'sourceRef':'sha256:'+expected,'sourceArchiveSHA256':expected,'sourceRootsSelected':sorted(SOURCE_ROOTS),'sourceAdditionalPathsSelected':list(SOURCE_PREFIXES)+['tools/migrations/baseline.sha256'],'parentCheckpointRef':review_ref,'buildMode':mode,'promotable':False,'independentReviewRequiredBeforeActivation':True,'builderImage':BASE_IMAGE,'features':['kubernetes'],'defaultFeatures':False,'cargoJobs':3,'testExecutionUID':65534,'harnessSHA256':hashes,'productionActivationPerformed':False}
        checkpoint['reusedSoftwareProof']=reused
        (stage/'checkpoint.json').write_text(json.dumps(checkpoint,indent=2)+'\n')
        stage.rename(output)
    return checkpoint


def validate_transcripts(output,mode='final'):
    require(mode in {'preflight','final','fixture-refresh','fixture-only'},'Known test mode required')
    if mode in {'fixture-refresh','fixture-only'}:
        proof=json.loads((output/'checkpoint.json').read_text())['reusedSoftwareProof']
        require(proof and proof['sourceComparison']['allOtherSourceFilesUnchanged'],'Reviewed fixture-only comparison required')
        require((output/'legacy-attachment-test').is_file(),'Refreshed integration test binary missing')
        if mode=='fixture-only':require(not (output/'spitzli-agent').exists() and not (output/'release-build.log').exists(),'Fixture-only compilation must not produce a release binary')
        return {name:{**value,'reusedFromSourceRef':proof['sourceRef']} for name,value in proof['testTranscripts'].items()}
    required_files={'agent-lib-tests.log':PREFLIGHT_STORAGE_TESTS} if mode=='preflight' else REQUIRED_TESTS
    counts={}
    for name,required in required_files.items():
        raw=(output/name).read_text()
        summaries=re.findall(r'test result: ok\. (\d+) passed; (\d+) failed; (\d+) ignored;',raw)
        require(summaries and sum(int(value[0]) for value in summaries)>0 and all(value[1]=='0' for value in summaries),'Nonzero successful test transcript required: '+name)
        for test in required:
            require(re.search(re.escape(test)+r'\s+\.\.\. ok',raw),'Required regression test did not pass: '+test)
        counts[name]={'passed':sum(int(value[0]) for value in summaries),'ignored':sum(int(value[2]) for value in summaries),'sha256':hashlib.sha256(raw.encode()).hexdigest()}
    return counts


def export_integration(messages,output):
    matches=[]
    for line in messages.read_text().splitlines():
        try:item=json.loads(line)
        except json.JSONDecodeError:continue
        if item.get('reason')=='compiler-artifact' and item.get('target',{}).get('name')=='legacy_attachment' and item.get('profile',{}).get('test') and item.get('executable'):
            matches.append(Path(item['executable']))
    require(len(matches)==1,'Unique compiled legacy_attachment test executable required')
    source=matches[0].resolve()
    require(source.is_relative_to('/agent-target') and source.is_file(),'Integration test executable must come from the private target cache')
    target=output/'legacy-attachment-test';shutil.copyfile(source,target);target.chmod(0o555)
    return {'binary':target.name,'sha256':hashlib.sha256(target.read_bytes()).hexdigest(),'liveKubernetesFixtureExecuted':False}


def qualified_client():
    require(os.getuid()==1001,'Build must run as qualified BUILD UID1001')
    binary=shutil.which('buildctl');require(binary,'Qualified buildctl must be preinstalled')
    binary=Path(binary).resolve()
    require(hashlib.sha256(binary.read_bytes()).hexdigest()==BUILDCTL_SHA256,'Qualified BuildKit client checksum mismatch')
    address='unix:///run/user/1001/spitzli-buildkit.sock';path=Path(address.removeprefix('unix://'))
    info=path.stat();require(path.resolve()==path and stat.S_ISSOCK(info.st_mode) and info.st_uid==1001,'Qualified rootless socket required')
    return str(binary),address


def build(context,expected,output):
    checkpoint=json.loads((context/'checkpoint.json').read_text())
    source_hash(context/'source.tar',expected)
    require(checkpoint['sourceArchiveSHA256']==expected and checkpoint['sourceRef']=='sha256:'+expected,'Checkpoint identity mismatch')
    for name,digest in checkpoint['harnessSHA256'].items():
        require(name in {'Dockerfile','agent_build.py','agent_elf.py'} and hashlib.sha256((context/name).read_bytes()).hexdigest()==digest,'Candidate recipe changed after preparation')
    require(not output.exists() and not output.is_relative_to(context),'New private artifact output outside source context required')
    binary,address=qualified_client();output.parent.mkdir(parents=True,exist_ok=True)
    output.mkdir(mode=0o700)
    home=Path(os.environ['HOME']);logs=home/'agent-build-failures'
    require(home.resolve()==home and home.stat().st_uid==1001 and not logs.is_symlink(),'Owned canonical diagnostic HOME required')
    logs.mkdir(mode=0o700,exist_ok=True);require(stat.S_IMODE(logs.stat().st_mode)==0o700,'Private diagnostic permissions required')
    env={key:os.environ[key] for key in ['PATH','HOME','USER','LANG','XDG_RUNTIME_DIR'] if key in os.environ}
    with tempfile.TemporaryDirectory(prefix='agent-build-',dir=home) as temporary:
        auth=Path(temporary)/'auth';auth.mkdir(mode=0o700);(auth/'config.json').write_text('{"auths":{}}\n');(auth/'config.json').chmod(0o600);env['DOCKER_CONFIG']=str(auth)
        log=Path(temporary)/'build.log'
        args=[binary,'--addr',address,'build','--progress=plain','--frontend=dockerfile.v0','--local','context='+str(context),'--local','dockerfile='+str(context),'--opt','filename=Dockerfile','--opt','platform=linux/amd64','--opt','target=artifact','--opt','build-arg:SOURCE_SHA256='+expected,'--opt','build-arg:BUILD_MODE='+checkpoint['buildMode'],'--output','type=local,dest='+str(output)]
        try:
            with log.open('wb') as capture:result=subprocess.run(args,env=env,stdout=capture,stderr=subprocess.STDOUT,timeout=3600)
        except subprocess.TimeoutExpired:
            result=subprocess.CompletedProcess(args,124)
        if result.returncode:
            with log.open('rb') as data:data.seek(max(0,log.stat().st_size-2*1024*1024));tail=data.read(2*1024*1024)
            fd,name=tempfile.mkstemp(prefix='agent-build-',suffix='.log',dir=logs)
            with os.fdopen(fd,'wb') as data:data.write(tail)
            for old in sorted(logs.glob('agent-build-*.log'),key=lambda path:path.stat().st_mtime,reverse=True)[4:]:old.unlink()
            raise ValueError('EU agent build failed; private log '+name)
    test_counts=validate_transcripts(output,checkpoint['buildMode'])
    if checkpoint['buildMode']=='fixture-only':
        inspected=subprocess.run(['python3',str(context/'agent_elf.py'),str(output/'legacy-attachment-test'),'--output',str(output/'integration-elf-proof.json')],env=env,capture_output=True,text=True,timeout=30)
        require(inspected.returncode==0,'Compiled integration fixture ABI check failed')
        integration=json.loads((output/'integration-elf-proof.json').read_text())
        listing=subprocess.run([str(output/'legacy-attachment-test'),'--list','--ignored'],env=env,capture_output=True,text=True,timeout=30)
        require(listing.returncode==0 and 'owned_k3s_cross_node_update_preserves_data_env_and_rejects_raced_binding: test' in listing.stdout,'Exact reviewed ignored fixture missing')
        (output/'integration-ignored-tests.txt').write_text(listing.stdout)
        proof={**checkpoint,'testTranscripts':test_counts,'compiledIntegrationFixture':integration,'acceptedRuntimeSHA256':checkpoint['reusedSoftwareProof']['artifactSHA256'],'releaseBinaryCompiled':False,'liveKubernetesFixtureExecuted':False,'productionActivationPerformed':False,'promotable':False}
        (output/'agent-build-proof.json').write_text(json.dumps(proof,indent=2)+'\n')
        return {'sourceRef':checkpoint['sourceRef'],'fixture':str(output/'legacy-attachment-test'),'sha256':integration['sha256'],'proof':str(output/'agent-build-proof.json'),'releaseBinaryCompiled':False,'productionActivationPerformed':False}
    probe=subprocess.run(['python3',str(context/'agent_elf.py'),str(output/'spitzli-agent'),'--run-help','--output',str(output/'ubuntu24-elf-proof.json')],env=env,capture_output=True,text=True,timeout=30)
    require(probe.returncode==0,'Ubuntu24 ABI/help probe failed')
    abi=json.loads((output/'ubuntu24-elf-proof.json').read_text())
    integration=None
    if checkpoint['buildMode'] in {'final','fixture-refresh'}:
        inspected=subprocess.run(['python3',str(context/'agent_elf.py'),str(output/'legacy-attachment-test'),'--output',str(output/'integration-elf-proof.json')],env=env,capture_output=True,text=True,timeout=30)
        require(inspected.returncode==0,'Compiled integration fixture ABI check failed')
        integration={**json.loads((output/'integration-elf-proof.json').read_text()),'binary':str(output/'legacy-attachment-test'),'liveKubernetesFixtureExecuted':False}
    proof={**checkpoint,'artifactSHA256':abi['sha256'],'ubuntu24ABI':abi,'testTranscripts':test_counts,'compiledIntegrationFixture':integration,'productionActivationPerformed':False,'reviewRequiredBeforePromotion':True}
    if checkpoint['buildMode']=='fixture-refresh':require(abi['sha256']==checkpoint['reusedSoftwareProof']['artifactSHA256'],'Fixture-only source unexpectedly changed the release binary')
    (output/'agent-build-proof.json').write_text(json.dumps(proof,indent=2)+'\n')
    return {'sourceRef':checkpoint['sourceRef'],'binary':str(output/'spitzli-agent'),'sha256':abi['sha256'],'proof':str(output/'agent-build-proof.json'),'productionActivationPerformed':False}


def main():
    parser=argparse.ArgumentParser(description=__doc__);sub=parser.add_subparsers(dest='command',required=True)
    for name in ['prepare','unpack']:
        command=sub.add_parser(name);command.add_argument('--archive',type=Path,required=True);command.add_argument('--source-sha256',required=True);command.add_argument('--output',type=Path,required=True)
        if name=='prepare':
            command.add_argument('--review-ref',required=True)
            command.add_argument('--mode',choices=['preflight','final','fixture-refresh','fixture-only'],default='preflight')
            command.add_argument('--previous-archive',type=Path);command.add_argument('--previous-sha256');command.add_argument('--reuse-software-proof',type=Path)
    command=sub.add_parser('build');command.add_argument('--context',type=Path,required=True);command.add_argument('--source-sha256',required=True);command.add_argument('--output',type=Path,required=True)
    command=sub.add_parser('validate-tests');command.add_argument('--output',type=Path,required=True);command.add_argument('--mode',choices=['preflight','final','fixture-refresh','fixture-only'],required=True)
    command=sub.add_parser('export-integration');command.add_argument('--messages',type=Path,required=True);command.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    if args.command=='prepare':result=prepare(args.archive.absolute(),args.source_sha256,args.output.absolute(),args.review_ref,args.mode,args.previous_archive,args.previous_sha256,args.reuse_software_proof)
    elif args.command=='unpack':unpack(args.archive,args.source_sha256,args.output);result={'sourceSHA256':args.source_sha256,'unpacked':True}
    elif args.command=='validate-tests':result=validate_transcripts(args.output,args.mode)
    elif args.command=='export-integration':result=export_integration(args.messages,args.output)
    else:result=build(args.context.resolve(),args.source_sha256,args.output.absolute())
    print(json.dumps(result))

if __name__=='__main__':
    try:main()
    except Exception as error:raise SystemExit(str(error)) from None
