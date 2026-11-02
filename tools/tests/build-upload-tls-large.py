#!/usr/bin/env python3
"""Owned local native TransferClient -> existing BUILD TLS relay -> SSH -> gateway diagnostic."""
import argparse
import datetime
import json
import os
from pathlib import Path
import secrets
import subprocess
import tempfile
import time
ROOT=Path(__file__).resolve().parents[2]


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--sizes',default='64');parser.add_argument('--timeout',type=int,default=60);parser.add_argument('--evidence',type=Path,default=ROOT/'docs/verification/2026-10-30-build-upload-large-tls.json');args=parser.parse_args()
    assert all(int(size) in [64,200,256] for size in args.sizes.split(',')) and 1<=args.timeout<=300
    if int(next(line.split()[1] for line in Path('/proc/meminfo').read_text().splitlines() if line.startswith('MemAvailable:')))*1024<9*1024**3:raise RuntimeError('9GiB_available_required')
    built=subprocess.run(['cargo','test','-p','webdock-build','--features','worker,kubernetes-job','--test','upload_gateway_tls_large','--no-run','--message-format=json'],cwd=ROOT,capture_output=True,text=True,timeout=150)
    if built.returncode:
        for row in map(json.loads,built.stdout.splitlines()):
            if row.get('reason')=='compiler-message':print(row['message'].get('rendered',''))
        raise RuntimeError('large_tls_fixture_compile_failed')
    binaries=[row['executable'] for row in map(json.loads,built.stdout.splitlines()) if row.get('reason')=='compiler-artifact' and row.get('target',{}).get('name')=='upload_gateway_tls_large' and row.get('executable')];assert len(binaries)==1
    work=Path(tempfile.mkdtemp(prefix='webdock-large-tls-'));work.chmod(0o700)
    for name in ['gateway','fixture-data']:(work/name).mkdir(mode=0o700)
    evidence=work/'evidence.json';unit='webdock-large-tls-'+secrets.token_hex(6)+'.service';script='mount --make-rprivate / && mount -t tmpfs -o size=1073741824,mode=0700,nosuid,nodev,noexec owned-large-tls-gateway "$WEBDOCK_TLS_LARGE_WORK/gateway" && mount -t tmpfs -o size=1073741824,mode=0700,nosuid,nodev,noexec owned-large-tls-fixture "$WEBDOCK_TLS_LARGE_WORK/fixture-data" && exec "$1" --exact actual_large_tls_transfer --ignored --test-threads=1'
    try:
        env={'WEBDOCK_TLS_LARGE_WORK':str(work),'WEBDOCK_TLS_LARGE_CERTS':str((ROOT/'.runtime-test/build-cluster/native-20261030-full1-staging').resolve()),'WEBDOCK_TLS_LARGE_TIMEOUT':str(args.timeout),'WEBDOCK_TLS_LARGE_SIZES':args.sizes,'WEBDOCK_TLS_LARGE_EVIDENCE':str(evidence)}
        command=['systemd-run','--user','--quiet','--unit='+unit,'--property=MemoryMax=6G','--property=MemorySwapMax=0','--property=TasksMax=16','--property=CPUQuota=200%','--property=RuntimeMaxSec=900','--property=TimeoutStopSec=5','--property=StandardOutput=null','--property=StandardError=null']+['--setenv='+key+'='+value for key,value in env.items()]+['unshare','--user','--map-root-user','--map-auto','--mount','--propagation','private','/bin/bash','-c',script,'owned-tls-fixture',binaries[0]]
        subprocess.run(command,check=True,capture_output=True,timeout=15)
        until=time.monotonic()+len(args.sizes.split(','))*(args.timeout+120);peak=0
        while time.monotonic()<until:
            state=subprocess.run(['systemctl','--user','show',unit,'--property=ActiveState,Result,MemoryPeak'],capture_output=True,text=True,timeout=10)
            fields=dict(line.split('=',1) for line in state.stdout.splitlines() if '=' in line)
            if fields.get('MemoryPeak','').isdigit():peak=max(peak,int(fields['MemoryPeak']))
            if fields.get('ActiveState')!='active':break
            time.sleep(0.5)
        if not evidence.exists():raise RuntimeError('large_tls_fixture_ended_without_evidence')
        report=json.loads(evidence.read_text());report.update(verifiedAt=datetime.datetime.now(datetime.timezone.utc).isoformat(),unitResult=fields.get('Result'),unitMemoryPeakBytes=peak,memoryMaxBytes=6*1024**3)
        args.evidence.parent.mkdir(parents=True,exist_ok=True);args.evidence.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))
    finally:
        subprocess.run(['systemctl','--user','stop',unit],capture_output=True,timeout=10)
        for name in ['gateway','fixture-data']:
            assert not any((work/name).iterdir());(work/name).rmdir()
        if evidence.exists():evidence.unlink()
        work.rmdir();subprocess.run(['systemctl','--user','reset-failed',unit],capture_output=True,timeout=10)


if __name__=='__main__':main()
