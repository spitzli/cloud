#!/usr/bin/env python3
"""Outbound, single-slot isolated build worker. Customer execution occurs only in QEMU/KVM."""
import contextlib
import datetime
import fcntl
import hashlib
import gzip
import urllib.parse
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import resource
import selectors
import shutil
import signal
import stat
import subprocess
import tarfile
import tempfile
import time
import urllib.request

MAX_SOURCE=128*1024*1024
MAX_OUTPUT=512*1024*1024
MAX_LOG=24000
_orphan_cursors={}
ID=re.compile(r'[1-9][0-9]{0,18}')
SHA=re.compile(r'[0-9a-f]{40}')
DIGEST=re.compile(r'sha256:[0-9a-f]{64}')

class Rejected(Exception):pass
class PublicationUncertain(Rejected):pass
class LeaseLost(Exception):pass

class BuildFailed(Rejected):
    def __init__(self,code,logs=b''):
        super().__init__(code)
        self.logs=(logs.decode('utf-8','ignore') if isinstance(logs,bytes) else logs).encode()[:MAX_LOG].decode('utf-8','ignore')


def redact_logs(logs,environment):
    text=logs.decode('utf-8','ignore') if isinstance(logs,bytes) else logs
    for value in sorted((v for v in environment.values() if isinstance(v,str) and v),key=len,reverse=True):
        text=text.replace(value,'[redacted]')
        # A bounded pipe/file read may stop partway through a printed secret.
        for length in range(min(len(value)-1,len(text)),0,-1):
            if text.endswith(value[:length]):
                text=text[:-length]+'[redacted]';break
    return text.encode()[:MAX_LOG].decode('utf-8','ignore')


def safe_path(name):
    p=PurePosixPath(name)
    if not name or '\\' in name or '\0' in name or p.is_absolute() or '..' in p.parts or len(name)>512:
        raise Rejected('unsafe_path')
    return p


def unpack(data,destination,limit=MAX_OUTPUT):
    """Never delegate path handling to tar extraction, and never accept links/devices."""
    count=0;total=0;seen=set()
    try:
        # Reject compressed/PAX/GNU extension headers before tarfile can expand them.
        offset=0
        while offset+512<=len(data) and any(data[offset:offset+512]):
            member=tarfile.TarInfo.frombuf(data[offset:offset+512], 'utf-8', 'strict')
            if member.type in (tarfile.XHDTYPE,tarfile.XGLTYPE) and member.size>65536:raise Rejected('unsafe_archive')
            if not (member.type in (tarfile.REGTYPE, tarfile.AREGTYPE, tarfile.DIRTYPE,tarfile.XHDTYPE,tarfile.XGLTYPE)) or member.size<0 or member.size>limit or offset+512+member.size>len(data):raise Rejected('unsafe_archive')
            offset+=512+((member.size+511)//512)*512
        if any(data[offset:]):raise Rejected('unsafe_archive')
        with tarfile.open(fileobj=io.BytesIO(data),mode='r:') as archive:
            for member in archive:
                rel=safe_path(member.name)
                if str(rel) in seen or member.issparse() or not (member.isdir() or member.isfile()):raise Rejected('unsafe_archive')
                seen.add(str(rel));count+=1;total+=member.size
                if count>50000 or total>limit or member.size<0:raise Rejected('artifact_limit')
                target=destination.joinpath(*rel.parts)
                if member.isdir():target.mkdir(parents=True,exist_ok=True);continue
                target.parent.mkdir(parents=True,exist_ok=True)
                with target.open('xb') as out, archive.extractfile(member) as source:
                    shutil.copyfileobj(source,out,1024*1024)
                # Private staging parents protect host data; preserve runtime read/execute bits.
                target.chmod(member.mode & 0o777)
    except (tarfile.TarError,OSError,ValueError,UnicodeError) as error:
        raise Rejected('unsafe_archive') from error


def tree_size(root):
    total=0;count=0
    for path in root.rglob('*'):
        mode=path.lstat().st_mode
        if not (stat.S_ISREG(mode) or stat.S_ISDIR(mode)):raise Rejected('unsafe_artifact')
        total+=path.stat().st_size if path.is_file() else 0;count+=1
        if total>MAX_OUTPUT or count>50000:raise Rejected('artifact_limit')
    return total


def pack(root, *, format=tarfile.USTAR_FORMAT):
    tree_size(root)
    buf=io.BytesIO()
    with tarfile.open(fileobj=buf,mode='w',format=format) as out:
        for path in sorted(root.rglob('*')):
            out.add(path,arcname=path.relative_to(root),recursive=False)
    if buf.tell()>MAX_OUTPUT:raise Rejected('artifact_limit')
    return buf.getvalue()


def private_file(path):
    p=Path(path);info=p.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_uid!=0 or info.st_mode & 0o022:raise Rejected('untrusted_config')
    return p


def validate_config(config):
    try:
        if not re.fullmatch(r'[A-Z]{2}',config['country']) or (config['country']=='ZZ' and not re.search(r'\bunverified\b',config['locationEvidence'],re.I)) or config['isolation'] not in ('qemu-kvm','artifact-only') or not config['locationEvidence'] or not config['isolationEvidence'] or not config['storageEvidence']:
            raise Rejected('setup_required')
        if not re.fullmatch(r'https://[^/?#]+(?:/[^?#]*)?',config['controlURL']):raise Rejected('setup_required')
        if config['isolation']=='artifact-only':
            private_file(config['credentialFile'])
            store=Path(config['artifactRoot'])
            if not store.is_absolute() or store.is_symlink() or store.stat().st_mode & 0o077:raise Rejected('unsafe_store')
            return
        if not re.fullmatch(r'[0-9a-f]{64}',config['imageSHA256']):raise Rejected('setup_required')
        if not Path('/dev/kvm').exists():raise Rejected('kvm_required')
        base=private_file(config['image'])
        digest=hashlib.file_digest(base.open('rb'),'sha256').hexdigest()
        if digest!=config['imageSHA256']:raise Rejected('image_mismatch')
        private_file(config['credentialFile'])
        store=Path(config['artifactRoot'])
        if not store.is_absolute() or store.is_symlink() or store.stat().st_mode & 0o077:raise Rejected('unsafe_store')
    except (KeyError,OSError,TypeError) as error:raise Rejected('setup_required') from error


def guest_request(job):
    try:
        for field in ('buildID','customerID','projectID'):
            if not ID.fullmatch(job[field]):raise Rejected('invalid_job')
        if not SHA.fullmatch(job['sourceSHA']) or type(job['generation']) is not int or job['generation']<1:raise Rejected('invalid_job')
        recipe=job['recipe']
        if recipe not in ('dockerfile','vercel'):raise Rejected('invalid_recipe')
        root=str(safe_path(job.get('rootDirectory','.')))
        limits=job['limits']
        for field,low,high in [('cpu',1,8),('memoryMiB',512,16384),('diskBytes',10000000,32*1024**3),('durationSeconds',30,1800)]:
            if type(limits[field]) is not int or not low<=limits[field]<=high:raise Rejected('invalid_limits')
        values=job.get('buildEnvironment',{})
        if not isinstance(values,dict) or len(values)>32:raise Rejected('invalid_environment')
        for key,value in values.items():
            if not re.fullmatch(r'[A-Za-z_][A-Za-z0-9_]{0,127}',key) or key.startswith(('VERCEL_','AWS_','SPITZLI_','DOCKER_','GIT_')) or key in ('PATH','HOME','NODE_OPTIONS','LD_PRELOAD','PYTHONPATH') or not isinstance(value,str) or '\0' in value or len(value)>4096:raise Rejected('invalid_environment')
        if len(json.dumps(values))>16000:raise Rejected('invalid_environment')
        result={'recipe':recipe,'rootDirectory':root,'buildEnvironment':values,'limits':limits}
        if recipe=='vercel':
            settings=job['vercelSettings']
            if not re.fullmatch(r'prj_[A-Za-z0-9]+',settings['projectId']) or not re.fullmatch(r'(team|user)_[A-Za-z0-9]+',settings['orgId']):raise Rejected('invalid_target')
            result['vercelSettings']={key:settings[key] for key in ('projectId','orgId','settings')}
            if len(json.dumps(result['vercelSettings']))>16000:raise Rejected('invalid_target')
        return result
    except (KeyError,TypeError,ValueError) as error:raise Rejected('invalid_job') from error


def run_bounded(command,*,timeout,max_bytes=MAX_LOG,cwd=None,env=None,file_limit=MAX_OUTPUT,lease_check=None,poll_interval=5):
    """Bound memory, output file growth, elapsed time, and all child processes."""
    def limits():resource.setrlimit(resource.RLIMIT_FSIZE,(file_limit,file_limit))
    proc=subprocess.Popen(command,cwd=cwd,env=env or {'PATH':'/usr/bin:/bin','LANG':'C.UTF-8'},stdout=subprocess.PIPE,stderr=subprocess.STDOUT,start_new_session=True,preexec_fn=limits)
    output=bytearray();deadline=time.monotonic()+timeout;next_poll=time.monotonic()+poll_interval
    def check_lease():
        if lease_check is not None:
            try:lease_check()
            except Exception:raise LeaseLost("build_lease_lost") from None
    try:
        with selectors.DefaultSelector() as selector:
            selector.register(proc.stdout,selectors.EVENT_READ)
            while selector.get_map() or proc.poll() is None:
                if lease_check is not None and time.monotonic()>=next_poll:
                    check_lease();next_poll=time.monotonic()+poll_interval
                if time.monotonic()>=deadline:raise Rejected('execution_timeout')
                for key,_ in selector.select(min(0.2,max(0,deadline-time.monotonic()))):
                    chunk=os.read(key.fileobj.fileno(),65536)
                    if not chunk:selector.unregister(key.fileobj);continue
                    output.extend(chunk)
                    if len(output)>max_bytes:raise Rejected('log_limit')
        if proc.wait(timeout=max(0.01,deadline-time.monotonic()))!=0:raise Rejected('execution_failed')
        check_lease()
        return bytes(output)
    except (Rejected,subprocess.TimeoutExpired) as error:
        code=str(error) if isinstance(error,Rejected) else 'execution_timeout'
        raise BuildFailed(code,bytes(output[:max_bytes])) from None
    finally:
        with contextlib.suppress(ProcessLookupError):os.killpg(proc.pid,signal.SIGKILL)
        proc.wait();proc.stdout.close()


# Input/source, result unpack, nested OCI validation, and final artifact copies.
BUILD_STAGING_RESERVE=5*MAX_OUTPUT+2*MAX_SOURCE


def execution_disk_limit(image,disk_bytes,allocation_block=4096):
    info=Path(image).lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_size<=0:raise Rejected('invalid_image')
    # The root drive's snapshot has one overlay and the serial result is the only other writable QEMU file.
    # RLIMIT_FSIZE bounds BOTH, including a malicious guest flooding its result channel.
    metadata=max(64*1024**2,(info.st_size+31)//32)
    file_limit=max(MAX_OUTPUT,info.st_size+metadata)
    if type(allocation_block) is not int or allocation_block<1:raise Rejected('invalid_filesystem')
    directory_overhead=4*50000*allocation_block
    if 2*file_limit+BUILD_STAGING_RESERVE+directory_overhead>disk_bytes:raise Rejected('image_exceeds_disk_budget')
    return file_limit


def qemu_command(image,input_tar,output_tar,limits,egress=False):
    network=['-netdev','user,id=net0,restrict=on,guestfwd=tcp:10.0.2.100:3128-tcp:127.0.0.1:3128','-device','virtio-net-pci,netdev=net0'] if egress else ['-nic','none']
    return ['/usr/bin/qemu-system-x86_64','-enable-kvm','-machine','q35','-cpu','host','-smp',str(limits['cpu']),'-m',str(limits['memoryMiB']),
        '-nodefaults','-no-reboot','-display','none','-monitor','none','-serial','none',*network,
        '-drive',f'file={image},format=raw,if=none,id=root,readonly=off,snapshot=on',
        '-device','virtio-blk-pci,drive=root,bootindex=1',
        '-drive',f'file={input_tar},format=raw,if=none,id=input,readonly=on',
        '-device','virtio-blk-pci,drive=input,serial=webdock-input',
        '-device','virtio-serial-pci','-chardev',f'file,id=result,path={output_tar}',
        '-device','virtserialport,chardev=result,name=webdock.result']


def execute_vm(config,job,source,lease_check=None):
    request=guest_request(job)
    file_limit=execution_disk_limit(config['image'],request['limits']['diskBytes'],os.statvfs(tempfile.gettempdir()).f_frsize)
    if shutil.disk_usage(tempfile.gettempdir()).free<request['limits']['diskBytes'] or shutil.disk_usage(config['artifactRoot']).free<MAX_OUTPUT:raise Rejected('insufficient_disk_space')
    egress=config.get('egress')=='loopback-connect-proxy' and bool(config.get('egressEvidence'))
    if egress:request['egressProxy']='http://10.0.2.100:3128'
    with tempfile.TemporaryDirectory(prefix='wd-build-') as directory:
        temp=Path(directory);checkout=temp/'source';checkout.mkdir()
        unpack(source,checkout,MAX_SOURCE)
        # Source endpoint returns normalized source, without the GitHub tarball wrapper.
        if not checkout.joinpath(request['rootDirectory']).is_dir():raise Rejected('missing_root')
        (temp/'request').mkdir();shutil.move(checkout,temp/'request/source')
        (temp/'request/job.json').write_text(json.dumps(request))
        input_tar=temp/'input.tar';input_tar.write_bytes(pack(temp/'request', format=tarfile.PAX_FORMAT))
        output_tar=temp/'output.tar'
        run_bounded(qemu_command(config['image'],input_tar,output_tar,request['limits'],egress),timeout=request['limits']['durationSeconds'],file_limit=file_limit,env={'PATH':'/usr/bin:/bin','TMPDIR':str(temp)},lease_check=lease_check)
        if not output_tar.exists() or output_tar.stat().st_size>MAX_OUTPUT:raise Rejected('artifact_limit')
        result=temp/'result';result.mkdir();unpack(output_tar.read_bytes(),result)
        status=json.loads((result/'status.json').read_text())
        logs=(result/'build.log').read_bytes()[:MAX_LOG].decode('utf-8','replace') if (result/'build.log').exists() else ''
        logs=redact_logs(logs,request['buildEnvironment'])
        if status!={'success':True}:raise BuildFailed('build_failed',logs)
        output=result/'artifact'
        if request['recipe']=='vercel':validate_vercel(output)
        else:validate_oci(output/'image.tar')
        artifact=pack(output);digest='sha256:'+hashlib.sha256(artifact).hexdigest()
        key=f"{job['customerID']}/{job['buildID']}/{job['generation']}/{digest[7:]}.tar"
        store=Path(config['artifactRoot'])
        maximum=config.get('maxStoreBytes',20*1024**3)
        if type(maximum) is not int or maximum<MAX_OUTPUT or maximum>200*1024**3:raise Rejected('invalid_storage_limit')
        used=sum(path.stat().st_size for path in store.rglob('*') if path.is_file())
        if used+len(artifact)>maximum:raise Rejected('artifact_store_full')
        destination=store/key;destination.parent.mkdir(parents=True,exist_ok=True,mode=0o700)
        with destination.open('xb') as out:out.write(artifact)
        return {'kind':'oci' if request['recipe']=='dockerfile' else 'vercel','digest':digest,'storageKey':key,'sizeBytes':len(artifact)},logs


def validate_oci(path):
    if not path.is_file() or path.stat().st_size>MAX_OUTPUT:raise Rejected('invalid_oci')
    with tempfile.TemporaryDirectory() as directory:
        root=Path(directory);unpack(path.read_bytes(),root)
        if json.loads((root/'oci-layout').read_text())!={'imageLayoutVersion':'1.0.0'}:raise Rejected('invalid_oci')
        index=json.loads((root/'index.json').read_text())
        if index.get('schemaVersion')!=2 or len(index.get('manifests',[]))!=1:raise Rejected('invalid_oci')
        # Verify every descriptor recursively, including layer/config sizes and digests.
        visited=set()
        def check(descriptor):
            digest=descriptor.get('digest','')
            if not DIGEST.fullmatch(digest) or type(descriptor.get('size')) is not int:raise Rejected('invalid_oci')
            path=root/'blobs/sha256'/digest[7:]
            if not path.is_file() or path.stat().st_size!=descriptor['size'] or hashlib.sha256(path.read_bytes()).hexdigest()!=digest[7:]:raise Rejected('invalid_oci')
            if digest in visited:return
            visited.add(digest)
            media=descriptor.get('mediaType','')
            if media in ('application/vnd.oci.image.manifest.v1+json','application/vnd.docker.distribution.manifest.v2+json'):
                manifest=json.loads(path.read_text());check(manifest['config'])
                for layer in manifest['layers']:check(layer)
            elif media not in ('application/vnd.oci.image.config.v1+json','application/vnd.oci.image.layer.v1.tar','application/vnd.oci.image.layer.v1.tar+gzip','application/vnd.oci.image.layer.v1.tar+zstd','application/vnd.docker.container.image.v1+json','application/vnd.docker.image.rootfs.diff.tar.gzip'):raise Rejected('invalid_oci')
        check(index['manifests'][0])
        return index['manifests'][0]['digest']


def validate_vercel(output):
    tree_size(output)
    config=json.loads((output/'config.json').read_text())
    if config.get('version')!=3 or set(config)-{'version','routes','images','wildcard','overrides','cache','crons','regions','framework'}:raise Rejected('invalid_vercel_output')
    framework=config.get('framework')
    if framework is not None and (not isinstance(framework,dict) or set(framework)-{'slug','version'} or any(not isinstance(v,str) or len(v)>200 for v in framework.values())):raise Rejected('invalid_vercel_output')
    for key,value in config.get('overrides',{}).items():
        safe_path(key)
        if 'path' in value:safe_path(value['path'])
    for item in config.get('cache',[]):safe_path(item)
    # Edge execution cannot satisfy verified Frankfurt functions.
    for path in output.rglob('.vc-config.json'):
        value=json.loads(path.read_text())
        allowed={'runtime','handler','launcherType','shouldAddHelpers','shouldAddSourceMapSupport','shouldAddSourcemapSupport','shouldDisableAutomaticFetchInstrumentation','awsLambdaHandler','regions','memory','maxDuration','environment','filePathMap','architecture','experimentalResponseStreaming','supportsResponseStreaming','operationType','supportsMultiPayloads','framework','experimentalAllowBundling'}
        if not isinstance(value,dict) or set(value)-allowed:raise Rejected('unsupported_function_config')
        if value.get('operationType','API') not in ('API','Page','ISR'):raise Rejected('invalid_function_config')
        if value.get('experimentalAllowBundling',False) is not False:raise Rejected('invalid_function_config')
        framework=value.get('framework')
        if framework is not None and (not isinstance(framework,dict) or set(framework)-{'slug','version'} or any(not isinstance(v,str) or len(v)>200 for v in framework.values())):raise Rejected('invalid_function_config')
        for flag in ('shouldAddSourcemapSupport','shouldDisableAutomaticFetchInstrumentation','supportsMultiPayloads'):
            if flag in value and not isinstance(value[flag],bool):raise Rejected('invalid_function_config')
        if 'handler' in value:safe_path(value['handler'])
        mapping=value.get('filePathMap',{})
        if not isinstance(mapping,dict):raise Rejected('invalid_function_config')
        for origin,target in mapping.items():safe_path(origin);safe_path(target)
        if value.get('runtime') not in ('nodejs24.x','nodejs22.x','python3.12','python3.13') or value.get('regions',['fra1'])!=['fra1']:raise Rejected('function_region_required')
    for path in output.iterdir():
        if path.name not in ('config.json','static','functions','diagnostics','builds.json'):raise Rejected('invalid_vercel_output')
    if config.get('regions', ['fra1'])!=['fra1']:raise Rejected('function_region_required')
    metadata=output/'builds.json'
    if metadata.exists():
        if metadata.stat().st_size>1024*1024:raise Rejected('invalid_build_metadata')
        build=json.loads(metadata.read_text())
        if not isinstance(build,dict) or build.get('target')!='production' or build.get('error') or not isinstance(build.get('builds',[]),list):raise Rejected('invalid_build_metadata')
        if any(not isinstance(item,dict) or item.get('error') for item in build.get('builds',[])):raise Rejected('invalid_build_metadata')


@contextlib.contextmanager
def vercel_workspace(output,settings):
    validate_vercel(output)
    with tempfile.TemporaryDirectory(prefix='wd-publish-') as directory:
        root=Path(directory);(root/'.vercel').mkdir()
        shutil.copytree(output,root/'.vercel/output')
        # Pinned CLI reads build target/errors/framework from this metadata. Never forward
        # guest argv, absolute paths or builder configuration into trusted publication.
        if (output/'builds.json').exists():
            (root/'.vercel/output/builds.json').write_text(json.dumps({'target':'production','builds':[]}))
        (root/'.vercel/project.json').write_text(json.dumps({'projectId':settings['projectId'],'orgId':settings['orgId']}))
        (root/'vercel.json').write_text('{"regions":["fra1"]}')
        yield root


class Control:
    def __init__(self,config):
        self.base=config['controlURL'].rstrip('/');self.token=Path(config['credentialFile']).read_text().strip()
    def request(self,path,body=None,binary=False,timeout=30):
        request=urllib.request.Request(self.base+path,data=json.dumps(body).encode() if body is not None else None,headers={'Authorization':'Bearer '+self.token,'Content-Type':'application/json'})
        # Redirects must never forward enrollment credentials to a different origin.
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self,*args,**kwargs):return None
        with urllib.request.build_opener(NoRedirect).open(request,timeout=timeout) as response:
            value=response.read((MAX_SOURCE if binary else 1024*1024)+1)
        if len(value)>(MAX_SOURCE if binary else 1024*1024):raise Rejected('response_limit')
        return value if binary else json.loads(value)



def download_source(descriptor,job):
    if descriptor['sha']!=job['sourceSHA'] or str(descriptor['repositoryID'])!=str(job['repositoryID']):raise Rejected('source_identity_mismatch')
    url=urllib.parse.urlsplit(descriptor['url'])
    if url.scheme!='https' or url.hostname!='codeload.github.com' or url.port not in (None,443) or url.username or url.password or url.fragment:raise Rejected('source_origin_denied')
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self,*args,**kwargs):return None
    with urllib.request.build_opener(NoRedirect).open(descriptor['url'],timeout=60) as response:compressed=response.read(MAX_SOURCE+1)
    if len(compressed)>MAX_SOURCE:raise Rejected('source_limit')
    with gzip.GzipFile(fileobj=io.BytesIO(compressed)) as archive:data=archive.read(MAX_SOURCE+1)
    if len(data)>MAX_SOURCE:raise Rejected('source_limit')
    with tempfile.TemporaryDirectory(prefix='wd-source-') as directory:
        root=Path(directory);unpack(data,root,MAX_SOURCE)
        children=list(root.iterdir())
        if len(children)!=1 or not children[0].is_dir():raise Rejected('invalid_source_wrapper')
        return pack(children[0])


def run_once(config):
    control=Control(config);job=control.request('/claim',{})
    if not job:return False
    result={'buildID':job['buildID'],'generation':job['generation']}
    try:
        if config.get('isolation')=='artifact-only':
            import actions_import
            if job.get('buildProvider')!='github-actions':raise Rejected('build_provider_denied')
            artifact,logs=actions_import.import_artifact(config,job)
        else:
            if job.get('buildProvider')=='github-actions':raise Rejected('build_provider_denied')
            guest_request(job)
            expires=datetime.datetime.fromisoformat(job['leaseUntil'].replace('Z','+00:00'))
            if (expires-datetime.datetime.now(datetime.timezone.utc)).total_seconds()<job['limits']['durationSeconds']+60:raise Rejected('lease_too_short')
            descriptor=control.request(f"/builds/{job['buildID']}/source?generation={job['generation']}")
            source=download_source(descriptor,job)
            def check_lease():
                try:
                    active=control.request(f"/builds/{job['buildID']}/lease?generation={job['generation']}",timeout=5)
                    if active!={'active':True}:raise LeaseLost('build_lease_lost')
                except Exception:raise LeaseLost('build_lease_lost') from None
            artifact,logs=execute_vm(config,job,source,lease_check=check_lease)
        result.update(status='succeeded',artifact=artifact,logs=logs)
    except LeaseLost:
        return True
    except BuildFailed as error:
        logs=redact_logs(error.logs,job.get('buildEnvironment',{}))
        result.update(status='failed',failureCode='BUILD_FAILED',logs=logs or 'Build execution failed.')
    except Exception:
        result.update(status='failed',failureCode='BUILD_FAILED',logs='Build failed; worker boundary rejected execution or output.')
    control.request('/complete',result);return True


def remove_artifact_keys(config,keys):
    if not isinstance(keys,list) or len(keys)>1000:raise Rejected('invalid_retention')
    root=Path(config['artifactRoot'])
    if not root.is_absolute() or root.lstat().st_mode & 0o077:raise Rejected('unsafe_store')
    purged=[]
    for key in keys:
        if not isinstance(key,str) or not re.fullmatch(r'[1-9][0-9]{0,18}/[1-9][0-9]{0,18}/[1-9][0-9]{0,18}/[0-9a-f]{64}\.tar',key):raise Rejected('invalid_retention')
        handles=[]
        try:
            # Resolve each component without following links; unlink never follows a final link either.
            handle=os.open('/',os.O_RDONLY|os.O_DIRECTORY);handles.append(handle)
            for part in (*root.parts[1:],*key.split('/')[:-1]):
                handle=os.open(part,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=handle);handles.append(handle)
            leaf=key.split('/')[-1]
            info=os.stat(leaf,dir_fd=handle,follow_symlinks=False)
            if not stat.S_ISREG(info.st_mode):raise Rejected('unsafe_artifact')
            os.unlink(leaf,dir_fd=handle)
        except FileNotFoundError:pass
        except OSError as error:raise Rejected('unsafe_artifact') from error
        finally:
            for handle in reversed(handles):os.close(handle)
        purged.append(key)
    return purged


def cleanup_artifacts(config,control):
    response=control.request('/retention',{})
    purged=remove_artifact_keys(config,response.get('storageKeys'))
    if purged:control.request('/retention-complete',{'storageKeys':purged})


def cleanup_orphan_artifacts(config,control):
    root=Path(config['artifactRoot']);keys=[];cutoff=time.time()-24*3600
    cursor=_orphan_cursors.get(str(root),'')
    if not root.is_absolute() or root.lstat().st_mode & 0o077:raise Rejected('unsafe_store')
    def scan(handle,parts):
        with os.scandir(handle) as entries:
            for entry in sorted(entries,key=lambda entry:entry.name):
                if len(keys)>=1000:return
                if len(parts)<3:
                    if not ID.fullmatch(entry.name) or not entry.is_dir(follow_symlinks=False):continue
                    child=os.open(entry.name,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=handle)
                    try:scan(child,[*parts,entry.name])
                    finally:os.close(child)
                elif re.fullmatch(r'[0-9a-f]{64}\.tar',entry.name):
                    info=entry.stat(follow_symlinks=False)
                    key='/'.join([*parts,entry.name])
                    if key>cursor and stat.S_ISREG(info.st_mode) and info.st_mtime<=cutoff:keys.append(key)
    handles=[]
    try:
        handle=os.open('/',os.O_RDONLY|os.O_DIRECTORY);handles.append(handle)
        for part in root.parts[1:]:
            handle=os.open(part,os.O_RDONLY|os.O_DIRECTORY|os.O_NOFOLLOW,dir_fd=handle);handles.append(handle)
        scan(handle,[])
    except OSError as error:raise Rejected('unsafe_store') from error
    finally:
        for handle in reversed(handles):os.close(handle)
    _orphan_cursors[str(root)]=keys[-1] if len(keys)==1000 else ''
    if not keys:return
    response=control.request('/orphan-retention',{'storageKeys':keys})
    approved=response.get('storageKeys')
    if not isinstance(approved,list) or len(approved)>1000 or any(not isinstance(key,str) or key not in keys for key in approved):raise Rejected('invalid_retention')
    remove_artifact_keys(config,approved)


def run_turn(config,prefer_release):
    import publisher
    if prefer_release:
        return publisher.run_once(config,Control(config)) or run_once(config)
    return run_once(config) or publisher.run_once(config,Control(config))


def main():
    import argparse
    parser=argparse.ArgumentParser();parser.add_argument('--config',required=True);parser.add_argument('--once',action='store_true');args=parser.parse_args()
    config=json.loads(private_file(args.config).read_text());validate_config(config)
    with open(Path(config['artifactRoot'])/'.worker.lock','w') as lock:
        fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
        prefer_release=False;next_retention=0
        while True:
            if time.monotonic()>=next_retention:
                try:cleanup_artifacts(config,Control(config))
                except Exception:print('artifact_retention_failed',flush=True)
                try:cleanup_orphan_artifacts(config,Control(config))
                except Exception:print('orphan_retention_failed',flush=True)
                next_retention=time.monotonic()+3600
            try:worked=run_turn(config,prefer_release)
            except Exception:worked=False;print('worker_request_failed',flush=True)
            prefer_release=not prefer_release
            if args.once:return
            if not worked:time.sleep(5)

if __name__=='__main__':main()
