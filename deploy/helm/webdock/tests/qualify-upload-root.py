#!/usr/bin/env python3
"""Owned local Docker kernel proof; no Kubernetes/RUN access, pulls or credentials."""
import argparse,json,re,secrets,subprocess
p=argparse.ArgumentParser();p.add_argument('--image',required=True);a=p.parse_args()
if not re.fullmatch(r'sha256:[a-f0-9]{64}',a.image):raise SystemExit('local immutable image ID required')
def run(*args):return subprocess.check_output(['docker',*args],text=True).strip()
run('image','inspect',a.image)
nonce=secrets.token_hex(8);volume='wd-controller-upload-'+nonce
run('volume','create','--driver','local','--label','webdock.dev/controller-upload-check='+nonce,'--opt','type=tmpfs','--opt','device=tmpfs','--opt','o=size=1g',volume)
def container(user,script,mount,cap=None):
    flags=['run','--rm','--pull','never','--network','none','--read-only','--user',user,'--cap-drop','ALL','--security-opt','no-new-privileges','--mount',mount,'--entrypoint','/bin/sh']
    if cap:flags+=['--cap-add',cap]
    return run(*flags,a.image,'-ec',script)
holder=None
try:
    holder=run('run','-d','--pull','never','--network','none','--read-only','--user','65532:65532','--cap-drop','ALL','--security-opt','no-new-privileges','--label','webdock.dev/controller-upload-check='+nonce,'--mount','type=volume,src='+volume+',dst=/hold','--entrypoint','/bin/sh',a.image,'-ec','sleep 120')
    meta=json.loads(run('volume','inspect',volume))[0]
    assert meta['Labels']['webdock.dev/controller-upload-check']==nonce
    command="if [ \"$(stat -c '%u:%g:%a' /upload)\" != 65532:65532:700 ]; then chmod 0700 /upload; chown 65532:65532 /upload; fi; test \"$(stat -c '%u:%g:%a' /upload)\" = 65532:65532:700"
    init=container('0:0',command+'; stat -c \"%u:%g:%a\" /upload','type=volume,src='+volume+',dst=/upload','CHOWN')
    repeated=container('0:0',command+'; stat -c \"%u:%g:%a\" /upload','type=volume,src='+volume+',dst=/upload','CHOWN')
    script='stat -c "%u:%g:%a" /upload; awk \'$5=="/upload"{print $4, $(NF-2)}\' /proc/self/mountinfo; stat -f -c \"%S:%b\" /upload; stat -c \"%d:%i\" /upload; mkdir /upload/child; chmod 700 /upload/child'
    positive=container('65532:65532',script,'type=volume,src='+volume+',dst=/upload')
    negative=container('65532:65532','awk \'$5=="/upload"{print $4, $(NF-2)}\' /proc/self/mountinfo','type=bind,src='+meta['Mountpoint']+'/child,dst=/upload')
    assert init=='65532:65532:700' and positive.splitlines()[:2]==['65532:65532:700','/ tmpfs'],positive
    assert repeated==init,repeated
    block,blocks=map(int,positive.splitlines()[2].split(':'))
    assert block*blocks==1073741824,positive
    assert negative=='/child tmpfs',negative
    print(json.dumps({'nonce':nonce,'rootInit':'UID0 dropALL addCHOWN only; readonly root; no network','positive':positive.splitlines()[:2],'capacityBytes':block*blocks,'deviceInode':positive.splitlines()[3],'repeatInit':repeated,'negativeSubPath':negative,'predicate':'mountinfo root predicate only: / satisfies it, /child fails; actual Rust constructor not invoked','cleanup':'exact nonce-labeled volume removed'},indent=2))
finally:
    if holder:
        holder_meta=json.loads(run('inspect',holder))[0]
        assert holder_meta['Config']['Labels']['webdock.dev/controller-upload-check']==nonce
        run('rm','-f',holder)
    meta=json.loads(run('volume','inspect',volume))[0]
    assert meta['Labels']['webdock.dev/controller-upload-check']==nonce
    run('volume','rm',volume)
