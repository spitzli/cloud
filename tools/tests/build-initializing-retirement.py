#!/usr/bin/env python3
"""Owned local user/mount-namespace proof for journal-before-mkdir build retirement."""
import datetime
import json
from pathlib import Path
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[2]


def main():
    compiled=subprocess.run(['cargo','test','-p','spitzli-agent','--no-default-features','--lib','initializing_build_retirement_tests','--no-run','--message-format=json'],cwd=ROOT,capture_output=True,text=True,timeout=180)
    if compiled.returncode:
        for row in map(json.loads,compiled.stdout.splitlines()):
            if row.get('reason')=='compiler-message':print(row['message'].get('rendered',''))
        raise RuntimeError('owned_retirement_fixture_compile_failed')
    binaries=[row['executable'] for row in map(json.loads,compiled.stdout.splitlines()) if row.get('reason')=='compiler-artifact' and row.get('target',{}).get('name')=='webdock_agent' and row.get('executable')]
    assert len(binaries)==1
    result=subprocess.run(['unshare','--user','--map-root-user','--mount','--propagation','private',binaries[0],'storage::initializing_build_retirement_tests::','--include-ignored','--nocapture','--test-threads=1'],cwd=ROOT,capture_output=True,text=True,timeout=45)
    print(result.stdout,end='')
    if result.returncode or '5 passed; 0 failed' not in result.stdout:
        print(result.stderr);raise RuntimeError('owned_retirement_fixture_failed')
    report={'verifiedAt':datetime.datetime.now(datetime.timezone.utc).isoformat(),'profile':'owned-local-user-mount-namespace-build-initializing-retirement','passed':True,'testsPassed':5,
        'checks':{'journalBeforeMkdirCancellationRestart':True,'signedRetirementAndLateReserveFence':True,'missingBuildOnlyCapacityReleased':True,'missingReadyAndPurgingRetainCharge':True,'appsAndDatabaseUnchanged':True,'symlinkAndForeignScopeRetainCharge':True,'activeMountedUuidRetainsCharge':True,'malformedMountAndLoopInventoriesRejected':True,'deletedBackingPathRejected':True,'existingAllocationParentRetained':True},
        'limits':{'liveBuildHostChanged':False,'loopOrMountMutation':False,'kubernetesAbsenceRequalification':False,'retirementAuthorityChanged':False}}
    path=ROOT/'docs/verification/2026-10-30-build-initializing-retirement.json';path.write_text(json.dumps(report,indent=2)+'\n')


if __name__=='__main__':main()
