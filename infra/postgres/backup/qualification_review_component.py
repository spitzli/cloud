#!/usr/bin/env python3
"""Own disposable PostgreSQL review primitives. Default prepares pins; --execute
requires the separately coordinated Root window. No Native/Core/S3/provider use.
"""
import argparse
import difflib
import hashlib
import json
import os
from pathlib import Path
import secrets
import re
import stat
import traceback
import shutil
import subprocess
import time
import uuid

ROOT = Path(__file__).resolve().parents[3]
IMAGE = 'sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336'
MIGRATOR_SHA = '76107649da9277144f6747914e50932fc1ac00dc461c127c346e13d51014e2c6'
FILTER = 'service::restore_orders::review_sql::component_'
SOURCES = [
 'crates/databases/src/service.rs', 'crates/databases/src/restore_intent.rs',
 'crates/databases/src/service/orders.rs', 'crates/databases/src/service/capacity.rs',
 'crates/databases/src/service/backups.rs', 'crates/databases/src/service/restore_orders.rs',
 'crates/databases/src/service/restore_orders/review_sql.rs',
 'docs/superpowers/plans/2026-10-26-postgresql-restore-0056.draft.sql',
 'infra/postgres/backup/qualification_review_component.py',
 'infra/postgres/backup/qualification_review_component_test.py', 'Cargo.lock',
]
ROLES = ['webdock_auth_bootstrap','webdock_auth_runtime','webdock_worker_runtime','webdock_admin_runtime',
         'webdock_registry_runtime','webdock_runtime','webdock_browser_website','webdock_cms_archive_runtime']

def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

def text_file(path, text):
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW
    fd = os.open(path, flags, 0o600)
    with os.fdopen(fd, 'w', encoding='utf-8') as output:
        output.write(text)


def write(path, value):
    text_file(path, json.dumps(value, indent=2, sort_keys=True) + '\n')


def owned(path, directory=False, mode=None):
    info = path.lstat()
    kind_ok = stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode)
    if not kind_ok or info.st_uid != os.getuid() or (mode is not None and stat.S_IMODE(info.st_mode) != mode):
        raise PermissionError('candidate ownership/type/mode boundary')


def candidate_pin(directory):
    directory = Path(os.path.abspath(directory))
    if directory.parent.name != '.runtime-test' or not re.fullmatch(r'pg-restore-review1-[a-f0-9]{12}', directory.name):
        raise PermissionError('exact candidate path required')
    # Do not resolve away a symlink before checking the supplied path.
    for parent in [directory, *directory.parents]:
        if parent.is_symlink():
            raise PermissionError('candidate ancestor symlink forbidden')
    owned(directory, directory=True, mode=0o700)
    owned(directory.parent, directory=True)
    if directory.parent.stat().st_mode & 0o022:
        raise PermissionError('candidate parent is writable by another principal')
    for folder, dirs, files in os.walk(directory, followlinks=False):
        for name in dirs:
            owned(Path(folder) / name, directory=True, mode=0o700)
        for name in files:
            path = Path(folder) / name
            owned(path, mode=0o700 if path.parent == directory / 'candidate' else 0o600)
    owned(directory / 'pin.json', mode=0o600)
    fd = os.open(directory / 'pin.json', os.O_RDONLY | os.O_NOFOLLOW)
    with os.fdopen(fd, encoding='utf-8') as handle:
        info=os.fstat(handle.fileno())
        if info.st_uid!=os.getuid() or not stat.S_ISREG(info.st_mode) or stat.S_IMODE(info.st_mode)!=0o600:
            raise PermissionError('opened pin ownership/type/mode boundary')
        pin = json.load(handle)
    if pin['owner'] != directory.name or pin['image'] != IMAGE or pin['filter'] != FILTER:
        raise PermissionError('candidate pin identity mismatch')
    if (directory / 'private').exists() or (directory / 'result.json').exists():
        raise PermissionError('use a fresh candidate; never overwrite prior private state/evidence')
    if set(pin['binaries'])!={'database-primitives','webdock-migrate','host-psql'}:
        raise PermissionError('all three pinned executables required before credentials')
    for name, fact in pin['binaries'].items():
        if name not in ['database-primitives', 'webdock-migrate', 'host-psql'] or sha(directory / 'candidate' / name) != fact['sha256']:
            raise PermissionError('candidate binary mismatch')
    for source, digest in pin['sourceSHA256'].items():
        relative = Path(source)
        if relative.is_absolute() or '..' in relative.parts or sha(directory / 'sources' / relative) != digest:
            raise PermissionError('candidate source mismatch')
    if sha(directory / 'fixed-0056.draft.sql') != pin['fixedDraftSHA256'] or sha(directory / 'baseline-0056.draft.sql') != pin['baselineDraftSHA256']:
        raise PermissionError('candidate draft mismatch')
    verify_baseline_syntax(directory,pin)
    return directory, pin


def check_port(port):
    import socket
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', port))


def absent_reply(completed, reference, volume=False):
    if completed.returncode != 1 or completed.stdout.strip() not in ['', '[]']:
        return False
    # Docker29 uses lowercase prefixes. Only known error wording is folded;
    # the complete expected reference remains case-sensitive and anchored.
    reference=re.escape(reference)
    patterns=([r'(?i:Error response from daemon: get )'+reference+r'(?i:: no such volume)',
               r'(?i:Error(?: response from daemon)?: No such volume: )'+reference] if volume else
              [r'(?i:Error(?: response from daemon)?: No such (?:container|object): )'+reference])
    return any(re.fullmatch(pattern,completed.stderr.strip()) for pattern in patterns)


class Commands:
    def __init__(self, directory, result, sensitive, env):
        self.directory, self.result, self.sensitive, self.env = directory, result, sensitive, env
        self.errors = []

    def clean(self, value):
        text = value.decode(errors='replace') if isinstance(value, bytes) else str(value or '')
        for secret in self.sensitive:
            text = text.replace(secret, '[fixture-secret-redacted]')
        return text

    def log(self, phase, stdout='', stderr='', exception=None):
        record = 'stdout:\n' + self.clean(stdout) + '\nstderr:\n' + self.clean(stderr)
        if exception:
            record += '\nexception:\n' + self.clean(exception)
        try:
            path = self.directory / (phase + '.log')
            text_file(path, record)
            self.result['phases'].setdefault(phase, {})['logSHA256'] = sha(path)
        except BaseException as error:
            self.errors.append({'phase': phase, 'type': type(error).__name__, 'detail': self.clean(error)})

    def run(self, args, phase, data=None, env=None, timeout=30):
        start = time.monotonic()
        self.result['phases'][phase] = {'exitCode': None}
        try:
            completed = subprocess.run(args, input=data, capture_output=True, env=env or self.env, timeout=timeout)
        except BaseException as error:
            self.log(phase, getattr(error, 'stdout', ''), getattr(error, 'stderr', ''), traceback.format_exc())
            self.result['phases'][phase].update({'exceptionType': type(error).__name__, 'seconds': round(time.monotonic()-start, 3)})
            raise
        self.log(phase, completed.stdout, completed.stderr)
        self.result['phases'][phase].update({'exitCode': completed.returncode, 'seconds': round(time.monotonic()-start, 3)})
        return subprocess.CompletedProcess(args, completed.returncode, self.clean(completed.stdout), self.clean(completed.stderr))

    def inspect(self, reference, phase):
        template = '{"Id":{{json .Id}},"Image":{{json .Image}},"Owner":{{json (index .Config.Labels "webdock.pg-review-owner")}},"Mounts":{{json .Mounts}}}'
        try:
            completed = self.run(['docker','inspect','--type','container','--format',template,reference], phase, timeout=10)
        except BaseException as error:
            return {'state': 'unknown', 'exceptionType': type(error).__name__}
        if absent_reply(completed, reference):
            return {'state': 'absent'}
        if completed.returncode == 0:
            try:
                metadata = json.loads(completed.stdout)
                if isinstance(metadata, dict) and re.fullmatch('[a-f0-9]{64}', metadata.get('Id','')):
                    return {'state': 'present', 'metadata': metadata}
            except (ValueError, TypeError):
                pass
        return {'state': 'unknown', 'exitCode': completed.returncode}


def verified(metadata, cid, pin):
    return (metadata.get('Id') == cid and metadata.get('Owner') == pin['owner'] and metadata.get('Image') == pin['image']
            and isinstance(metadata.get('Mounts'), list))


def wait_for_postgres(commands, client, port, password, private, budget=30):
    profiles=private/'client-profiles'
    profiles.mkdir(mode=0o700)
    for name in ['pgpass','pgservice','psqlrc','krb-cache']:
        text_file(profiles/name,'')
    env=dict(commands.env,PGPASSWORD=password,PGCONNECT_TIMEOUT='2',PGSSLMODE='prefer',
             PGPASSFILE=str(profiles/'pgpass'),PGSERVICEFILE=str(profiles/'pgservice'),
             PGSYSCONFDIR=str(profiles),PSQLRC=str(profiles/'psqlrc'),PSQL_HISTORY=str(profiles/'history'),
             KRB5CCNAME='FILE:'+str(profiles/'krb-cache'))
    start=time.monotonic();deadline=start+budget
    evidence={'transport':'host-tcp','target':f'127.0.0.1:{port}/webdock_rls_test','authenticatedSelect1':False,'attempts':0}
    commands.result['startupGate']=evidence
    while True:
        remaining=deadline-time.monotonic()
        if remaining<=0:
            evidence['seconds']=round(time.monotonic()-start,3)
            raise TimeoutError('owned final host TCP authenticated SELECT 1 readiness timeout')
        evidence['attempts']+=1
        args=[str(client),'-X','-w','-A','-t','-v','ON_ERROR_STOP=1','-h','127.0.0.1','-p',str(port),'-U','postgres','-d','webdock_rls_test','-c','SELECT 1']
        try:
            ready=commands.run(args,f"startup-tcp-login-{evidence['attempts']:03d}",env=env,timeout=min(3,remaining))
        except subprocess.TimeoutExpired:
            ready=None
        if ready and ready.returncode==0 and ready.stdout.strip()=='1':
            evidence.update(authenticatedSelect1=True,seconds=round(time.monotonic()-start,3))
            return
        time.sleep(min(0.25,max(0,deadline-time.monotonic())))


def capture_server_diagnostics(commands, cid):
    evidence={'complete':True,'stateCaptured':False,'logsCaptured':False,'errors':[]}
    state='{"Id":{{json .Id}},"Status":{{json .State.Status}},"Running":{{json .State.Running}},"StartedAt":{{json .State.StartedAt}},"FinishedAt":{{json .State.FinishedAt}},"ExitCode":{{json .State.ExitCode}},"OOMKilled":{{json .State.OOMKilled}},"Error":{{json .State.Error}},"HealthStatus":{{with index .State "Health"}}{{json .Status}}{{else}}null{{end}}}'
    for phase,args,key in [
        ('server-state',['docker','inspect','--type','container','--format',state,cid],'stateCaptured'),
        ('server-logs',['docker','logs','--timestamps','--tail','400',cid],'logsCaptured')]:
        try:
            reply=commands.run(args,phase,timeout=10)
            captured=reply.returncode==0
            if key=='stateCaptured' and captured:
                data=json.loads(reply.stdout)
                captured=data.get('Id')==cid and all(field in data for field in ['Status','Running','StartedAt','FinishedAt','HealthStatus'])
            evidence[key]=captured
            if not captured:evidence['errors'].append({'phase':phase,'exitCode':reply.returncode})
        except BaseException as error:
            evidence['errors'].append({'phase':phase,'exceptionType':type(error).__name__})
    evidence['complete']=evidence['stateCaptured'] and evidence['logsCaptured']
    return evidence

def cleanup_container(commands, creation, pin):
    cleanup = {'complete': True, 'ownedAbsent': None, 'obligations': [], 'containerID': creation.get('verifiedCID'), 'persistentOrAnonymousVolumes': None}
    if not creation['attempted']:
        cleanup['state'] = 'no-create-attempt'
        return cleanup
    reference = creation.get('returnedCID') or pin['owner']
    probe = commands.inspect(reference, 'cleanup-inspect')
    if probe['state'] == 'absent':
        cleanup.update(ownedAbsent=True, state='positively-absent')
    else:
        metadata = probe.get('metadata')
        if creation.get('returnedCID') and metadata and verified(metadata, creation['returnedCID'], pin):
            creation['verifiedCID'] = creation['returnedCID']
            creation['metadata'] = metadata
        cid = creation.get('verifiedCID')
        if not cid:
            cleanup.update(complete=False, state='pending-ownership')
            cleanup['obligations'].append({'reference': reference, 'reason': 'create outcome/ownership not verified; nothing removed'})
            return cleanup
        cleanup['containerID'] = cid
        if metadata and not verified(metadata, cid, pin):
            cleanup.update(complete=False, state='ownership-mismatch')
            cleanup['obligations'].append({'reference': cid, 'reason': 'inspection did not match verified ownership; no removal'})
            return cleanup
        if commands.result.get('server-diagnostics',{}).get('skipped'):
            try:
                commands.result['server-diagnostics']=capture_server_diagnostics(commands,cid)
            except BaseException as error:
                commands.log('recovered-server-diagnostics-exception',exception=traceback.format_exc())
                commands.result['server-diagnostics']={'complete':False,'exceptionType':type(error).__name__}
        try:
            commands.run(['docker','rm','-f','-v',cid], 'cleanup-remove', timeout=15)
        except BaseException:
            pass  # Details/partial output were captured; still probe exact CID.
        after = commands.inspect(cid, 'cleanup-after')
        cleanup['ownedAbsent'] = True if after['state'] == 'absent' else False if after['state'] == 'present' else None
        if cleanup['ownedAbsent'] is not True:
            cleanup['complete'] = False
            cleanup['obligations'].append({'reference': cid, 'reason': 'exact container absence not positively established'})
    mounts = creation.get('metadata', {}).get('Mounts', [])
    volumes = [mount.get('Name') for mount in mounts if mount.get('Type') == 'volume']
    cleanup['persistentOrAnonymousVolumes'] = len(volumes) if creation.get('metadata') else None
    cleanup['volumeAbsence'] = {}
    for number, volume in enumerate(volumes):
        try:
            reply = commands.run(['docker','volume','inspect',volume], f'cleanup-volume-{number}', timeout=10)
            absent = True if absent_reply(reply, volume, volume=True) else False if reply.returncode == 0 else None
        except BaseException:
            absent = None
        cleanup['volumeAbsence'][volume] = absent
        if absent is not True:
            cleanup['complete'] = False
            cleanup['obligations'].append({'reference': volume, 'reason': 'exact volume absence not positively established'})
    return cleanup


def cleanup_private(private, created):
    result = {'complete': True, 'createdByThisAttempt': created, 'removed': not created}
    if not created:
        return result
    try:
        shutil.rmtree(private)  # Only this attempt's fresh 0700 private subtree.
        result['removed'] = not private.exists() and not private.is_symlink()
        result['complete'] = result['removed']
    except BaseException as error:
        result.update(complete=False, removed=False, exceptionType=type(error).__name__, detail=str(error), exceptionTraceback=traceback.format_exc())
    return result

BILLING_CASE = "CASE WHEN (intent#>'{target,plan}') ? 'sponsoredCustomerID' THEN 'sponsored' ELSE 'monthly-net-fixed' END"


def baseline_syntax(raw):
    original=raw.decode('utf-8')
    wrapped='('+BILLING_CASE+')'
    if original.count(BILLING_CASE)!=1 or wrapped in original or len(re.findall(r'\bCASE\b',original,re.IGNORECASE))!=1:
        raise ValueError('expected exactly one original unparenthesized billing CASE')
    normalized=original.replace(BILLING_CASE,wrapped,1)
    if normalized.replace(wrapped,BILLING_CASE,1)!=original or len(normalized.encode())!=len(raw)+2:
        raise ValueError('normalization must be two parentheses only')
    diff=''.join(difflib.unified_diff(original.splitlines(True),normalized.splitlines(True),fromfile='baseline-0056.draft.sql',tofile='baseline-0056.syntax-normalized.draft.sql'))
    return normalized.encode(),diff


def write_baseline_syntax(directory):
    raw=(directory/'baseline-0056.draft.sql').read_bytes()
    normalized,diff=baseline_syntax(raw)
    text_file(directory/'baseline-0056.syntax-normalized.draft.sql',normalized.decode())
    text_file(directory/'baseline-syntax.diff',diff)
    provenance={'originalFile':'baseline-0056.draft.sql','normalizedFile':'baseline-0056.syntax-normalized.draft.sql',
                'originalSHA256':hashlib.sha256(raw).hexdigest(),'normalizedSHA256':hashlib.sha256(normalized).hexdigest(),
                'replacementCount':1,'byteDelta':2,'diffFile':'baseline-syntax.diff','diffSHA256':hashlib.sha256(diff.encode()).hexdigest(),
                'change':'parentheses only around the one billing CASE; original old logic retained',
                'originalParsedSuccessfully':False,'normalizedExecutionPending':True}
    write(directory/'baseline-syntax-provenance.json',provenance)
    return dict(provenance,provenanceFile='baseline-syntax-provenance.json',provenanceSHA256=sha(directory/'baseline-syntax-provenance.json'))


def verify_baseline_syntax(directory,pin):
    overlay=pin['baselineSyntaxOverlay']
    normalized,diff=baseline_syntax((directory/'baseline-0056.draft.sql').read_bytes())
    if ((directory/'baseline-0056.syntax-normalized.draft.sql').read_bytes()!=normalized
        or sha(directory/'baseline-0056.draft.sql')!=overlay['originalSHA256']
        or sha(directory/'baseline-0056.syntax-normalized.draft.sql')!=overlay['normalizedSHA256']
        or sha(directory/'baseline-syntax.diff')!=overlay['diffSHA256']
        or (directory/'baseline-syntax.diff').read_text()!=diff
        or sha(directory/'baseline-syntax-provenance.json')!=overlay['provenanceSHA256']):
        raise PermissionError('exact labelled syntax-only baseline overlay required')

def prepare(baseline):
    previous = json.loads((baseline / 'baseline-evidence.json').read_text())
    for source, digest in previous['sources'].items():
        assert sha(baseline / 'baseline' / source) == digest, 'review baseline changed'
    prior = json.loads((ROOT / 'docs/verification/2026-10-26-oauth-mcp-owned-postgres-component.json').read_text())
    native = {str(p.relative_to(ROOT)): {'sha256': sha(p), 'sha384': hashlib.sha384(p.read_bytes()).hexdigest()}
              for p in sorted((ROOT / 'crates/persistence/migrations').glob('*.sql'))}
    assert native == prior['schema']['nativeMigrations'], 'copied <=55 migrator requires its exact source set'
    base = ROOT / 'crates/persistence/schema/baseline.sql'
    assert sha(base) == prior['candidate']['sourceSHA256'][str(base.relative_to(ROOT))]
    migrator = ROOT / prior['candidate']['directory'] / 'candidate/webdock-migrate'
    assert sha(migrator) == MIGRATOR_SHA
    binary = ROOT / 'target/debug/deps/webdock_databases-452f7e98f980a6af'
    assert binary.is_file()
    listing = subprocess.run([str(binary), '--list'], capture_output=True, check=True, timeout=10).stdout.decode()
    assert listing.count(FILTER) == 5
    check = subprocess.run(['docker','image','inspect',IMAGE,'--format','{{.Id}}'], capture_output=True, check=True, timeout=10)
    assert check.stdout.decode().strip() == IMAGE
    directory = ROOT / '.runtime-test' / ('pg-restore-review1-' + uuid.uuid4().hex[:12])
    directory.mkdir(mode=0o700)
    (directory / 'candidate').mkdir(mode=0o700)
    shutil.copy2(binary, directory / 'candidate/database-primitives')
    shutil.copy2(migrator, directory / 'candidate/webdock-migrate')
    host_psql=Path('/usr/bin/psql')
    assert host_psql.is_file() and not host_psql.is_symlink(), 'installed direct psql required'
    shutil.copy2(host_psql,directory/'candidate/host-psql')
    for source in SOURCES + list(native) + ['crates/persistence/schema/baseline.sql']:
        destination = directory / 'sources' / source
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / source, destination)
    old = baseline / 'baseline/docs/superpowers/plans/2026-10-26-postgresql-restore-0056.draft.sql'
    shutil.copy2(old, directory / 'baseline-0056.draft.sql')
    shutil.copy2(ROOT / SOURCES[7], directory / 'fixed-0056.draft.sql')
    overlay=write_baseline_syntax(directory)
    pin = {
        'owner': directory.name, 'image': IMAGE, 'port': 33276, 'filter': FILTER,
        'sourceSHA256': {p: sha(ROOT / p) for p in SOURCES}, 'nativeMigrations': native,
        'baselineSHA256': sha(base), 'baselineDraftSHA256': sha(old), 'baselineSyntaxOverlay':overlay,
        'fixedDraftSHA256': sha(directory / 'fixed-0056.draft.sql'),
        'binaries': {name: {'sha256':sha(directory / 'candidate' / name), 'bytes':(directory / 'candidate' / name).stat().st_size}
                     for name in ['database-primitives','webdock-migrate','host-psql']},
        'budget': {'cpus':1,'memoryBytes':1610612736,'tmpfsBytes':1073741824,'pids':256,
                   'readinessSeconds':30,'migrationSeconds':120,'testSeconds':180,
                   'newNetworks':0,'persistentOrAnonymousVolumes':0,'imagePull':False},
        'scope': 'timestamp/NULL/replay-query/epoch/temp-ownership/sub-XID/MAC/tuple primitives only; no Source/audit/Native positive',
        'reviewBaselineEvidenceSHA256': sha(baseline / 'baseline-evidence.json'),
    }
    for folder, dirs, files in os.walk(directory):
        Path(folder).chmod(0o700)
        for name in files:
            path=Path(folder)/name
            path.chmod(0o700 if path.parent==directory/'candidate' else 0o600)
    write(directory / 'pin.json', pin)
    print(json.dumps({'prepared':str(directory),'pinSHA256':sha(directory / 'pin.json'),'binaries':pin['binaries'],
                      'draftSHA256':pin['fixedDraftSHA256'],'schemaCount':len(native),'budget':pin['budget']}))

def execute(directory):
    directory, pin = candidate_pin(directory)  # Before generating/sending secrets.
    private = directory / 'private'
    private_created = False
    sensitive = []
    safe_env = {'PATH': os.defpath, 'RUST_BACKTRACE':'0', 'DOCKER_CONFIG':str(private / 'docker-config')}
    name = pin['owner']
    result = {'baselineInput':'syntax-normalized original old logic; original raw draft not parsed/tested in this run','scope':pin['scope'], 'container':name, 'started':False, 'phases':{}, 'cleanup':None, 'status':'not-started'}
    creation = {'attempted':False, 'returnedCID':None, 'verifiedCID':None}
    commands = Commands(directory,result,sensitive,safe_env)
    primary = None
    evidence_error = None

    def command(args, phase, data=None, env=None, timeout=30):
        reply=commands.run(args,phase,data=data,env=env,timeout=timeout)
        return reply.returncode, reply.stdout

    def sql(text, phase):
        return command(['docker','exec','-i',creation['verifiedCID'],'psql','-X','-v','ON_ERROR_STOP=1','-U','postgres','-d','webdock_rls_test','-At'], phase, text.encode(),timeout=30)

    try:
        private.mkdir(mode=0o700, exist_ok=False)
        private_created = True
        role_passwords={role:secrets.token_urlsafe(36) for role in ROLES}
        password,rls=secrets.token_urlsafe(36),secrets.token_urlsafe(36)
        sensitive.extend([password,rls,*role_passwords.values()])
        role_file=private/'roles.json'
        write(role_file,role_passwords)
        env_file=private/'postgres.env'
        text_file(env_file,f'POSTGRES_USER=postgres\nPOSTGRES_PASSWORD={password}\nPOSTGRES_DB=webdock_rls_test\n')
        (private/'docker-config').mkdir(mode=0o700)
        before=commands.inspect(name,'before-create')
        assert before['state']=='absent', 'requested name absence is unverified; refuse create'
        check_port(pin['port'])
        run_args = ['docker','run','-d','--pull=never','--name',name,'--label','webdock.pg-review-owner='+name,
                    '--cpus=1','--memory=1536m','--pids-limit=256','--network=bridge',
                    '-p',f"127.0.0.1:{pin['port']}:5432",'--tmpfs','/var/lib/postgresql:rw,nosuid,size=1g',
                    '--log-driver=local','--log-opt','max-size=5m','--log-opt','max-file=1','--log-opt','compress=false',
                    '--env-file',str(env_file),pin['image'],'postgres','-c','log_statement=none',
                    '-c','log_min_error_statement=panic','-c','log_connections=off','-c','log_disconnections=off']
        creation['attempted']=True
        reply=commands.run(run_args,'start')
        assert reply.returncode==0, 'owned PG start failed'
        cid=reply.stdout.strip()
        assert re.fullmatch('[a-f0-9]{64}',cid), 'successful create must return one CID64'
        creation['returnedCID']=cid
        inspection=commands.inspect(cid,'created-ownership')
        assert inspection['state']=='present' and verified(inspection['metadata'],cid,pin), 'create ownership/image not proven'
        creation.update(verifiedCID=cid,metadata=inspection['metadata'])
        result.update(started=True,containerID=cid)
        assert not any(mount.get('Type')=='volume' for mount in inspection['metadata']['Mounts']), 'unexpected retained/anonymous volume budget'
        wait_for_postgres(commands,directory/'candidate/host-psql',pin['port'],password,private)
        owner = f"postgresql://postgres:{password}@127.0.0.1:{pin['port']}/webdock_rls_test"
        migration_env = dict(safe_env, WEBDOCK_MIGRATION_DATABASE_URL=owner, DATABASE_RLS_SECRET=rls)
        code,_ = command([str(directory / 'candidate/webdock-migrate'),'--target',f"127.0.0.1:{pin['port']}/webdock_rls_test",'--initialize-empty','--provision-roles-from',str(role_file)],'migration',env=migration_env,timeout=120)
        assert code == 0, 'copied <=55 bootstrap failed'
        print('owned <=55 bootstrap completed',flush=True)
        code,journal=sql("SELECT jsonb_object_agg(version::text,jsonb_build_object('checksum',encode(checksum,'hex'),'success',success)) FROM webdock_api_migrations.rust_journal;",'journal')
        assert code == 0
        expected_journal={str(int(Path(path).name.split('_')[0])):{'checksum':fact['sha384'],'success':True} for path,fact in pin['nativeMigrations'].items()}
        assert json.loads(journal.strip())==expected_journal, 'native journal must exactly match copied <=55 source'
        component={'ownerUrl':owner,'workerUrl':f"postgresql://webdock_worker_runtime:{role_passwords['webdock_worker_runtime']}@127.0.0.1:{pin['port']}/webdock_rls_test",
                   'businessUrl':f"postgresql://webdock_auth_runtime:{role_passwords['webdock_auth_runtime']}@127.0.0.1:{pin['port']}/webdock_rls_test",'rlsSecret':rls,'baseline':True}
        input_file = private / 'component.json'
        for baseline, draft, expected_test_exit in [(True,'baseline-0056.syntax-normalized.draft.sql',101),(False,'fixed-0056.draft.sql',0)]:
            phase = 'baseline-syntax-normalized' if baseline else 'fixed'
            if not baseline:
                code,_=sql('DROP FUNCTION webdock_rls.record_database_restore_admission(text); DROP FUNCTION webdock_rls.database_restore_source(text,text); DROP FUNCTION webdock_rls.database_restore_epoch(uuid);','drop-baseline-only')
                assert code == 0
            code,_=sql((directory / draft).read_text(),'apply-'+phase+'-external-only')
            assert code == 0, 'external draft did not compile'
            component['baseline'] = baseline
            if input_file.exists():
                input_file.unlink()
            write(input_file,component)
            test_env = dict(safe_env, WEBDOCK_PG_REVIEW_COMPONENT_INPUT=str(input_file))
            code,out=command([str(directory / 'candidate/database-primitives'),pin['filter'],'--ignored','--test-threads=1','--nocapture'],phase+'-primitives',env=test_env,timeout=180)
            assert code == expected_test_exit, phase+' primitive outcome unexpected'
            assert ('0 passed; 5 failed' if baseline else '5 passed; 0 failed') in out, 'exact component selection/result required'
            print(phase+' primitive result recorded',flush=True)
        code,journal=sql("SELECT jsonb_object_agg(version::text,jsonb_build_object('checksum',encode(checksum,'hex'),'success',success)) FROM webdock_api_migrations.rust_journal;",'journal-after-external')
        assert code == 0 and json.loads(journal.strip())==expected_journal, 'external draft must not change native journal'
        result['status']='component-only-green-after-five-baseline-reds'
    except BaseException as error:
        primary=error
        result.update(status='failed',failureType=type(error).__name__,failureDetail=commands.clean(error))
        commands.log('primary-exception',exception=traceback.format_exc())
    finally:
        try:
            if creation.get('verifiedCID'):
                result['server-diagnostics']=capture_server_diagnostics(commands,creation['verifiedCID'])
            else:
                result['server-diagnostics']={'complete':True,'skipped':'no verified ownership; no foreign logs read'}
        except BaseException as error:
            commands.log('server-diagnostics-exception',exception=traceback.format_exc())
            result['server-diagnostics']={'complete':False,'exceptionType':type(error).__name__}
        try:
            result['cleanup']=cleanup_container(commands,creation,pin)
        except BaseException as error:
            commands.log('cleanup-exception',exception=traceback.format_exc())
            result['cleanup']={'complete':False,'ownedAbsent':None,'obligations':[{'reference':creation.get('verifiedCID') or creation.get('returnedCID') or name,'reason':'cleanup exception; reconcile exact ownership/absence'}], 'exceptionType':type(error).__name__}
        # Credential erasure is independent of daemon/cleanup/evidence failures.
        result['privateCleanup']=cleanup_private(private,private_created)
        if not result['privateCleanup']['complete']:
            for key in ['detail','exceptionTraceback']:
                if key in result['privateCleanup']:result['privateCleanup'][key]=commands.clean(result['privateCleanup'][key])
            commands.log('private-cleanup-exception',exception=result['privateCleanup'].get('exceptionTraceback','private cleanup incomplete'))
            result['cleanup']['obligations'].append({'reference':str(private),'reason':'private credential cleanup incomplete'})
        if not result['cleanup']['complete'] or not result['privateCleanup']['complete']:
            result['status']='failed-with-cleanup-obligations' if primary else 'incomplete-cleanup'
        if not result['server-diagnostics']['complete'] and not primary:
            result['status']='incomplete-evidence'
        if commands.errors:
            result['diagnosticWriteFailures']=commands.errors
            if not primary: result['status']='incomplete-evidence'
        try:
            write(directory/'result.json',json.loads(commands.clean(json.dumps(result))))
        except BaseException as error:
            evidence_error=error
            commands.log('result-write-failure',exception=traceback.format_exc())
        try:
            print(commands.clean(json.dumps({'status':result['status'] if not evidence_error else 'result-evidence-write-failed','cleanup':result['cleanup'], 'privateCleanup':result['privateCleanup'], 'resultSHA256':sha(directory/'result.json') if not evidence_error else None})),flush=True)
        except BaseException as error:
            evidence_error=evidence_error or error
            commands.log('summary-write-failure',exception=traceback.format_exc())
    if primary is not None:
        raise primary.with_traceback(primary.__traceback__)
    if evidence_error:
        raise RuntimeError('component result evidence could not be written') from evidence_error
    if not result['cleanup']['complete'] or not result['privateCleanup']['complete'] or commands.errors or not result['server-diagnostics']['complete']:
        raise RuntimeError('component cleanup/evidence incomplete; explicit obligations recorded')

if __name__=='__main__':
    parser=argparse.ArgumentParser()
    parser.add_argument('--prepare',type=Path)
    parser.add_argument('--execute',type=Path)
    args=parser.parse_args()
    assert bool(args.prepare) != bool(args.execute), 'choose prepare or Root-released execute'
    prepare(args.prepare.resolve()) if args.prepare else execute(args.execute)
