#!/usr/bin/env python3
"""Finite owned kernel-credential probe; not Core admission or full recovery acceptance."""
from pathlib import Path
import argparse, importlib.util, os, json, hashlib, uuid, shutil
ROOT=Path(__file__).resolve().parents[2]
def main():
    parser=argparse.ArgumentParser();parser.add_argument('--binary',type=Path,required=True);args=parser.parse_args();os.umask(0o077)
    spec=importlib.util.spec_from_file_location('fixture',ROOT/'tools/tests/mail-application-recovery.py');module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
    base=ROOT/'.runtime-test'/('recovery-ipc-'+uuid.uuid4().hex[:12]);base.mkdir(mode=0o700)
    fixture=module.Fixture(base,ROOT/'plugins/mail/target/debug/examples/recovery-object')
    image='sha256:60e7c44155bbcb84ec27c1f3d20997c12f6246e1d079a9a0225601b72c4c7bbe';binary=args.binary.resolve()
    report={'scope':'actual kernel UID65532 recovery IPC info/denials only; no canonical Core grant/data/volume acceptance','binarySHA256':hashlib.sha256(binary.read_bytes()).hexdigest(),'imageID':image,'priorHarnessFailures':['docker cp into read-only created layer denied','fixture omitted CAP_CHOWN before required UID65 stage setup','fixture Root cleanup lacked DAC_OVERRIDE on enrolled UID65 private directory']}
    volumes=[]
    try:
        container=fixture.create('ipc',image,['--network=none','--cap-drop=ALL','--cap-add=SETUID','--cap-add=SETGID','--cap-add=CHOWN','--cap-add=DAC_OVERRIDE','--tmpfs','/owned:rw,nodev,nosuid,noexec,size=64m,mode=755'],entrypoint='/usr/local/bin/recovery-ipc-probe',env={'WEBDOCK_DISPOSABLE_RECOVERY_IPC':'yes'},command_args=['--ignored','--exact','disposable_recovery_ipc_enforces_kernel_peer_and_preserves_legacy_denial'])
        volumes=[m['Name'] for m in fixture.inspect(container)['Mounts'] if m['Type']=='volume']
        fixture.docker('cp',str(binary),container+':/usr/local/bin/recovery-ipc-probe')
        result=fixture.docker('start','--attach',container,allowed=True,timeout=40);code=fixture.inspect(container)['State']['ExitCode'];report['exitCode']=code
        (base/'test.stdout').write_bytes(result.stdout);(base/'test.stderr').write_bytes(result.stderr)
        print('Owned kernel IPC probe exit='+str(code))
        if code: print(result.stdout.decode()[-1800:])
        report['passed']=code==0
    finally:
        fixture.cleanup()
        for volume in volumes:
            absent=fixture.docker('volume','inspect',volume,allowed=True);assert absent.returncode and b'no such' in absent.stderr.lower()
        report['cleanup']={'exactContainerAbsent':True,'exactAnonymousVolumesAbsent':len(volumes),'privateDirectoryRemoved':report.get('passed',False),'existingServicesChanged':False}
        (base/'report.json').write_text(json.dumps(report,indent=2)+'\n')
        if report.get('passed'):
            (ROOT/'docs/verification/2026-10-25-recovery-ipc-component.json').write_text(json.dumps(report,indent=2)+'\n');shutil.rmtree(base)
        else: print('Private failure evidence retained at '+str(base))
    if not report.get('passed'): raise SystemExit(1)
if __name__=='__main__':main()
