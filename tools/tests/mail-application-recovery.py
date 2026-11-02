#!/usr/bin/env python3
"""Disposable private-Mail + ntfy data recovery qualification; NOT Core/CE authority proof.

Reuses tools/tests/object-storage.py's existing MinIO fixture. No cluster access.
Every created container has unique custody, bounded lifetime and exact-ID cleanup.
The object adapter executable is prepared from recovery-object.rs only after a
coordinated build; this script never changes a Cargo manifest or dependency lock.
"""
import base64
from contextlib import closing
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import secrets
import shutil
import sqlite3
import stat
import subprocess
import sys
import tarfile
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'agents/kubernetes'))
import ntfy_recovery as profile

PG = 'docker.io/library/postgres@sha256:4da1a4828be12604092fa55311276f08f9224a74a62dcb4708bd7439e2a03911'
NTFY = 'binwiederhier/ntfy@sha256:6ef4b819f722fccdc036af611c4774cfdc2de821ab74fdd48bbf4c9d6f8973da'
PYTHON = 'python@sha256:420cd0bf0f3998275875e02ecd5808168cf0843cbb4d3c536432f729247b2acc'


def require(value, message):
    if not value: raise RuntimeError(message)


class Fixture:
    def __init__(self, directory, executable):
        self.root = Path(directory); self.run = uuid.uuid4().hex
        self.executable = Path(executable).resolve(); self.containers = []
        self.clients = {}
        self.s3_principals = []
        self.report = {'scope': 'private Mail storage + offline ntfy profile; native Core/CE integration not claimed', 'run': self.run, 'checks': []}
        self.deadline = time.monotonic() + 900

    def command(self, args, *, data=None, env=None, timeout=90, allowed=False):
        require(time.monotonic() < self.deadline, 'fixture_deadline')
        result = subprocess.run(args, input=data, capture_output=True, env={**os.environ, **(env or {})}, timeout=min(timeout, max(1, self.deadline-time.monotonic())), cwd=ROOT)
        if result.returncode and not allowed:
            # Driver/SQL diagnostics may echo private input. Keep only safe failure metadata.
            with (self.root / 'private-diagnostics.log').open('a') as log:log.write(json.dumps({'program':Path(str(args[0])).name,'exitCode':result.returncode})+'\n')
            raise RuntimeError('owned_fixture_command_failed_' + Path(str(args[0])).name)
        return result

    def docker(self, *args, **kw): return self.command(['docker', '--host', 'unix:///var/run/docker.sock', *args], **kw)

    def create(self, kind, image, args, *, entrypoint=None, env=None, command_args=()):
        name = 'wd-recovery-' + kind + '-' + self.run[:16]
        item = {'name': name, 'id': None}; self.containers.append(item); self.custody()
        command = ['create', '--name', name, '--label', 'webdock.recovery-fixture=' + self.run, '--restart=no', '--memory=512m', '--cpus=1', '--pids-limit=128']
        if entrypoint: command += ['--entrypoint', entrypoint]
        for key in env or {}: command += ['--env', key]
        result = self.docker(*command, *args, image, *command_args, env=env)
        item['id'] = result.stdout.decode().strip(); self.custody()
        return item['id']

    def custody(self):
        (self.root / 'custody.json').write_text(json.dumps({'run': self.run, 'containers': self.containers, 's3Principals': self.s3_principals}))

    def inspect(self, container):
        value = json.loads(self.docker('inspect', container).stdout)[0]
        require(value['Config']['Labels'].get('webdock.recovery-fixture') == self.run, 'container_custody_changed')
        return value

    def cleanup(self):
        failures = []
        for item in reversed(self.containers):
            identity = item['id'] or item['name']
            try:
                found = self.docker('inspect', identity, allowed=True)
                if found.returncode:
                    require(b'no such' in found.stderr.lower(), 'container_absence_unverified'); continue
                value = self.inspect(identity)
                require(not item['id'] or value['Id'] == item['id'], 'container_id_changed')
                self.docker('rm', '--force', '--volumes', value['Id'])
                result = self.docker('inspect', value['Id'], allowed=True)
                require(result.returncode and b'no such' in result.stderr.lower(), 'container_cleanup_unverified')
            except Exception: failures.append(item['name'])
        require(not failures, 'exact_container_cleanup_failed_' + ','.join(failures))
        self.report['cleanup'] = {'containers': len(self.containers), 'exactIDsAbsent': True, 'clustersTouched': False}

    def pki(self):
        p = self.root / 'tls'; p.mkdir(mode=0o700)
        self.command(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-keyout', str(p/'ca.key'), '-out', str(p/'ca.crt'), '-days', '1', '-subj', '/CN=Owned recovery fixture CA', '-addext', 'keyUsage=critical,keyCertSign,cRLSign'])
        self.command(['openssl', 'req', '-newkey', 'rsa:2048', '-nodes', '-keyout', str(p/'server.key'), '-out', str(p/'server.csr'), '-subj', '/CN=localhost'])
        (p/'extensions').write_text('subjectAltName=DNS:localhost,IP:127.0.0.1\nextendedKeyUsage=serverAuth\n')
        self.command(['openssl', 'x509', '-req', '-in', str(p/'server.csr'), '-CA', str(p/'ca.crt'), '-CAkey', str(p/'ca.key'), '-CAcreateserial', '-out', str(p/'server.crt'), '-days', '1', '-extfile', str(p/'extensions')])
        for file in p.iterdir(): file.chmod(0o600)
        return p

    def postgres(self, name, pki):
        password = secrets.token_hex(24)
        script = '(sleep 850; kill -TERM 1) & mkdir -p /tmp/tls; cp /fixture-tls/server.* /tmp/tls/; chown -R postgres:postgres /tmp/tls; chmod 600 /tmp/tls/server.key; exec docker-entrypoint.sh postgres -c ssl=on -c ssl_cert_file=/tmp/tls/server.crt -c ssl_key_file=/tmp/tls/server.key'
        container = self.create(name, PG, ['--publish', '127.0.0.1::5432', '--mount', f'type=bind,source={pki},target=/fixture-tls,readonly'], entrypoint='/bin/sh', env={'POSTGRES_PASSWORD': password}, command_args=['-ec',script])
        self.docker('start', container)
        until = time.monotonic()+40
        # The entrypoint's temporary initialization postmaster accepts Unix sockets
        # before it shuts down. Only the final server opens this TCP listener.
        while self.docker('exec', container, 'pg_isready', '-h', '127.0.0.1', '-U', 'postgres', allowed=True).returncode:
            require(time.monotonic()<until, 'owned_postgres_readiness_deadline'); time.sleep(.2)
        port = self.inspect(container)['NetworkSettings']['Ports']['5432/tcp'][0]['HostPort']
        return container, port

    def sql(self, container, sql, database='postgres'):
        return self.docker('exec', '-i', container, 'psql', '-X', '-q', '-A', '-t', '-v', 'ON_ERROR_STOP=1', '-U', 'postgres', '-d', database, data=sql.encode()).stdout

    def rust(self, name, url, pki, extra=None):
        env={'WEBDOCK_MAIL_RECOVERY_TEST_URL':url, 'WEBDOCK_MAIL_RECOVERY_TEST_CA':str(pki/'ca.crt'), **(extra or {})}
        self.command(['cargo','test','--manifest-path','plugins/mail/Cargo.toml','--locked','--test','recovery', name,'--','--ignored','--exact'], env=env, timeout=180)

    def encrypt(self, source, target, recipient):
        self.command(['age','--encrypt','--recipient',recipient,'--output',str(target),str(source)])

    def decrypt(self, source, target, key, *, allowed=False):
        return self.command(['age','--decrypt','--identity',str(key),'--output',str(target),str(source)], allowed=allowed)

    def object(self, mode, config, scope, file, receipt, *, allowed=False):
        result=self.command([str(self.executable),mode,str(config),scope,str(file),str(receipt)],allowed=allowed)
        if mode=='upload' and result.returncode==0:
            self.report.setdefault('objectReceipts',{})[scope]=json.loads(Path(receipt).read_text())
        return result

    def ntfy(self, name, data, config, pki):
        args=['create','--name','wd-recovery-'+name+'-'+self.run[:16],'--label','webdock.recovery-fixture='+self.run,'--network','none','--restart=no','--user',f'{os.getuid()}:{os.getgid()}','--memory=256m','--cpus=1','--pids-limit=64','--read-only','--cap-drop=ALL','--security-opt=no-new-privileges','--tmpfs','/tmp:rw,noexec,nosuid,size=16777216','--mount',f'type=bind,source={data},target=/var/lib/ntfy','--mount',f'type=bind,source={config},target=/etc/ntfy/server.yml,readonly','--mount',f'type=bind,source={pki},target=/tls,readonly','--entrypoint','/bin/sh',NTFY,'-ec','(sleep 600; kill -TERM 1) & exec ntfy serve']
        item={'name':args[2],'id':None};self.containers.append(item);self.custody()
        item['id']=self.docker(*args).stdout.decode().strip();self.custody();self.docker('start',item['id'])
        return item['id']

    def http(self, container, path, *, method='GET', body=b'', headers=None, password=None, allowed=False, wrong_ca=False):
        request={'url':'https://localhost:8443'+path,'method':method,'body':base64.b64encode(body).decode(),'headers':headers or {},'password':password,'wrongCA':wrong_ca}
        script='''import base64,json,ssl,sys,urllib.request,urllib.error,socket
x=json.load(sys.stdin)
ctx=ssl.create_default_context(cafile=None if x['wrongCA'] else '/tls/ca.crt')
h=x['headers']
if x['password']:h['Authorization']='Basic '+base64.b64encode(('owner:'+x['password']).encode()).decode()
req=urllib.request.Request(x['url'],data=base64.b64decode(x['body']) if x['method']!='GET' else None,headers=h,method=x['method'])
try:
 with urllib.request.urlopen(req,context=ctx,timeout=5) as r:print(json.dumps({'status':r.status,'body':base64.b64encode(r.read()).decode()}))
except urllib.error.HTTPError as e:print(json.dumps({'status':e.code,'body':''}))
except urllib.error.URLError as e:
 if isinstance(e.reason,ssl.SSLCertVerificationError):print(json.dumps({'tlsRejected':True,'verifyCode':e.reason.verify_code,'verifyMessage':e.reason.verify_message}))
 else:sys.exit(2)
'''
        # A bounded disposable client shares ONLY the target's network-none namespace.
        if container not in self.clients:
            name='wd-recovery-client-'+uuid.uuid4().hex[:16]
            item={'name':name,'id':None};self.containers.append(item);self.custody()
            result=self.docker('create','--name',name,'--label','webdock.recovery-fixture='+self.run,'--network','container:'+container,'--user',f'{os.getuid()}:{os.getgid()}','--read-only','--cap-drop=ALL','--security-opt=no-new-privileges','--memory=64m','--cpus=.5','--pids-limit=32','--mount',f'type=bind,source={self.root / "tls"},target=/tls,readonly',PYTHON,'python','-c','import time;time.sleep(600)')
            item['id']=result.stdout.decode().strip();self.custody();self.docker('start',item['id']);self.clients[container]=item['id']
        result=self.docker('exec','-i',self.clients[container],'python','-c',script,data=json.dumps(request).encode(),allowed=allowed)
        if result.returncode:
            with (self.root / 'http-client-private.log').open('ab') as log:log.write(result.stdout+result.stderr)
        return json.loads(result.stdout) if result.returncode==0 else None


def exercise(config_path, mc, uploader, executable):
    with tempfile.TemporaryDirectory(prefix='wd-mail-app-recovery-') as directory:
        os.chmod(directory,0o700);f=Fixture(directory,executable);p=f.root
        readonly='wdrestore'+secrets.token_hex(8);policy_name=readonly;readonly_created=False;policy_created=False
        try:
            input_config=json.loads(Path(config_path).read_text());bucket=input_config['config']['bucket']
            uploader_config=p/'upload.json';uploader_config.write_text(json.dumps({k:input_config[k] for k in ('config','credentials')}))
            readonly_secret=secrets.token_hex(32)
            f.s3_principals.append({'user':readonly,'policy':policy_name,'bucket':bucket});f.custody()
            mc('admin','user','add','fixture',readonly,readonly_secret);readonly_created=True
            policy={'Version':'2012-10-17','Statement':[{'Effect':'Allow','Action':['s3:GetObject'],'Resource':[f'arn:aws:s3:::{bucket}/*']}]}
            mc('admin','policy','create','fixture',policy_name,'/dev/stdin',input_text=json.dumps(policy));policy_created=True
            mc('admin','policy','attach','fixture',policy_name,'--user',readonly)
            reader_config=p/'restore.json';reader_config.write_text(json.dumps({'config':input_config['config'],'credentials':{'accessKeyId':readonly,'secretAccessKey':readonly_secret}}))
            for x in [uploader_config,reader_config]:x.chmod(0o600)
            pki=f.pki()
            key=p/'operator.agekey';f.command(['age-keygen','-o',str(key)])
            recipient=f.command(['age-keygen','-y',str(key)]).stdout.decode().strip()
            wrong=p/'wrong.agekey';f.command(['age-keygen','-o',str(wrong)])
            source,port=f.postgres('mail-source',pki)
            role_password=secrets.token_hex(24)
            f.sql(source,f"CREATE ROLE mail_owner LOGIN PASSWORD '{role_password}'; CREATE DATABASE mail_gate OWNER mail_owner; CREATE DATABASE mail_uncertain OWNER mail_owner; CREATE DATABASE mail_data OWNER mail_owner;")
            url=lambda port,database:f'postgresql://mail_owner:{role_password}@127.0.0.1:{port}/{database}'
            f.rust('durable_gate_drains_rejects_stale_resume_and_quarantines_restore',url(port,'mail_gate'),pki)
            f.rust('cancelled_effect_cannot_be_mistaken_for_a_drained_transport',url(port,'mail_uncertain'),pki)
            f.report['checks'].append('private_gate_drain_stale_resume_restart_quarantine')
            key_material=p/'mail.key';key_material.write_text(secrets.token_hex(32));key_material.chmod(0o600)
            reference=str(uuid.uuid4());extra={'WEBDOCK_MAIL_RECOVERY_KEY_FILE':str(key_material),'WEBDOCK_MAIL_RECOVERY_REFERENCE':reference}
            f.rust('private_snapshot_prepare',url(port,'mail_data'),pki,extra)
            f.report['mailSnapshotAt']=time.time()
            dump=p/'mail.dump';dump.write_bytes(f.docker('exec',source,'pg_dump','-U','postgres','-d','mail_data','--format=custom','--no-owner').stdout);dump.chmod(0o600)
            secret_plain=p/'recovery-material.json';secret_plain.write_text(json.dumps({'mailKey':key_material.read_text(),'metadataRole':'mail_owner','metadataPassword':role_password}));secret_plain.chmod(0o600)
            f.encrypt(secret_plain,p/'recovery-material.age',recipient);f.encrypt(dump,p/'mail.dump.age',recipient)
            f.report['mailOperatorRecoveryMaterial']={'bytes':(p/'recovery-material.age').stat().st_size,'sha256':profile.digest(p/'recovery-material.age'),'custody':'separate local operator age envelope; not production escrow or customer download'}
            f.object('upload',uploader_config,'mail-private',p/'mail.dump.age',p/'mail.receipt')
            f.report['mailDump']={'bytes':dump.stat().st_size,'sha256':profile.digest(dump)}
            f.docker('stop','--time','10',source);require(not f.inspect(source)['State']['Running'],'source_still_running')
            unavailable_at=time.monotonic()
            require(f.docker('exec',source,'pg_isready',allowed=True).returncode!=0,'source_still_available')
            dump.unlink();key_material.unlink();secret_plain.unlink()
            f.report['checks'].append('mail_source_unavailable_original_plaintext_removed')
            # Missing/wrong independent operator key cannot recover credentials.
            require(f.decrypt(p/'recovery-material.age',p/'bad-credentials',wrong,allowed=True).returncode!=0,'wrong_key_opened')
            require(f.decrypt(p/'recovery-material.age',p/'missing-credentials',p/'absent-key',allowed=True).returncode!=0,'missing_key_opened')
            f.decrypt(p/'recovery-material.age',p/'recovered-credentials',key)
            material=json.loads((p/'recovered-credentials').read_text());key_material.write_text(material['mailKey']);key_material.chmod(0o600)
            role_password=material['metadataPassword']
            # Backup principal is disabled before any recovery read; restore principal has no write.
            mc('admin','user','disable','fixture',uploader)
            require(f.object('download',uploader_config,'mail-private',p/'denied-download',p/'mail.receipt',allowed=True).returncode!=0,'revoked_writer_read')
            require(f.object('upload',reader_config,'mail-private',p/'mail.dump.age',p/'denied-write.receipt',allowed=True).returncode!=0,'reader_wrote')
            f.object('download',reader_config,'mail-private',p/'restored.dump.age',p/'mail.receipt')
            corrupt=json.loads((p/'mail.receipt').read_text());corrupt['sha256']='0'*64
            (p/'corrupt.receipt').write_text(json.dumps(corrupt));(p/'corrupt.receipt').chmod(0o600)
            require(f.object('download',reader_config,'mail-private',p/'corrupt.partial',p/'corrupt.receipt',allowed=True).returncode!=0,'corrupt_receipt_downloaded')
            require(not (p/'corrupt.partial').exists(),'failed_download_left_target_bytes')
            f.decrypt(p/'restored.dump.age',p/'restored.dump',key)
            target,target_port=f.postgres('mail-target',pki)
            f.sql(target,f"CREATE ROLE mail_owner LOGIN PASSWORD '{role_password}'; CREATE DATABASE mail_data OWNER mail_owner;")
            f.docker('exec','-i',target,'pg_restore','-U','postgres','--role=mail_owner','-d','mail_data','--no-owner','--exit-on-error',data=(p/'restored.dump').read_bytes())
            f.rust('private_snapshot_verify',url(target_port,'mail_data'),pki,extra)
            f.report['mailSnapshotToVerifiedTargetSeconds']=round(time.time()-f.report['mailSnapshotAt'],3)
            f.report['mailSourceUnavailableToVerifiedTargetSeconds']=round(time.monotonic()-unavailable_at,3)
            f.report['mailRPO']='one encrypted private payload; no writes after fence; no canonical Core/CE recovery claim'
            f.report['checks']+=['mail_private_fresh_pg_tls_payload_owner_wrong_key_closed_dispatch','separate_readonly_restore_and_revoked_writer']
            # Re-enable only the fixture upload principal for the separate application set.
            mc('admin','user','enable','fixture',uploader)
            run_ntfy(f,pki,recipient,key,wrong,uploader_config,reader_config)
        except Exception:
            # Diagnostics stay private for inspection after exact owned cleanup.
            diagnostic=Path(tempfile.mkdtemp(prefix='wd-recovery-failed-'));diagnostic.chmod(0o700)
            for item in f.containers:
                if 'ntfy-source' in item['name'] and item['id']:
                    (diagnostic/'ntfy-private.log').write_bytes(f.docker('logs',item['id'],allowed=True).stdout)
            for name in ('http-client-private.log','private-diagnostics.log','custody.json'):
                if (p/name).exists():shutil.copyfile(p/name,diagnostic/name)
            print('Private failed-fixture diagnostics: '+str(diagnostic),file=sys.stderr)
            raise
        finally:
            f.cleanup()
            cleanup_errors=[]
            if readonly_created:
                for action in ('disable','remove'):
                    try:mc('admin','user',action,'fixture',readonly)
                    except Exception:cleanup_errors.append('restore-user-'+action)
            if policy_created:
                try:mc('admin','policy','remove','fixture',policy_name)
                except Exception:cleanup_errors.append('restore-policy-remove')
            require(not cleanup_errors,'readonly_principal_cleanup_failed_'+','.join(cleanup_errors))
        f.report['cleanup']['privateDirectoryRemovedOnReturn']=True
        (ROOT/'docs/verification/2026-10-25-mail-application-recovery.json').write_text(json.dumps(f.report,indent=2)+'\n')
        print('Owned private-Mail and ntfy data recovery proof passed; exact cleanup completed.')


def run_ntfy(f,pki,recipient,key,wrong,uploader_config,reader_config):
    p=f.root;source_data=p/'ntfy-source';source_data.mkdir(mode=0o700)
    password=secrets.token_hex(24)
    config=p/'ntfy-server.yml'
    config.write_text('base-url: "https://localhost:8443"\nlisten-http: "-"\nlisten-https: ":8443"\nkey-file: "/tls/server.key"\ncert-file: "/tls/server.crt"\ncache-file: "/var/lib/ntfy/cache.db"\nauth-file: "/var/lib/ntfy/auth.db"\nauth-default-access: "deny-all"\nattachment-cache-dir: "/var/lib/ntfy/attachments"\ncache-duration: "24h"\nattachment-expiry-duration: "24h"\n')
    config.chmod(0o600);source=f.ntfy('ntfy-source',source_data,config,pki)
    # ntfy's own supported CLI stores the real bcrypt credential, never SQL repairs.
    f.docker('exec','--env','NTFY_PASSWORD',source,'ntfy','user','add','--role=admin','owner',env={'NTFY_PASSWORD':password})
    until=time.monotonic()+30
    while True:
        health=f.http(source,'/v1/health',allowed=True)
        if health and health.get('status')==200:break
        require(time.monotonic()<until,'ntfy_start_deadline');time.sleep(.2)
    require(f.http(source,'/recovery/json?poll=1')['status']==403,'ntfy_anonymous_access')
    attachment=b'owned ntfy attachment '+f.run.encode()
    response=f.http(source,'/recovery',method='PUT',body=attachment,headers={'Filename':'sentinel.txt','Content-Type':'application/octet-stream'},password=password)
    require(response['status']==200,'ntfy_attachment_publish_failed')
    notification=json.loads(base64.b64decode(response['body']));attachment_url=notification['attachment']['url']
    require(attachment_url.startswith('https://localhost:8443/file/'),'ntfy_logical_origin_changed')
    attachment_path=attachment_url.removeprefix('https://localhost:8443')
    attachment_permission=f.http(source,attachment_path)['status']
    f.docker('stop','--time','10',source)
    identity={'volumeUUID':str(uuid.uuid4()),'owner':f.run,'bytes':67108864}
    def stopped():
        state=f.inspect(source);require(not state['State']['Running'],'ntfy_source_resumed')
        require(any(m['Source']==str(source_data) and m['Destination']=='/var/lib/ntfy' for m in state['Mounts']),'ntfy_volume_owner_changed')
    started=time.monotonic()
    receipt=profile.snapshot(source_data,p/'ntfy-snapshot',identity,{'image':NTFY,'profile':profile.PROFILE},stopped,reserved_bytes=134217728)
    archive=p/'ntfy-data.tar'
    with tarfile.open(archive,'w') as tar:
        for file in sorted((p/'ntfy-snapshot').rglob('*')):
            if file.is_file():tar.add(file,arcname=file.relative_to(p/'ntfy-snapshot').as_posix(),recursive=False)
    config_material=p/'ntfy-material.json';config_material.write_text(json.dumps({'configuration':config.read_text(),'password':password,'tlsKey':(pki/'server.key').read_text()}));config_material.chmod(0o600)
    f.encrypt(config_material,p/'ntfy-material.age',recipient);f.encrypt(archive,p/'ntfy-data.age',recipient)
    f.report['ntfyOperatorRecoveryMaterial']={'bytes':(p/'ntfy-material.age').stat().st_size,'sha256':profile.digest(p/'ntfy-material.age'),'custody':'separate local operator age envelope with environment/TLS recovery; not production escrow'}
    f.object('upload',uploader_config,'ntfy',p/'ntfy-data.age',p/'ntfy.receipt')
    # Retain the source directory under its original custody, but remove its container
    # and local staging so recovery depends on the object and independent key.
    f.docker('rm',source);source_data.rename(p/'ntfy-source-unavailable');shutil.rmtree(p/'ntfy-snapshot');archive.unlink();config.unlink();config_material.unlink();(pki/'server.key').unlink()
    unavailable_at=time.monotonic()
    require(f.docker('inspect',source,allowed=True).returncode!=0,'ntfy_source_available')
    require(f.decrypt(p/'ntfy-material.age',p/'ntfy-wrong',wrong,allowed=True).returncode!=0,'ntfy_wrong_key_opened')
    require(f.decrypt(p/'ntfy-material.age',p/'ntfy-missing',p/'absent-key',allowed=True).returncode!=0,'ntfy_missing_key_opened')
    f.decrypt(p/'ntfy-material.age',p/'ntfy-recovered-material',key)
    material=json.loads((p/'ntfy-recovered-material').read_text());require(material['password']==password,'ntfy_credentials_changed')
    config.write_text(material['configuration']);config.chmod(0o600)
    (pki/'server.key').write_text(material['tlsKey']);(pki/'server.key').chmod(0o600)
    f.object('download',reader_config,'ntfy',p/'ntfy-restored.age',p/'ntfy.receipt');f.decrypt(p/'ntfy-restored.age',p/'ntfy-restored.tar',key)
    restored=p/'ntfy-restored-set';restored.mkdir(mode=0o700)
    with tarfile.open(p/'ntfy-restored.tar','r:') as tar:
        total=0;seen=set()
        for member in tar:
            name=Path(member.name);require(member.isfile() and not name.is_absolute() and '..' not in name.parts and member.name not in seen,'unsafe_recovery_archive')
            seen.add(member.name);total+=member.size;require(total<=67108864 and len(seen)<=10001,'recovery_archive_limit')
            out=restored/name;out.parent.mkdir(mode=0o700,exist_ok=True)
            profile.write(out,tar.extractfile(member).read())
    target_data=p/'ntfy-target';target_data.mkdir(mode=0o700)
    target_identity={**identity,'volumeUUID':str(uuid.uuid4())}
    profile.restore(restored,target_data,target_identity,receipt,reserved_bytes=134217728)
    # Profile receipts remain outside the workload's data selection on future backups.
    target=f.ntfy('ntfy-target',target_data,config,pki)
    require(f.inspect(target)['HostConfig']['NetworkMode']=='none','target_egress_not_disabled')
    routes=f.docker('exec',target,'cat','/proc/net/route').stdout.decode().splitlines()
    require(not any(line.split()[1]=='00000000' for line in routes[1:] if len(line.split())>1),'target_default_route_present')
    wrong_tls=f.http(target,'/v1/health',wrong_ca=True);require(wrong_tls.get('tlsRejected'),'target_wrong_ca_not_rejected')
    require(f.http(target,'/recovery/json?poll=1')['status']==403,'target_anonymous_access')
    require(f.http(target,'/recovery/json?poll=1',password='wrong')['status']==401,'target_wrong_password_access')
    rows=f.http(target,'/recovery/json?poll=1',password=password);require(rows['status']==200,'target_auth_failed')
    messages=[json.loads(line) for line in base64.b64decode(rows['body']).splitlines()]
    require(any(x.get('id')==notification['id'] and x.get('attachment',{}).get('url')==attachment_url for x in messages),'target_attachment_reference_missing')
    fetched=f.http(target,attachment_path,password=password)
    require(fetched['status']==200 and base64.b64decode(fetched['body'])==attachment,'fresh_target_attachment_bytes_changed')
    require(f.http(target,attachment_path)['status']==attachment_permission,'attachment_url_permissions_changed')
    with closing(sqlite3.connect((target_data/'cache.db').as_uri()+'?mode=ro',uri=True)) as db:
        require(db.execute('PRAGMA integrity_check').fetchall()==[('ok',)],'restored_application_sql_invalid')
    f.report['ntfy']={'image':NTFY,'profile':profile.PROFILE,'sourceUnavailable':True,'sourceRetained':True,'freshVolumeUUID':target_identity['volumeUUID']!=identity['volumeUUID'],'nativePoolAllocationClaimed':False,'logicalOriginPreserved':True,'targetTLSVerified':True,'wrongCARejected':True,'authenticatedRead':True,'anonymousAndWrongPasswordTopicReadDenied':True,'attachmentURLAnonymousStatusPreserved':attachment_permission,'attachmentBytes':len(attachment),'attachmentSHA256':hashlib.sha256(attachment).hexdigest(),'networkMode':'none','defaultRouteAbsent':True,'snapshotToVerifiedTargetSeconds':round(time.monotonic()-started,3),'rpo':'offline fence; one local cached attachment notification; no commercial SLA'}
    f.report['ntfy']['sourceUnavailableToVerifiedTargetSeconds']=round(time.monotonic()-unavailable_at,3)
    f.report['checks']+=['ntfy_source_unavailable_fresh_target_sql_auth_tls_attachment_origin_bytes_no_egress','independent_operator_age_key']


if __name__=='__main__':
    os.umask(0o077)
    require(len(sys.argv)==2,'pass_coordinated_recovery_object_executable')
    spec=importlib.util.spec_from_file_location('owned_s3',ROOT/'tools/tests/object-storage.py');storage=importlib.util.module_from_spec(spec);spec.loader.exec_module(storage)
    storage.main(callback=lambda config,mc,access:exercise(config,mc,access,sys.argv[1]))
