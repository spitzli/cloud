#!/usr/bin/env python3
"""Build-host log filesystem qualification and identity verification; no tenant code."""
import argparse
import errno
import hashlib
import json
import os
from pathlib import Path
import secrets
import socket
import stat
import subprocess
import shutil
import tempfile
import uuid

def run(*args,check=True):
    result=subprocess.run(args,text=True,capture_output=True,timeout=45)
    if check and result.returncode:raise RuntimeError("bounded log operation failed: "+args[0])
    return result

def private(path,directory=False):
    info=path.lstat()
    if path.resolve()!=path or info.st_uid!=0 or info.st_mode&0o077 or (directory and not stat.S_ISDIR(info.st_mode)) or (not directory and (not stat.S_ISREG(info.st_mode) or info.st_nlink!=1)):
        raise RuntimeError("owned private log-store path required")
    return info

def validate_mount(record,image_info,mount,loop):
    options=set(mount.get("options","").split(','))
    if mount.get("target")!=record['target'] or mount.get('fstype')!='ext4' or mount.get('uuid')!=record['uuid'] or not {'rw','nosuid','nodev','noexec'}<=options:
        raise RuntimeError("log-store mount identity drift")
    if loop.get('back-file')!=record['image'] or loop.get('back-ino')!=image_info.st_ino or loop.get('back-maj:min')!=f'{os.major(image_info.st_dev)}:{os.minor(image_info.st_dev)}':
        raise RuntimeError("log-store backing identity drift")

def verify(record):
    if record.get('state','ready')!='ready':raise RuntimeError('log-store initialization incomplete')
    image=Path(record['image']);info=private(image)
    if info.st_size!=record['bytes'] or info.st_ino!=record['inode'] or info.st_dev!=record['device']:
        raise RuntimeError("log-store image replaced")
    mounts=json.loads(run('/usr/bin/findmnt','--json','--mountpoint',record['target'],'-o','SOURCE,TARGET,FSTYPE,OPTIONS,UUID').stdout)['filesystems']
    if len(mounts)!=1:raise RuntimeError("one exact log-store mount required")
    source=mounts[0]['source']
    if not source.startswith('/dev/loop') or not source[9:].isdigit():raise RuntimeError("private loop filesystem required")
    loops=json.loads(run('/usr/sbin/losetup','--json','--list','--output','NAME,BACK-FILE,BACK-INO,BACK-MAJ:MIN',source).stdout)['loopdevices']
    if len(loops)!=1:raise RuntimeError("one exact log-store backing required")
    validate_mount(record,info,mounts[0],loops[0])
    if run('/usr/sbin/blkid','-p','-s','UUID','-o','value',str(image)).stdout.strip()!=record['uuid']:
        raise RuntimeError("log-store UUID drift")

def save_record(path,record):
    with tempfile.NamedTemporaryFile(mode='w',dir=path.parent,delete=False) as output:
        os.fchmod(output.fileno(),0o600);json.dump(record,output);output.flush();os.fsync(output.fileno());temporary=output.name
    os.replace(temporary,path)
    fd=os.open(path.parent,os.O_RDONLY|os.O_DIRECTORY)
    try:os.fsync(fd)
    finally:os.close(fd)

def install():
    record_path=Path('/etc/webdock-build-logs.json')
    if record_path.exists():
        private(record_path);verify(json.loads(record_path.read_text()));print('Bounded log store already verified');return
    inventory=json.loads(run('/usr/local/bin/k3s','kubectl','get','pods','-A','-o','json').stdout)['items']
    if any(p['metadata']['namespace']!='kube-system' and p.get('status',{}).get('phase') not in ['Succeeded','Failed'] for p in inventory):
        raise RuntimeError('active non-system Pod; refuse log migration')
    target=Path('/var/log/pods');backup=Path('/var/log/pods.before-webdock-bounded')
    if target.is_symlink() or backup.exists() or run('/usr/bin/findmnt','--mountpoint',str(target),check=False).returncode==0:
        raise RuntimeError('unowned log target or existing migration; refuse replacement')
    total=0
    if target.exists():
        for path in target.rglob('*'):
            info=path.lstat()
            if not stat.S_ISDIR(info.st_mode) and not (stat.S_ISREG(info.st_mode) and info.st_nlink==1):raise RuntimeError('unexpected existing log file type')
            total+=info.st_size
    if total>256*1024*1024:raise RuntimeError('existing logs exceed bounded migration budget')
    parent=Path('/var/lib/webdock-build-logs');parent.mkdir(mode=0o700,exist_ok=True);private(parent,True)
    image=parent/'pods.ext4';size=4*1024*1024*1024
    free=os.statvfs(parent)
    if free.f_bavail*free.f_frsize<size+20*1024*1024*1024:raise RuntimeError('insufficient host/control-plane reserve')
    fd=os.open(image,os.O_RDWR|os.O_CREAT|os.O_EXCL,0o600)
    try:os.posix_fallocate(fd,0,size);os.fsync(fd)
    finally:os.close(fd)
    info=image.stat();record={'state':'initializing','image':str(image),'target':str(target),'uuid':str(uuid.uuid4()),'bytes':size,'inode':info.st_ino,'device':info.st_dev,'backup':str(backup)}
    save_record(record_path,record)
    run('/usr/sbin/mkfs.ext4','-q','-m','0','-U',record['uuid'],'-E','nodiscard,lazy_itable_init=0,lazy_journal_init=0',str(image))
    # From this point a failure retains the exact data/config and stays closed;
    # never silently restart untrusted execution with an unbounded fallback.
    run('/usr/bin/systemctl','stop','k3s')
    if target.exists():target.rename(backup)
    target.mkdir(mode=0o755);run('/usr/bin/chattr','+i',str(target))
    run('/usr/bin/mount','-t','ext4','-o','loop,nodev,nosuid,noexec',str(image),str(target))
    if backup.exists():shutil.copytree(backup,target,dirs_exist_ok=True,copy_function=shutil.copy2)
    script=Path('/usr/local/libexec/webdock-build-bounded-logs');script.parent.mkdir(mode=0o755,parents=True,exist_ok=True)
    shutil.copyfile(Path(__file__).resolve(),script);script.chmod(0o755)
    unit=Path('/etc/systemd/system/var-log-pods.mount')
    unit.write_text('[Unit]\nDescription=Bounded private Webdock build-cluster CRI logs\nBefore=k3s.service\n\n[Mount]\nWhat='+str(image)+'\nWhere='+str(target)+'\nType=ext4\nOptions=loop,nodev,nosuid,noexec\n\n[Install]\nWantedBy=multi-user.target\n');unit.chmod(0o644)
    dropin=Path('/etc/systemd/system/k3s.service.d');dropin.mkdir(mode=0o755,exist_ok=True)
    (dropin/'30-webdock-bounded-logs.conf').write_text('[Unit]\nRequires=var-log-pods.mount\nAfter=var-log-pods.mount\nBindsTo=var-log-pods.mount\n[Service]\nExecStartPre=/usr/local/libexec/webdock-build-bounded-logs --qualified-build-host --verify /etc/webdock-build-logs.json\n')
    journal=Path('/etc/systemd/journald.conf.d');journal.mkdir(mode=0o755,exist_ok=True)
    (journal/'40-webdock-build-budget.conf').write_text('[Journal]\nSystemMaxUse=512M\nRuntimeMaxUse=128M\nSystemMaxFileSize=64M\nRuntimeMaxFileSize=32M\n')
    record['state']='ready';save_record(record_path,record);verify(record)
    run('/usr/bin/systemctl','daemon-reload');run('/usr/bin/systemctl','enable','--now','var-log-pods.mount')
    run('/usr/bin/systemctl','restart','systemd-journald');run('/usr/bin/systemctl','start','k3s')
    print(json.dumps({'status':'bounded-log-store-installed','bytes':size,'uuid':record['uuid'],'priorLogsRetainedAt':str(backup),'fallback':'immutable-directory-plus-k3s-mount-dependency'}))

def qualify():
    parent=Path('/var/lib/webdock-build-log-proof');parent.mkdir(mode=0o700,exist_ok=True);private(parent,True)
    work=parent/secrets.token_hex(12);work.mkdir(mode=0o700);private(work,True)
    target=work/'logs';target.mkdir(mode=0o700)
    image=work/'logs.ext4';size=64*1024*1024
    fd=os.open(image,os.O_RDWR|os.O_CREAT|os.O_EXCL,0o600)
    try:os.posix_fallocate(fd,0,size);os.fsync(fd)
    finally:os.close(fd)
    info=image.stat();record={'image':str(image),'target':str(target),'uuid':str(uuid.uuid4()),'bytes':size,'inode':info.st_ino,'device':info.st_dev}
    mounted=False;guarded=False
    try:
        run('/usr/sbin/mkfs.ext4','-q','-m','0','-U',record['uuid'],'-E','nodiscard,lazy_itable_init=0,lazy_journal_init=0',str(image))
        run('/usr/bin/chattr','+i',str(target));guarded=True
        def deny_fallback():
            try:(target/'unexpected-host-log').write_bytes(b'not allowed')
            except PermissionError:return
            raise RuntimeError('unmounted log target permits unbounded host fallback')
        deny_fallback()
        run('/usr/bin/mount','-t','ext4','-o','loop,nodev,nosuid,noexec',str(image),str(target));mounted=True
        verify(record)
        written=0
        with (target/'owned.log').open('wb',buffering=0) as output:
            block=b'x'*(1024*1024)
            try:
                while written<size*2:written+=output.write(block)
            except OSError as error:
                if error.errno!=errno.ENOSPC:raise
        if not 0<written<size:raise RuntimeError('hard log-store byte boundary not observed')
        (target/'owned.log').unlink()
        run('/usr/bin/umount',str(target));mounted=False
        deny_fallback()
        run('/usr/bin/mount','-t','ext4','-o','loop,nodev,nosuid,noexec',str(image),str(target));mounted=True;verify(record)
        print(json.dumps({'status':'owned-log-substrate-qualified','bytesBeforeEnospc':written,'allocationBytes':size,'unmountedRootFallbackDenied':True,'remountIdentityVerified':True}))
    finally:
        if mounted:run('/usr/bin/umount',str(target))
        attached=json.loads(run('/usr/sbin/losetup','--json','--list','--associated',str(image)).stdout)['loopdevices']
        if attached:raise RuntimeError('owned log loop still referenced; retain exact proof work')
        if guarded:run('/usr/bin/chattr','-i',str(target))
        target.rmdir();image.unlink();work.rmdir()

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--qualified-build-host',action='store_true',required=True)
    action=parser.add_mutually_exclusive_group(required=True);action.add_argument('--qualify-owned',action='store_true');action.add_argument('--verify',type=Path);action.add_argument('--install',action='store_true')
    args=parser.parse_args()
    if os.geteuid()!=0 or socket.gethostname()!='webdock-build-eu-01' or not Path('/etc/webdock-build-host').is_file():raise SystemExit('explicit owned build host required')
    if args.qualify_owned:qualify()
    elif args.install:install()
    else:private(args.verify);verify(json.loads(args.verify.read_text()))

if __name__=='__main__':main()
