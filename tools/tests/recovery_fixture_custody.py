"""Exact image deletion guards for the disposable native recovery component only."""
from dataclasses import dataclass
import re

RUN_LABEL = 'webdock.recovery-component-run'
KIND_LABEL = 'webdock.recovery-component-kind'
KIND = 'native-recovery-component'

@dataclass(frozen=True)
class ComponentImage:
    run: str
    image_id: str
    @property
    def reference(self):
        return 'wd-owned-recovery-component:' + self.run

@dataclass(frozen=True)
class PauseImage:
    reference: str
    image_id: str
    configuration_id: str
    absent_before: bool
    source_k3s: str
    required_version: str


def _digest(value):
    return isinstance(value,str) and re.fullmatch(r'sha256:[a-f0-9]{64}',value) is not None


def component_delete(receipt, current, protected_ids):
    if not re.fullmatch(r'[a-f0-9]{12}',receipt.run) or not _digest(receipt.image_id):
        raise ValueError('invalid immutable component custody')
    config=current.get('Config') or {}
    labels=config.get('Labels') or {}
    digests=current.get('RepoDigests') or []
    own_digest='wd-owned-recovery-component@'+receipt.image_id
    if (receipt.image_id in protected_ids or current.get('Id')!=receipt.image_id
        or labels.get(RUN_LABEL)!=receipt.run or labels.get(KIND_LABEL)!=KIND
        or current.get('RepoTags')!=[receipt.reference]
        or any(value!=own_digest for value in digests)):
        raise ValueError('component image custody differs; no removal')
    return ('image','rm',receipt.reference)


def pause_delete(receipt, current, current_config_id, protected_ids):
    k3s='rancher/k3s@sha256:057cfd5a9d51b89f1f0ee139c28d5d5fdd5aa7ddeac2742a0ba90c6dbc0329bb'
    repository,separator,digest=receipt.reference.partition('@')
    if (receipt.source_k3s!=k3s or not receipt.absent_before
        or not re.fullmatch(r'rancher/mirrored-pause:[0-9]+\.[0-9]+\.[0-9]+',receipt.required_version)
        or repository!=receipt.required_version.rsplit(':',1)[0] or separator!='@'
        or not _digest(digest) or not _digest(receipt.image_id) or not _digest(receipt.configuration_id)
        or receipt.image_id in protected_ids or current.get('Id')!=receipt.image_id
        or current_config_id!=receipt.configuration_id):
        raise ValueError('pause provenance differs; no removal')
    references=(current.get('RepoTags') or [])+(current.get('RepoDigests') or [])
    if not references or any(value!=receipt.reference for value in references):
        raise ValueError('pause has foreign or ambiguous aliases; no removal')
    return ('image','rm',receipt.reference)

# Concrete runner stages; all command output remains in the private fixture directory.
import hashlib
import json
import os
import subprocess
import traceback
import uuid
import sys
from pathlib import Path

class CommandFailed(RuntimeError):
    pass

def _private_write(path, data, *, exclusive=False):
    path=Path(path)
    with path.open('xb' if exclusive else 'wb') as stream:
        os.chmod(path,0o600)
        stream.write(data);stream.flush();os.fsync(stream.fileno())


def _bytes(value):
    if value is None:return b''
    return value if isinstance(value,bytes) else str(value).encode()


def _evidence_write(path, payload, failures, stage, *, exclusive=True):
    try:
        _private_write(path,payload,exclusive=exclusive)
        return True
    except BaseException as error:
        # Recording is secondary: retain failure in memory without exposing private details.
        failures.append({'stage':stage,'artifact':Path(path).name,'errorType':type(error).__qualname__})
        return False


def _exception_details(error):
    return {'type':type(error).__qualname__,'repr':repr(error),'message':str(error),'traceback':''.join(traceback.format_exception(error))}


def record_exception(directory, stage, error, failures=None):
    failures=[] if failures is None else failures
    identifier=uuid.uuid4().hex
    prefix=stage+'--'+identifier
    files={'exception':prefix+'.exception.json'}
    for attribute,suffix in [('output','stdout'),('stderr','stderr')]:
        value=getattr(error,attribute,None)
        if value is not None:
            files[suffix]=prefix+'.'+suffix
            _evidence_write(Path(directory)/files[suffix],_bytes(value),failures,stage)
    details={**_exception_details(error),'attemptID':identifier,'stage':stage,'files':files}
    payload=json.dumps(details,indent=2).encode()
    if _evidence_write(Path(directory)/files['exception'],payload,failures,stage):
        return hashlib.sha256(payload).hexdigest()
    return None


def secondary_exception(directory, stage, error, failures):
    try:return record_exception(directory,stage,error,failures)
    except BaseException as recording_error:
        failures.append({'stage':stage,'artifact':'exception-record','errorType':type(recording_error).__qualname__})
        return None


class CheckedCommands:
    def __init__(self, invoke, directory):
        self.invoke=invoke
        self.directory=Path(directory)
        self.attempts=[]
        self.evidence_failures=[]
    def call(self, stage, *arguments, check=True, **options):
        identifier=uuid.uuid4().hex
        prefix=stage+'--'+identifier
        attempt={'attemptID':identifier,'stage':stage,'status':'submitted','recorded':False,
                 'argumentsSHA256':hashlib.sha256(json.dumps(arguments).encode()).hexdigest(),
                 'files':{kind:prefix+'.'+suffix for kind,suffix in [('stdout','stdout'),('stderr','stderr'),('metadata','attempt.json')]}}
        self.attempts.append(attempt)
        primary=None;result=None
        try:
            result=self.invoke(*arguments,allowed=True,**options)
            attempt.update({'status':'completed' if result.returncode==0 else 'failed','returnCode':result.returncode})
            stdout=_bytes(result.stdout);stderr=_bytes(result.stderr)
            if check and result.returncode:primary=CommandFailed('owned command exit='+str(result.returncode)+'; private evidence retained')
        except BaseException as error:
            primary=error
            attempt.update({'status':'failed','errorType':type(error).__qualname__})
            stdout=_bytes(getattr(error,'output',None));stderr=_bytes(getattr(error,'stderr',None))
        # Provider outcome is already in memory before any evidence write can fail.
        before=len(self.evidence_failures)
        _evidence_write(self.directory/attempt['files']['stdout'],stdout,self.evidence_failures,stage)
        _evidence_write(self.directory/attempt['files']['stderr'],stderr,self.evidence_failures,stage)
        if primary is not None:
            attempt['files']['exception']=prefix+'.exception.json'
            details={**_exception_details(primary),'attemptID':identifier,'stage':stage,'files':dict(attempt['files'])}
            _evidence_write(self.directory/attempt['files']['exception'],json.dumps(details,indent=2).encode(),self.evidence_failures,stage)
        attempt['recorded']=len(self.evidence_failures)==before
        if not _evidence_write(self.directory/attempt['files']['metadata'],json.dumps(attempt,indent=2).encode(),self.evidence_failures,stage):attempt['recorded']=False
        if primary is not None:raise primary
        return result
    def require_recorded(self):
        if not self.attempts[-1]['recorded']:raise CommandFailed('private evidence unavailable; acquisition or retention closed')
    def exception(self, stage, error):
        return secondary_exception(self.directory,stage,error,self.evidence_failures)


def write_final_report(directory, report, evidence_failures):
    # Set in-memory failure before attempting a possibly impossible durable write.
    if evidence_failures:
        report['passed']=False
        report['cleanup']['complete']=False
    report['recordingFailures']=list(evidence_failures)
    try:_private_write(Path(directory)/'report.json',(json.dumps(report,indent=2)+'\n').encode())
    except BaseException as error:
        evidence_failures.append({'stage':'final-report','artifact':'report.json','errorType':type(error).__qualname__})
        report['passed']=False
        report['cleanup']['complete']=False
        report['recordingFailures']=list(evidence_failures)
        print('Recovery fixture failed: private evidence/report unavailable; cleanup status incomplete.',file=sys.stderr)
        return False
    return report['passed']

class Acquisitions:
    def __init__(self, directory):
        self.path=Path(directory)/'acquisitions.json'
        self.entries={}
    def record(self, kind, value):
        self.entries[kind]=value
        temporary=self.path.with_suffix('.next')
        _private_write(temporary,(json.dumps(self.entries,indent=2)+'\n').encode())
        temporary.replace(self.path)
        descriptor=os.open(self.path.parent,os.O_RDONLY|os.O_DIRECTORY)
        try:os.fsync(descriptor)
        finally:os.close(descriptor)
    def unresolved(self):
        return [kind for kind,value in self.entries.items() if value['state'] in ('pending','verified','retained-obligation')]


def image_probe(reference, result):
    stderr=_bytes(result.stderr).strip()
    expected={('Error response from daemon: No such image: '+reference).encode(),('Error: No such image: '+reference).encode()}
    if result.returncode==1 and _bytes(result.stdout).strip() in (b'',b'[]') and stderr in expected:
        return None
    if result.returncode!=0:raise ValueError('unknown image probe; no acquisition')
    rows=json.loads(result.stdout)
    if not isinstance(rows,list) or len(rows)!=1 or not _digest(rows[0].get('Id')):
        raise ValueError('ambiguous image probe; no acquisition')
    return rows[0]


def acquire_image(kind, reference, action, commands, acquisitions, verify):
    if kind=='component':
        if not re.fullmatch(r'wd-owned-recovery-component:[a-f0-9]{12}',reference) or tuple(action[:3])!=('build','--tag',reference):
            raise ValueError('component action not bound to immutable namespace')
    elif kind=='pause':
        if not re.fullmatch(r'rancher/mirrored-pause@sha256:[a-f0-9]{64}',reference) or tuple(action)!=('pull',reference):
            raise ValueError('pause action not bound to exact reference')
    else:raise ValueError('unsupported fixture acquisition')
    current=image_probe(reference,commands.call(kind+'-before','image','inspect',reference,check=False))
    commands.require_recorded()
    if current is not None:
        if kind=='component':raise ValueError('preexisting component reference; no build or adoption')
        return current,False
    acquisitions.record(kind,{'reference':reference,'state':'pending','before':'exact expected No such image'})
    commands.call(kind+'-acquire',*action,timeout=240 if kind=='component' else 90)
    current=image_probe(reference,commands.call(kind+'-verify','image','inspect',reference))
    if current is None:raise ValueError('acquisition outcome unresolved')
    commands.require_recorded()
    verify(current)
    acquisitions.record(kind,{'reference':reference,'state':'verified','imageID':current['Id']})
    return current,True


def retain_diagnostics(commands, container, directory, capture):
    destination=Path(directory)/'diagnostics'
    if destination.exists():
        destination=Path(directory)/('diagnostics-retry-'+str(len(list(Path(directory).glob('diagnostics*')))))
        if destination.exists():raise ValueError('diagnostic capture namespace collision')
    capture['retained']=False
    try:
        commands.call('copy-diagnostics','cp',container+':/diagnostics',str(destination))
        commands.require_recorded()
        if not destination.is_dir() or destination.is_symlink():raise ValueError('diagnostic directory absent')
        files={};total=0
        for path in sorted(destination.rglob('*')):
            if path.is_symlink():raise ValueError('diagnostic symlink rejected')
            if path.is_dir():continue
            if not path.is_file() or len(files)>=256:raise ValueError('diagnostic file boundary')
            data=path.read_bytes();total+=len(data)
            if total>16*1024*1024:raise ValueError('diagnostic retention byte limit')
            files[path.relative_to(destination).as_posix()]={'bytes':len(data),'sha256':hashlib.sha256(data).hexdigest()}
        if not {'preflight.log','stage'}<=files.keys():raise ValueError('incomplete diagnostic retention')
        if any(not (destination/required).read_bytes().strip() for required in ('preflight.log','stage')):raise ValueError('empty required diagnostic evidence')
        receipt={'container':container,'directory':destination.name,'files':files,'retained':True}
        payload=(json.dumps(receipt,indent=2)+'\n').encode()
        _private_write(Path(directory)/'diagnostic-retention.json',payload)
        capture.update(receipt)
        capture['receiptSHA256']=hashlib.sha256(payload).hexdigest()
        return True
    except BaseException as error:
        capture['status']='partial' if destination.exists() else 'failed'
        capture['errorSHA256']=commands.exception('diagnostic-retention',error)
        raise


def diagnostic_delete(volume, capture, current, owner):
    if (capture.get('retained') is not True or not capture.get('files')
        or not re.fullmatch(r'[a-f0-9]{64}',capture.get('receiptSHA256',''))
        or any(capture.get('files',{}).get(required,{}).get('bytes',0)<=0 for required in ('preflight.log','stage'))):
        raise ValueError('diagnostics not verified; retain private volume obligation')
    if current.get('Labels',{}).get('webdock.recovery-fixture')!=owner:raise ValueError('foreign diagnostics')
    return ('volume','rm',volume)


def wait_and_capture(commands, container, directory, capture, ensure_stopped):
    primary=None;secondary=None;result=None
    try:
        result=commands.call('wait-owned-container','wait',container,timeout=300)
        capture['observedWaitExit']=int(result.stdout.strip())
    except BaseException as error:primary=error
    try:
        ensure_stopped()
        retain_diagnostics(commands,container,directory,capture)
    except BaseException as error:secondary=error
    if secondary is not None:
        capture['secondaryErrorSHA256']=commands.exception('capture-secondary',secondary)
    if primary is not None:
        capture['primaryErrorSHA256']=commands.exception('wait-primary',primary)
        raise primary
    if secondary is not None:raise secondary
    return result


def container_probe(reference, result):
    stderr=_bytes(result.stderr).strip()
    prefixes=(b'Error: No such object: ',b'Error response from daemon: No such container: ')
    exact_absence=any(stderr[:len(prefix)].lower()==prefix.lower() and stderr[len(prefix):]==reference.encode() for prefix in prefixes)
    if result.returncode==1 and _bytes(result.stdout).strip() in (b'',b'[]') and exact_absence:return None
    if result.returncode!=0:raise ValueError('unknown container inspection; retain obligation')
    rows=json.loads(result.stdout)
    if not isinstance(rows,list) or len(rows)!=1:raise ValueError('ambiguous owned container')
    return rows[0]


def owned_container(entry, current, owner):
    if (not current or current.get('Name')!='/'+entry['name']
        or entry.get('id') and current.get('Id')!=entry['id']
        or current.get('Config',{}).get('Labels',{}).get('webdock.recovery-fixture')!=owner):
        raise ValueError('container custody differs; no stop/remove')
    return current['Id']


def cleanup_container(entry, owner, commands):
    reference=entry.get('id') or entry['name']
    stage='container-'+hashlib.sha256(reference.encode()).hexdigest()[:12]
    current=container_probe(reference,commands.call(stage+'-inspect','inspect',reference,check=False))
    if current is None:return
    identity=owned_container(entry,current,owner)
    if current.get('State',{}).get('Running'):
        commands.call(stage+'-stop','stop','--time','5',identity)
        current=container_probe(identity,commands.call(stage+'-stopped','inspect',identity,check=False))
        owned_container(entry,current,owner)
        if current.get('State',{}).get('Running'):raise ValueError('owned stop incomplete')
    commands.call(stage+'-remove','rm','--volumes',identity)
    if container_probe(identity,commands.call(stage+'-absence','inspect',identity,check=False)) is not None:
        raise ValueError('container removal unverified')


def volume_absent(reference, result):
    expected={('Error response from daemon: get '+reference+': no such volume').encode(),('Error: No such volume: '+reference).encode()}
    if result.returncode==1 and _bytes(result.stdout).strip() in (b'',b'[]') and _bytes(result.stderr).strip() in expected:return True
    raise ValueError('volume absence not positively established')
