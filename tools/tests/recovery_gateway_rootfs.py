"""One owned gateway image closure, executed only in the pinned cached-base build stage."""
import hashlib,json,os,posixpath,re,shutil,stat,struct,subprocess,sys
from pathlib import Path
ALLOWED=(Path('/lib'),Path('/lib64'),Path('/usr/lib'),Path('/usr/lib64'))
def library(path):
    return path.is_absolute() and '..' not in path.parts and any(path==p or p in path.parents for p in ALLOWED)
def interpreter(binary):
    with binary.open('rb') as f:
        head=f.read(64)
        if head[:6]!=b'\x7fELF\x02\x01' or struct.unpack_from('<H',head,18)[0]!=62:raise ValueError('gateway must be linux amd64 ELF64')
        offset=struct.unpack_from('<Q',head,32)[0];size,count=struct.unpack_from('<HH',head,54)
        if size!=56 or count>128:raise ValueError('ELF program header bound')
        for n in range(count):
            f.seek(offset+n*size);p=f.read(size)
            if struct.unpack_from('<I',p)[0]==3:
                begin=struct.unpack_from('<Q',p,8)[0];length=struct.unpack_from('<Q',p,32)[0]
                if not 1<length<=256:raise ValueError('ELF interpreter bound')
                f.seek(begin);return Path(f.read(length).rstrip(b'\0').decode('ascii'))
    return None

def build(binary,root):
    binary=Path(binary);root=Path(root)
    if not binary.is_file() or binary.is_symlink() or root.exists():raise ValueError('fresh gateway closure required')
    root.mkdir(mode=0o755);paths=set();interp=interpreter(binary)
    if interp is not None:
        if not library(interp):raise ValueError('foreign ELF interpreter')
        paths.add(interp)
        fixed_path='/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin'
        command=shutil.which('ldd',path=fixed_path)
        if command is None:raise ValueError('maintained ldd unavailable')
        emit_builder_tool('ldd',command)
        command=Path(command).resolve(strict=True);metadata=command.stat()
        if command.parent not in (Path('/usr/bin'),Path('/usr/local/bin'),Path('/bin')) or not metadata.st_mode&0o111 or metadata.st_uid!=0 or metadata.st_mode&0o022:raise ValueError('maintained ldd executable custody')
        result=subprocess.run([str(command),str(binary)],capture_output=True,timeout=10,env={'PATH':fixed_path,'LC_ALL':'C'})
        if result.returncode or b'not found' in result.stdout or result.stderr:raise ValueError('unresolved gateway dependency')
        for line in result.stdout.decode('ascii').splitlines():
            if '=>' in line:part=line.split('=>',1)[1].strip().split(' ',1)[0]
            else:part=line.strip().split(' ',1)[0]
            if not part.startswith('/'):continue
            path=Path(part)
            if not library(path):raise ValueError('foreign shared library')
            paths.add(path)
    files={};links={};total=0
    for path in paths:
        resolved=path.resolve(strict=True)
        if not library(resolved) or not resolved.is_file():raise ValueError('foreign dependency target')
        for parent in [*reversed(path.parents),path]:
            if parent.is_symlink():
                alias=parent.parent.resolve(strict=True)/parent.name;target=os.readlink(parent)
                if not library(parent.resolve(strict=True)):raise ValueError('library symlink escape')
                links[alias]=target
        files[resolved]=resolved
    files[Path('/usr/local/bin/webdock-recovery-gateway')]=binary
    # Copy regular canonical paths first; never follow a generated absolute symlink while writing.
    for dest,src in files.items():
        target=root/str(dest).lstrip('/');target.parent.mkdir(parents=True,exist_ok=True)
        size=src.stat().st_size;total+=size
        if total>256*1024*1024:raise ValueError('gateway filesystem exceeds 256MiB')
        shutil.copyfile(src,target);target.chmod(0o555 if dest.name=='webdock-recovery-gateway' else stat.S_IMODE(src.stat().st_mode)&0o755)
    for alias,target in sorted(links.items(),key=lambda row:len(row[0].parts),reverse=True):
        path=root/str(alias).lstrip('/');path.parent.mkdir(parents=True,exist_ok=True)
        if path.exists() or path.is_symlink():raise ValueError('dependency alias collision')
        path.symlink_to(target)
    manifest={'scope':'only current gateway ELF + actual interpreter/sharedlibs from exact base','bytes':total,'files':{str(dest):{'bytes':src.stat().st_size,'sha256':hashlib.sha256(src.read_bytes()).hexdigest()} for dest,src in sorted(files.items())},'symlinks':{str(k):v for k,v in sorted(links.items())}}
    (root/'gateway-files.json').write_text(json.dumps(manifest,sort_keys=True)+'\n')
# The Mail profile reads only these tools and their ELF closure from a read-only cached-base mount.
MAIL_BASE_ROOT=Path('/cached-base')
MAIL_SOURCE_UID=0
MAIL_TOOLS=('/bin/sh','/bin/cp','/bin/mkdir','/bin/chmod','/bin/tar','/bin/sleep')
MAIL_MAX_BYTES=192*1024*1024
MAIL_MAX_FILES=128
MAIL_MAX_INODES=512
MAIL_DNS_PROBE_SHA256='0fe69dd5bcd00178e750f167cc050d46313bb62b8962a4b9ceae314b1284891a'

def mail_source(path,links,*,tool=None):
    path=Path(path)
    if not path.is_absolute() or '..' in path.parts:raise ValueError('invalid Mail closure path')
    for _ in range(32):
        current=Path('/')
        for part in path.parts[1:]:
            current=current/part;source=MAIL_BASE_ROOT/str(current).lstrip('/')
            if source.is_symlink():
                if source.lstat().st_uid!=MAIL_SOURCE_UID:raise ValueError('unowned Mail source symlink')
                target=os.readlink(source);links[current]=target
                target=target if target.startswith('/') else str(current.parent/target)
                suffix=path.relative_to(current).as_posix()
                path=Path(posixpath.normpath(target+('/'+suffix if suffix!='.' else '')))
                break
        else:
            if tool is None:
                allowed=library(path) and not any(p in path.parents for p in (Path('/lib/cargo'),Path('/usr/lib/cargo')))
            elif tool=='sh':allowed=path.parent in (Path('/bin'),Path('/usr/bin')) and path.name in ('sh','dash','bash','busybox')
            elif tool=='cp':allowed=path in {Path(p) for p in ('/bin/cp','/usr/bin/cp','/bin/gnucp','/usr/bin/gnucp')}
            elif tool in ('mkdir','chmod','sleep'):allowed=path in {Path(parent)/tool for parent in ('/bin','/usr/bin','/lib/cargo/bin/coreutils','/usr/lib/cargo/bin/coreutils')}
            elif tool=='tar':allowed=path in (Path('/bin/tar'),Path('/usr/bin/tar'))
            else:allowed=False
            if not allowed:raise ValueError('foreign Mail dependency target')
            source=MAIL_BASE_ROOT/str(path).lstrip('/');metadata=source.lstat()
            if not stat.S_ISREG(metadata.st_mode) or metadata.st_uid!=MAIL_SOURCE_UID or metadata.st_mode&0o022:raise ValueError('unsafe Mail dependency')
            return path,source
    raise ValueError('Mail dependency symlink cycle')

def build_mail(binary,root):
    binary=Path(binary);root=Path(root)
    if not binary.is_file() or binary.is_symlink() or root.exists() or not MAIL_BASE_ROOT.is_dir():raise ValueError('fresh Mail closure required')
    root.mkdir(mode=0o755);root.chmod(0o755)
    for parent in (root/'etc',root/'run',root/'run/webdock'):parent.mkdir(mode=0o755);parent.chmod(0o755)
    files={Path('/usr/local/bin/webdock-mail'):binary};links={};source_receipts={}
    selected=[binary]
    for path in MAIL_TOOLS:
        canonical,source=mail_source(Path(path),links,tool=Path(path).name);files[canonical]=source;selected.append(source)
    for source in selected:
        interp=interpreter(source)
        if interp is None:continue
        canonical,loader=mail_source(interp,links);files[canonical]=loader
        search=':'.join(str(MAIL_BASE_ROOT/p) for p in ('lib/x86_64-linux-gnu','usr/lib/x86_64-linux-gnu','lib64','usr/lib64','lib','usr/lib'))
        result=subprocess.run([str(loader),'--inhibit-cache','--library-path',search,'--list',str(source)],capture_output=True,timeout=5,env={'PATH':'/usr/bin:/bin','LC_ALL':'C'})
        if result.returncode or result.stderr or len(result.stdout)>65536:raise ValueError('unresolved cached-base Mail dependency')
        for line in result.stdout.decode('ascii').splitlines():
            part=line.split('=>')[-1].strip().split(' ',1)[0]
            if not part.startswith('/'):continue
            path=Path(part)
            if path.is_relative_to(MAIL_BASE_ROOT):path=Path('/')/path.relative_to(MAIL_BASE_ROOT)
            elif path!=interp:raise ValueError('Mail dependency resolved outside exact cached base')
            if not library(path):raise ValueError('foreign Mail shared library')
            canonical,dependency=mail_source(path,links);files[canonical]=dependency
    if len(files)+1>MAIL_MAX_FILES:raise ValueError('Mail closure file bound')
    total=0
    for dest,source in sorted(files.items()):
        metadata=source.stat();total+=metadata.st_size
        if total>MAIL_MAX_BYTES:raise ValueError('Mail closure byte bound')
        target=root/str(dest).lstrip('/');target.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(source,target);target.chmod(0o555 if dest.name=='webdock-mail' else stat.S_IMODE(metadata.st_mode)&0o755)
        source_receipts[str(dest)]={'bytes':metadata.st_size,'sha256':hashlib.sha256(source.read_bytes()).hexdigest(),'source':str(source) if source==binary else '/cached-base/'+str(dest).lstrip('/')}
    for alias,target in sorted(links.items(),key=lambda row:len(row[0].parts),reverse=True):
        path=root/str(alias).lstrip('/');path.parent.mkdir(parents=True,exist_ok=True)
        if path.exists() or path.is_symlink():raise ValueError('Mail alias collision')
        path.symlink_to(target)
    count=sum(1 for _ in root.rglob('*'))+1
    if count>MAIL_MAX_INODES:raise ValueError('Mail closure inode bound')
    manifest={'scope':'exact current PrivateMail worker plus finite stage/recovery tools and actual loader/libraries from read-only cached BASE','bytes':total,'fileCount':len(files)+1,'inodeCount':count,'maxBytes':MAIL_MAX_BYTES,'maxFiles':MAIL_MAX_FILES,'maxInodes':MAIL_MAX_INODES,'requiredCommands':MAIL_TOOLS,'files':source_receipts,'symlinks':{str(k):v for k,v in sorted(links.items())},'directories':mail_directories(root)}
    (root/'mail-files.json').write_text(json.dumps(manifest,sort_keys=True)+'\n')

def mail_directories(root):
    paths=[Path(root),*(p for p in Path(root).rglob('*') if p.is_dir() and not p.is_symlink())]
    return {('/' if p==Path(root) else '/'+p.relative_to(root).as_posix()):{'uid':p.stat().st_uid,'gid':p.stat().st_gid,'mode':oct(stat.S_IMODE(p.stat().st_mode))} for p in paths}

def smoke_file_plumbing(root,stage_file,invocation,fixed_path):
    root=Path(root);stage_file=Path(stage_file)
    stage=stage_file.read_bytes()
    if len(stage)>4096:raise ValueError('retained Agent stage byte bound')
    if hashlib.sha256(stage).hexdigest()!='3a2473c468628a2f45b98dd148810adabb1ccee604b5921af2d827ce76ad5099':raise ValueError('retained Agent stage bytes changed')
    for path in (root,root/'run',root/'run/webdock'):
        metadata=path.lstat()
        if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid!=0 or stat.S_IMODE(metadata.st_mode)!=0o755:raise ValueError('image mount parent contract')
    source=root/'source';private=root/'private';target=root/'run/webdock';results={}
    if source.exists() or source.is_symlink() or private.exists() or private.is_symlink() or list(target.iterdir()):raise ValueError('smoke fixture collision')
    environment={'PATH':fixed_path,'LC_ALL':'C','WEBDOCK_RECOVERY_COMPONENT_TEST':'1'}
    try:
        source.mkdir(mode=0o750);os.chown(source,0,65532);source.chmod(0o750)
        private.mkdir(mode=0o770);os.chown(private,0,65532);private.chmod(0o770)
        for name in ('runtime.json','manifest.json','runtime.crt','runtime.key','core-ca.crt','storage-ca.crt'):
            path=source/name;path.write_bytes(b'{');os.chown(path,0,65532);path.chmod(0o440)
        staged=subprocess.run([str(invocation),'--userspec=65532:65532',str(root),'/bin/sh','-ec',stage.decode('ascii')],capture_output=True,timeout=5,env=environment)
        if staged.returncode or staged.stdout or staged.stderr:raise ValueError('retained StageScript smoke failed')
        staged_file=private/'runtime/runtime.json';identity=staged_file.stat();target.rmdir()
        # BuildKit needs no SYS_ADMIN: rename preserves the produced inode, ownership and mode.
        # The separate owned micro-repro qualifies real bind mapping; this gate is relocation.
        private.rename(target)
        actual=(target/'runtime/runtime.json').stat()
        if (actual.st_dev,actual.st_ino,actual.st_uid,actual.st_gid,stat.S_IMODE(actual.st_mode),actual.st_nlink)!=(identity.st_dev,identity.st_ino,65532,65532,0o600,1):raise ValueError('stage relocation inode contract')
        command=[str(invocation),'--userspec=65532:65532',str(root),'/usr/local/bin/webdock-mail','/run/webdock/runtime/runtime.json']
        positive=subprocess.run(command,capture_output=True,timeout=5,env=environment)
        expected=b'mail_startup_stage stage=args\nmail_startup_stage stage=config_read\nmail_startup_stage stage=config_decode\nmail_plugin_start_failed\n'
        if positive.returncode!=1 or positive.stdout or positive.stderr!=expected:raise ValueError('positive real-reader plumbing smoke failed')
        results['positive']={'uid':65532,'gid':65532,'reachedConfigDecode':True,'noReadMetadata':True,'intentionalInvalidPublicJSON':True,'mapping':'same-filesystem rename relocation, NOT bind','sameDevInodeMode':True}
        file=target/'runtime/runtime.json';file.chmod(0o640)
        negative=subprocess.run(command,capture_output=True,timeout=5,env=environment)
        if negative.returncode!=1 or negative.stdout or b'stage=config_decode' in negative.stderr or b'"error_kind":"non_io"' not in negative.stderr:raise ValueError('wrong private mode no longer denied')
        results['wrongModeDenied']=True;file.chmod(0o600);saved=file.with_name('runtime-public-dummy');file.rename(saved);file.symlink_to(saved.name)
        negative=subprocess.run(command,capture_output=True,timeout=5,env=environment)
        if negative.returncode!=1 or negative.stdout or b'stage=config_decode' in negative.stderr or b'"errno":40' not in negative.stderr:raise ValueError('symlink no longer denied')
        results['symlinkDenied']=True
    finally:
        for path in (source,private,target):
            if path.is_symlink():raise ValueError('smoke tree became symlink')
            if path.exists():shutil.rmtree(path)
        target.mkdir(mode=0o755);os.chown(target,0,0);target.chmod(0o755)
    if source.exists() or private.exists() or list(target.iterdir()):raise ValueError('smoke material retained in final image')
    results['dummyFilesRemoved']=True;results['stageScriptSHA256']=hashlib.sha256(stage).hexdigest()
    return results

def smoke_system_dns(root,probe,invocation,fixed_path):
    root=Path(root);probe=Path(probe);etc=root/'etc';tool=root/'mail-dns-probe'
    if probe.is_symlink() or not probe.is_file() or hashlib.sha256(probe.read_bytes()).hexdigest()!=MAIL_DNS_PROBE_SHA256:raise ValueError('actual DNS test-tool identity')
    metadata=etc.lstat()
    if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid!=0 or metadata.st_gid!=0 or stat.S_IMODE(metadata.st_mode)!=0o755 or list(etc.iterdir()) or tool.exists() or tool.is_symlink():raise ValueError('fresh image DNS parent required')
    files={'resolv.conf':b'nameserver 192.0.2.53\noptions attempts:1 timeout:1\n','hosts':b'127.0.0.1 localhost\n::1 localhost\n','hostname':b'owned-mail-constructor\n'}
    cases={};environment={'PATH':fixed_path,'LC_ALL':'C','WEBDOCK_RECOVERY_COMPONENT_TEST':'1'}
    try:
        shutil.copyfile(probe,tool);tool.chmod(0o555)
        for name,data in files.items():
            path=etc/name;path.write_bytes(data);path.chmod(0o644)
        command=[str(invocation),'--userspec=65532:65532',str(root),'/mail-dns-probe','--exact','system_dns_probe::constructor_only','--ignored','--nocapture','--test-threads=1']
        for case,expected in (('valid',True),('malformed',False),('missing',False),('inaccessible',False)):
            if case=='malformed':(etc/'resolv.conf').write_bytes(b'nameserver not-an-ip\n')
            if case=='missing':(etc/'resolv.conf').unlink()
            if case=='inaccessible':(etc/'resolv.conf').write_bytes(files['resolv.conf']);(etc/'resolv.conf').chmod(0o644);etc.chmod(0o700)
            result=subprocess.run(command,capture_output=True,timeout=5,env={**environment,'WEBDOCK_DNS_CONSTRUCTOR_EXPECT':'ok' if expected else 'deny'})
            rows=re.findall(rb'(?:^|\.\.\. )mail_dns_constructor (\{[^\n]+\})',result.stdout,re.M)
            if result.returncode or result.stderr or len(result.stdout)>4096 or len(rows)!=1:raise ValueError('actual resolver constructor smoke failed')
            row=json.loads(rows[0]);wanted={'constructorOk':expected,'readable':case not in ('missing','inaccessible'),'readErrno':2 if case=='missing' else 13 if case=='inaccessible' else None}
            if row!=wanted:raise ValueError('resolver constructor strict result mismatch')
            cases[case]=row
    finally:
        etc.chmod(0o755)
        for path in (tool,*(etc/name for name in files)):
            if path.is_symlink():raise ValueError('DNS smoke fixture became symlink')
            if path.exists():path.unlink()
    if tool.exists() or list(etc.iterdir()):raise ValueError('public DNS smoke files retained')
    return {'uid':65532,'gid':65532,'probeSHA256':MAIL_DNS_PROBE_SHA256,'constructorOnlyNoQuery':True,'cases':cases,'dummyFilesAndProbeRemoved':True}

def builder_tool_receipt(name,invocation,canonical,metadata):
    targets={'chroot':{Path('/lib/cargo/bin/coreutils/chroot'),Path('/usr/lib/cargo/bin/coreutils/chroot')},'python3':{Path('/usr/bin/python3.14')},'ldd':{Path('/usr/bin/ldd')}}
    invocations={'chroot':{Path(p)/'chroot' for p in ('/bin','/sbin','/usr/bin','/usr/sbin')},'python3':{Path('/usr/bin/python3'),Path('/usr/bin/python3.14')},'ldd':{Path('/bin/ldd'),Path('/usr/bin/ldd')}}
    canonical_ok=canonical in targets.get(name,set());invocation_ok=invocation in invocations.get(name,set())
    criteria={'canonical':canonical_ok,'invocation':invocation_ok,'regular':metadata is not None and stat.S_ISREG(metadata.st_mode),'ownerRoot':metadata is not None and metadata.st_uid==0,'no022':metadata is not None and not metadata.st_mode&0o022,'executable':metadata is not None and bool(metadata.st_mode&0o111)}
    return {'tool':name,'invocation':str(invocation) if invocation_ok else None,'canonical':str(canonical) if canonical_ok else None,'criteria':criteria,'uid':metadata.st_uid if metadata is not None else None,'gid':metadata.st_gid if metadata is not None else None,'mode':oct(stat.S_IMODE(metadata.st_mode)) if metadata is not None else None,'bytes':metadata.st_size if metadata is not None else None,'passed':all(criteria.values())}

def emit_builder_tool(name,invocation):
    canonical=None;metadata=None
    if invocation is not None:
        invocation=Path(invocation)
        try:
            canonical=invocation.resolve(strict=True)
            if builder_tool_receipt(name,invocation,canonical,None)['criteria']['canonical']:metadata=canonical.stat()
        except OSError:pass
    receipt=builder_tool_receipt(name,invocation,canonical,metadata)
    print('MAIL_BUILDER_TOOL_RECEIPT '+json.dumps(receipt,sort_keys=True),flush=True)
    if not receipt['passed']:raise ValueError('build '+name+' custody')
    return invocation

def smoke_mail(root,stage_file,dns_probe):
    fixed_path='/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin'
    invocation=emit_builder_tool('chroot',shutil.which('chroot',path=fixed_path))
    for command in [('/bin/sh','-ec',':'),*((p,'--version') for p in MAIL_TOOLS if p!='/bin/sh')]:
        result=subprocess.run([str(invocation),'--userspec=65532:65532',str(root),*command],capture_output=True,timeout=5,env={'PATH':fixed_path,'LC_ALL':'C'})
        if result.returncode or result.stderr or len(result.stdout)>65536:raise ValueError('scratch Mail command smoke failed')
    result=subprocess.run([str(invocation),'--userspec=65532:65532',str(root),'/usr/local/bin/webdock-mail','/unconfigured'],capture_output=True,timeout=5,env={'PATH':fixed_path,'LC_ALL':'C'})
    if result.returncode!=1 or result.stdout or result.stderr!=b'mail_plugin_start_failed\n':raise ValueError('scratch Mail worker smoke failed')
    plumbing=smoke_file_plumbing(root,stage_file,invocation,fixed_path)
    dns=smoke_system_dns(root,dns_probe,invocation,fixed_path)
    path=Path(root)/'mail-files.json';manifest=json.loads(path.read_bytes());manifest['filePlumbingSmoke']=plumbing;manifest['systemDnsSmoke']=dns;manifest['directories']=mail_directories(root);manifest['inodeCount']=sum(1 for _ in Path(root).rglob('*'))
    manifest['scratchBuildSmoke']={'uid':65532,'gid':65532,'commands':list(MAIL_TOOLS),'workerUnconfiguredExit':1,'workerStaticError':'mail_plugin_start_failed'};path.write_text(json.dumps(manifest,sort_keys=True)+'\n')

if __name__=='__main__':
    emit_builder_tool('python3',Path(sys.executable))
    print('MAIL_BUILDER_TOOL_RECEIPT '+json.dumps({'tool':'readelf','used':False,'contract':'bounded internal ELF header parser'}),flush=True)
    if len(sys.argv)==6 and sys.argv[3]=='mail' and sys.argv[4]=='/tmp/mail-stage.sh' and sys.argv[5]=='/tmp/mail-dns-probe':
        build_mail(sys.argv[1],sys.argv[2]);smoke_mail(sys.argv[2],sys.argv[4],sys.argv[5])
    elif len(sys.argv)==3:build(sys.argv[1],sys.argv[2])
    else:raise ValueError('only gateway or finite Mail closure profiles are supported')
