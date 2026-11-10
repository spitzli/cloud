#!/usr/bin/env python3
"""Compile reviewed native retirement coverage, then run one owned PG18 test without Cargo."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import re
import tomllib
import core_quality as quality
import core_tooling

require=quality.require
POLICY_TEST='authority::database_orders::tests::retirement_blocks_both_new_postgres_allocations_but_not_maintenance_or_replay'
RETIREMENT_TEST='retired_postgres_stale_grants_cannot_create_but_retained_work_and_maintenance_survive'
BINARY=Path('/results/native-compile/retirement-test')


def compile_plan():
    return [
        {'id':'retirement-policy-lib','args':['cargo','test','--locked','-p','spitzli-plugin-host','--lib',POLICY_TEST,'--jobs','3','--','--exact','--nocapture','--test-threads=1'],'requiredTests':[POLICY_TEST]},
        {'id':'resource-admission-compile','args':['cargo','test','--locked','-p','spitzli-plugin-host','--test','resource_admission','--no-run','--message-format=json','--jobs','3']},
        {'id':'control-plane-compile','args':['cargo','test','--locked','-p','spitzli-control-plane','--lib','--bins','--tests','--no-run','--message-format=json','--jobs','3']},
    ]


def runtime_plan():return [str(BINARY),'--exact',RETIREMENT_TEST,'--ignored','--nocapture','--test-threads=1']


def messages(raw):
    result=[]
    for line in raw.splitlines():
        try:item=json.loads(line)
        except json.JSONDecodeError:continue
        if item.get('reason')=='compiler-artifact' and item.get('executable'):result.append(item)
    return result


def integration_artifact(raw):
    found=[Path(x['executable']) for x in messages(raw) if x.get('target',{}).get('name')=='resource_admission' and x.get('profile',{}).get('test')]
    require(len(found)==1,'Unique compiled resource_admission test required')
    path=found[0].resolve();require(path.is_relative_to('/quality-target/debug/deps'),'Integration executable must come from the locked private target cache')
    return path


def control_plane_artifacts(items,package_id):
    own=[x for x in items if x.get('package_id')==package_id and x.get('manifest_path')=='/src/crates/control-plane/Cargo.toml']
    bins={x['target']['name'] for x in own if 'bin' in x['target'].get('kind',[])}
    require({'spitzli-control-plane','spitzli-plugin-controller','spitzli-plugin-control','spitzli-agent-control'}<=bins,'Affected control-plane bins were not all compiled from the expected package')
    bootstrap=[x for x in own if x['target'].get('name')=='bootstrap' and x['target'].get('src_path')=='/src/crates/control-plane/tests/bootstrap.rs' and 'test' in x['target'].get('kind',[]) and x.get('profile',{}).get('test')]
    require(len(bootstrap)==1,'Recovery fixture must be compiled through the exact control-plane bootstrap target')
    return {'affectedControlPlaneBinsCompiled':sorted(bins),'recoveryCompileTarget':'bootstrap','recoveryCompileSourcePath':bootstrap[0]['target']['src_path'],'controlPlanePackageID':package_id}


def recovery_module_chain(root=Path('/src')):
    chain=[('crates/control-plane/tests/bootstrap.rs','support/plugin_builtins.rs','plugin_builtins'),('crates/control-plane/tests/support/plugin_builtins.rs','recovery_admission.rs','recovery_admission'),('crates/control-plane/tests/support/recovery_admission.rs','recovery_mail_component.rs','mail_component')]
    proof=[]
    for relative,next_path,module in chain:
        path=root/relative;raw=path.read_text();compact=re.sub(r'\s+','',raw)
        attribute='#[path="'+next_path+'"]';position=compact.find(attribute)
        require(position>=0 and '#![cfg(' not in compact and '#[cfg(' not in compact[max(0,position-256):position],'Recovery support module chain has an unverified conditional declaration')
        declaration=compact[position+len(attribute):]
        require(declaration.startswith('mod'+module+';') or declaration.startswith('pub(super)mod'+module+';'),'Unexpected recovery support module declaration')
        target=path.parent/next_path;require(target.is_file() and not target.is_symlink(),'Recovery support module target missing')
        proof.append({'sourcePath':relative,'sourceSHA256':digest(path),'pathAttribute':next_path,'module':module,'targetPath':str(target.relative_to(root)),'unconditionalDeclarationVerified':True})
    final=root/'crates/control-plane/tests/support/recovery_mail_component.rs'
    proof.append({'sourcePath':str(final.relative_to(root)),'sourceSHA256':digest(final),'supportModuleNotCargoTarget':True})
    return proof


def digest(path):
    with path.open('rb') as f:return hashlib.file_digest(f,'sha256').hexdigest()


def compile_checks(output,checkpoint):
    output.mkdir(mode=0o700);tooling=core_tooling.verify(output)
    records=[]
    for item in compile_plan():
        stdout=output/(item['id']+'.stdout');stderr=output/(item['id']+'.stderr.log')
        carry=json.loads(checkpoint.read_text()).get('controlPlaneCompileCarry') if item['id']=='control-plane-compile' else None
        carry_proof=None
        if carry is not None:
            carry_proof=quality.validate_control_plane_carry(carry,Path('/checkpoint/source.tar'),json.loads(checkpoint.read_text())['sourceArchiveSHA256'],item['args'])
            stdout.write_bytes(carry['stdoutRaw'].encode());stderr.write_bytes(carry['stderrRaw'].encode());result=subprocess.CompletedProcess(item['args'],0)
        else:
            with stdout.open('xb') as out,stderr.open('xb') as err:result=subprocess.run(item['args'],stdout=out,stderr=err,timeout=10800)
        stdout.chmod(0o600);stderr.chmod(0o600)
        if result.returncode:
            print(stderr.read_text(errors='replace')[-65536:]);raise ValueError('Native PG compile gate failed: '+item['id'])
        record={'id':item['id'],'command':item['args'],'exitCode':0,'stdoutSHA256':digest(stdout),'stderrSHA256':digest(stderr)}
        if item.get('requiredTests') is not None:record['testCounts']=quality.test_counts(stdout.read_text()+stderr.read_text(),item)
        if carry_proof is not None:record['carriedCompileEvidence']=carry_proof
        records.append(record)
    integration=integration_artifact((output/'resource-admission-compile.stdout').read_text())
    require(integration.is_file() and not integration.is_symlink(),'Compiled integration executable missing')
    target=output/'retirement-test';shutil.copyfile(integration,target);target.chmod(0o555)
    cp=messages((output/'control-plane-compile.stdout').read_text())
    manifest=tomllib.loads(Path('/src/crates/control-plane/Cargo.toml').read_text());require(manifest['package']['name']=='spitzli-control-plane','Unexpected control-plane package manifest')
    package_id=subprocess.check_output(['cargo','pkgid','--locked','--manifest-path','/src/crates/control-plane/Cargo.toml'],text=True,timeout=60).strip()
    require(package_id.split('#')[0]=='path+file:///src/crates/control-plane','Unexpected control-plane Cargo package source')
    closure=control_plane_artifacts(cp,package_id);chain=recovery_module_chain()
    proof={**json.loads(checkpoint.read_text()),'mode':'native-pg-compile','compileChecksPassed':True,'checks':records,**closure,'recoveryMailComponentCompiled':True,'recoverySupportModuleChain':chain,'binarySHA256':digest(target),'binaryBytes':target.stat().st_size,'tooling':tooling,'controlPlaneCompileReexecuted':not bool(json.loads(checkpoint.read_text()).get('controlPlaneCompileCarry')),'livePGAcceptanceExecuted':False,'productionActivationPerformed':False}
    (output/'native-pg-compile-proof.json').write_text(json.dumps(proof,indent=2)+'\n')
    return proof


def runtime_checks(output,checkpoint):
    require(shutil.which('cargo') is None,'Native PG runtime must not depend on Cargo tools')
    for name in ['initdb','pg_ctl','createdb','postgres','psql']:require((Path('/usr/lib/postgresql/18/bin')/name).is_file(),'Fixed owned PG18 tool missing: '+name)
    compile_root=BINARY.parent;compiled=json.loads((compile_root/'native-pg-compile-proof.json').read_text());source=json.loads(checkpoint.read_text())
    require(compiled['sourceArchiveSHA256']==source['sourceArchiveSHA256'] and compiled['compileChecksPassed'],'Compiled test belongs to a different source or failed gate')
    require(digest(BINARY)==compiled['binarySHA256'],'Copied compiled test checksum mismatch')
    roots_before=set(Path('/tmp').glob('webdock-owned-plugin-jobs-*'));require(not roots_before,'Native PG fixture stage must start without an old owned fixture root')
    output.mkdir(mode=0o700);tooling=core_tooling.verify(output)
    subprocess.run(['python3','/src/tools/plugin-ci/agent_elf.py',str(BINARY),'--output',str(output/'native-pg-elf-proof.json')],check=True,timeout=30,capture_output=True)
    env={k:v for k,v in os.environ.items() if k in {'PATH','HOME','USER','LANG','CI','NODE_OPTIONS'}};env['WEBDOCK_PLUGIN_TEST_LOCAL_POSTGRES']='1';env['DOCKER_HOST']=core_tooling.NO_DAEMON
    log=output/'retirement-pg18.log'
    with log.open('xb') as out:result=subprocess.run(runtime_plan(),env=env,stdout=out,stderr=subprocess.STDOUT,timeout=1800)
    log.chmod(0o600);raw=log.read_text(errors='replace');clean=set(Path('/tmp').glob('webdock-owned-plugin-jobs-*'))==roots_before
    if result.returncode or not clean:
        print(raw[-65536:]);raise ValueError('Exact owned PG18 retirement test failed or cleanup did not finish')
    counts=quality.test_counts(raw,{'id':'retirement-pg18','requiredTests':[RETIREMENT_TEST]})
    require(counts['passed']==1,'Exactly one reviewed ignored PG test must execute')
    for p in compile_root.iterdir():
        if p.name!='retirement-test':shutil.copyfile(p,output/p.name)
    proof={**source,'mode':'native-pg','compileProof':compiled,'fixturePassed':True,'ownedDatabaseProcessStoppedAndRootRemoved':clean,'testCounts':counts,'exactTest':RETIREMENT_TEST,'command':runtime_plan(),'binarySHA256':compiled['binarySHA256'],'tooling':tooling,'cargoInstalledInPGRuntime':False,'externalDatabaseURLAccepted':False,'productionActivationPerformed':False}
    (output/'core-quality-proof.json').write_text(json.dumps(proof,indent=2)+'\n');return proof


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--phase',choices=['compile','runtime'],required=True);p.add_argument('--output',type=Path,required=True);p.add_argument('--checkpoint',type=Path,required=True);a=p.parse_args()
    require(os.getuid()==1001 and Path.cwd()==Path('/src'),'Isolated nonroot native fixture required')
    require(not a.output.exists() and a.output.resolve().is_relative_to('/results'),'New private results directory required')
    proof=compile_checks(a.output,a.checkpoint) if a.phase=='compile' else runtime_checks(a.output,a.checkpoint)
    print(json.dumps({'phase':a.phase,'sourceArchiveSHA256':proof['sourceArchiveSHA256'],'passed':True,'productionActivationPerformed':False}))

if __name__=='__main__':
    try:main()
    except Exception as e:raise SystemExit(str(e)) from None
