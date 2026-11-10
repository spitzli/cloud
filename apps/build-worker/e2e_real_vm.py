#!/usr/bin/env python3
"""Opt-in local KVM acceptance; requires a disposable prepared raw image and static BusyBox.

No control service, enrollment, publication credentials or host Docker are used.
Only authored fixture Dockerfiles execute, inside the real worker QEMU boundary.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import tempfile
import time
import worker


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--image',type=Path,required=True)
    parser.add_argument('--image-sha256',required=True)
    parser.add_argument('--busybox',type=Path,required=True)
    parser.add_argument('--scratch',type=Path,required=True)
    parser.add_argument('--allow-disposable-local-vm',action='store_true',required=True)
    args=parser.parse_args()
    image=args.image.resolve(); scratch=args.scratch.resolve()
    if hashlib.file_digest(image.open('rb'),'sha256').hexdigest()!=args.image_sha256:
        parser.error('image SHA256 mismatch')
    if not os.access('/dev/kvm',os.R_OK|os.W_OK):parser.error('writable /dev/kvm required')
    scratch.mkdir(mode=0o700,parents=True,exist_ok=False)
    temp=scratch/'tmp';temp.mkdir(mode=0o700)
    tempfile.tempdir=str(temp)
    store=scratch/'artifacts';store.mkdir(mode=0o700)
    source=scratch/'source';source.mkdir()
    shutil.copyfile(args.busybox,source/'busybox');(source/'busybox').chmod(0o755)
    (source/'index.html').write_text('webdock-real-kvm-e2e\n')
    config={'image':str(image),'artifactRoot':str(store)}
    job={'customerID':'999991','projectID':'999992','buildID':'999993','generation':1,
         'sourceSHA':'a'*40,'recipe':'dockerfile','rootDirectory':'.','buildEnvironment':{},
         'limits':{'cpu':2,'memoryMiB':2048,'diskBytes':24*1024**3,'durationSeconds':180}}
    original_hash=args.image_sha256
    os.environ['SPITZLI_TEST_HOST_ONLY']='host-only-must-never-enter-vm'
    common='FROM scratch\nCOPY busybox /busybox\nSHELL ["/busybox", "sh", "-c"]\n'
    checks=[]
    def run(name,dockerfile,expected=None):
        job['generation']+=1
        (source/'Dockerfile').write_text(common+dockerfile)
        started=time.monotonic()
        try:
            artifact,logs=worker.execute_vm(config,job,worker.pack(source))
            if expected:raise AssertionError(f'{name}: expected {expected}, build succeeded')
            target=scratch/name;target.mkdir();worker.unpack((store/artifact['storageKey']).read_bytes(),target)
            digest=worker.validate_oci(target/'image.tar')
            result={'name':name,'status':'passed','artifact':str(target/'image.tar'),'imageDigest':digest,'logs':logs}
        except worker.BuildFailed as error:
            if expected is None or expected not in str(error):raise
            result={'name':name,'status':'passed','expectedFailure':str(error),'logs':error.logs}
        result['elapsedSeconds']=round(time.monotonic()-started,2);checks.append(result)
        (scratch/'evidence.json').write_text(json.dumps({'qemu':'/usr/bin/qemu-system-x86_64','accelerator':'kvm','network':'none','imageSHA256':original_hash,'checks':checks},indent=2))
        print(json.dumps({key:value for key,value in result.items() if key!='logs'}),flush=True)
    run('http-app',
        'RUN --mount=type=cache,target=/cache test ! -e /cache/previous && /busybox touch /cache/previous && test -z "$SPITZLI_TEST_HOST_ONLY" && test ! -e /etc/webdock-build-worker/credential && ! /busybox wget -T 2 -q -O - http://169.254.169.254/\n'
        'COPY index.html /www/index.html\nUSER 65532:65532\nEXPOSE 8080\nENTRYPOINT ["/busybox","httpd","-f","-p","8080","-h","/www"]\n')
    run('fresh-second-build',
        'RUN --mount=type=cache,target=/cache test ! -e /cache/previous && test -z "$SPITZLI_TEST_HOST_ONLY"\nCOPY index.html /www/index.html\n')
    run('failed-command','RUN echo authored-failure-visible; exit 17\n','build_failed')
    assert 'authored-failure-visible' in checks[-1]['logs']
    run('log-limit','RUN /busybox yes bounded-log-flood\n','build_failed')
    assert len(checks[-1]['logs'].encode())<=worker.MAX_LOG
    assert checks[-1]['logs'].count('bounded-log-flood')>10
    job['limits']['durationSeconds']=30
    run('guest-timeout','RUN /busybox sleep 120\n','build_failed')
    assert checks[-1]['elapsedSeconds']<30
    # Exercise the outer host watchdog separately from the earlier guest deadline.
    request=scratch/'host-deadline-input';request.mkdir()
    shutil.copytree(source,request/'source')
    job['limits']['durationSeconds']=180
    (request/'job.json').write_text(json.dumps(worker.guest_request(job)))
    input_disk=scratch/'host-deadline.tar';input_disk.write_bytes(worker.pack(request))
    output_disk=scratch/'host-deadline-result.tar'
    started=time.monotonic()
    with tempfile.TemporaryDirectory(dir=temp) as overlay_directory:
        try:
            worker.run_bounded(worker.qemu_command(str(image),input_disk,output_disk,job['limits']),
                timeout=5,file_limit=worker.execution_disk_limit(image,job['limits']['diskBytes']),
                env={'PATH':'/usr/bin:/bin','TMPDIR':overlay_directory})
            raise AssertionError('host watchdog did not stop VM')
        except worker.BuildFailed as error:
            assert str(error)=='execution_timeout',str(error)
    try:
        os.waitpid(-1,os.WNOHANG)
        raise AssertionError('host watchdog left a child process')
    except ChildProcessError:pass
    checks.append({'name':'host-deadline','status':'passed','expectedFailure':'execution_timeout',
                   'elapsedSeconds':round(time.monotonic()-started,2)})
    (scratch/'evidence.json').write_text(json.dumps({'qemu':'/usr/bin/qemu-system-x86_64','accelerator':'kvm',
        'network':'none','imageSHA256':original_hash,'checks':checks},indent=2))
    print(json.dumps(checks[-1]),flush=True)
    assert hashlib.file_digest(image.open('rb'),'sha256').hexdigest()==original_hash,'base image mutated'
    assert not list(temp.iterdir()),'temporary execution files leaked'
    print('Real KVM acceptance passed; base image unchanged and temporary VM files removed.',flush=True)

if __name__=='__main__':main()
