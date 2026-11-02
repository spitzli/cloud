#!/usr/bin/env python3
"""Owned BUILD-only qualification TLS relay. No HTTP parsing or body buffering."""
import argparse
import asyncio
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import socket
import ssl
import stat
import subprocess
import sys
import time

HOST = '167.86.75.144'
HOSTNAME = 'webdock-build-eu-01'
LISTEN = (HOST, 19444)
UPSTREAM = ('127.0.0.1', 19445)
CHUNK = 16 * 1024
LIFETIME = 300
SSH = ['ssh', '-o', 'BatchMode=yes', '-o', 'IdentityAgent=none', '-o', 'IdentitiesOnly=yes', '-o', 'ConnectTimeout=8', '-i', str(Path.home()/'.ssh/id_ed25519'), 'root@'+HOST]


def owner(value):
    if not re.fullmatch(r'native-[a-z0-9][a-z0-9-]{1,47}', value):
        raise ValueError('invalid_relay_owner')
    return value


def root_for(value):
    return Path('/root/webdock-build-upload-relay-' + owner(value))


def unit_for(value):
    return 'webdock-build-upload-relay-' + owner(value) + '.service'


def guard():
    marker=Path('/etc/webdock-build-host')
    if os.geteuid()!=0 or socket.gethostname()!=HOSTNAME or marker.is_symlink() or not marker.is_file() or marker.stat().st_uid!=0:
        raise ValueError('owned_build_host_required')


def private_directory(path):
    if not path.is_absolute() or path.resolve()!=path:
        raise ValueError('unsafe_relay_directory')
    info=path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid!=os.geteuid() or info.st_mode & 0o777!=0o700:
        raise ValueError('unsafe_relay_directory')


def private_read(path, maximum=64*1024):
    fd=os.open(path,os.O_RDONLY|os.O_NOFOLLOW)
    with os.fdopen(fd,'rb') as source:
        info=os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode) or info.st_uid!=os.geteuid() or info.st_nlink!=1 or info.st_mode & 0o777!=0o600 or info.st_size>maximum:
            raise ValueError('unsafe_relay_file')
        data=source.read(maximum+1)
        if len(data)>maximum: raise ValueError('relay_file_limit')
        return data


def digest(data):
    return hashlib.sha256(data).hexdigest()


def write_new(path, data):
    fd=os.open(path,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
    with os.fdopen(fd,'wb') as target:
        target.write(data);target.flush();os.fsync(target.fileno())


def proc_start(pid):
    text=Path('/proc',str(pid),'stat').read_text()
    return int(text.rsplit(')',1)[1].split()[19])


def process_identity(pid, root, expected):
    if pid<=1 or proc_start(pid)!=expected['startTimeTicks']:
        raise ValueError('relay_process_identity_changed')
    command=Path('/proc',str(pid),'cmdline').read_bytes().split(b'\0')
    required=[b'/usr/bin/python3',b'-B',str(root/'relay.py').encode(),b'serve',b'--owner',expected['owner'].encode()]
    if command[:-1]!=required or expected['pid']!=pid or expected['bootID']!=Path('/proc/sys/kernel/random/boot_id').read_text().strip():
        raise ValueError('relay_process_identity_changed')


def command(args, timeout=20):
    result=subprocess.run(args,capture_output=True,timeout=timeout,check=False)
    if result.returncode: raise ValueError('relay_command_failed')
    return result.stdout.decode()


def systemd_state(unit, absent_ok=False):
    result=subprocess.run(['systemctl','show',unit,'--property=LoadState,ActiveState,MainPID,ControlGroup,FragmentPath,Transient,MemoryMax,TasksMax'],capture_output=True,timeout=10)
    fields=dict(line.split('=',1) for line in result.stdout.decode().splitlines() if '=' in line)
    if result.returncode and not (absent_ok and fields.get('LoadState')=='not-found'):
        raise ValueError('relay_command_failed')
    return fields


def verified_files(value, expected_sha):
    root=root_for(value);private_directory(root)
    record=json.loads(private_read(root/'ownership.json'))
    if record['owner']!=value or record['host']!=HOST or record['unit']!=unit_for(value) or record['scriptSHA256']!=expected_sha:
        raise ValueError('relay_owner_changed')
    for name, field in [('relay.py','scriptSHA256'),('server.crt','certificateSHA256'),('server.key','keySHA256')]:
        if digest(private_read(root/name))!=record[field]: raise ValueError('relay_file_changed')
    return root,record


def verified_status(value, expected_sha):
    root,record=verified_files(value,expected_sha)
    observed=systemd_state(record['unit'])
    if observed.get('Transient')!='yes' or observed.get('ActiveState')!='active' or observed.get('ControlGroup')!='/system.slice/'+record['unit']:
        raise ValueError('relay_unit_identity_changed')
    ready=json.loads(private_read(root/'process.json'))
    if ready['owner']!=value or ready['scriptSHA256']!=record['scriptSHA256']:
        raise ValueError('relay_process_identity_changed')
    process_identity(int(observed['MainPID']),root,ready)
    listeners=command(['ss','-H','-ltnp','sport = :19444'])
    if f'{HOST}:19444' not in listeners or f'pid={ready["pid"]},' not in listeners:
        raise ValueError('relay_listener_identity_changed')
    return {'owner':value,'unit':record['unit'],'pid':ready['pid'],'startTimeTicks':ready['startTimeTicks'],
            'listen':f'{HOST}:19444','upstream':'127.0.0.1:19445','active':True,'scriptSHA256':record['scriptSHA256'],
            'certificateSHA256':record['certificateSHA256'],'memoryMaxBytes':int(observed['MemoryMax']),'tasksMax':int(observed['TasksMax'])}


def unit_command(value):
    root=root_for(value)
    properties={
        'MemoryMax':'64M','MemorySwapMax':'0','TasksMax':'4','LimitNOFILE':'32','CPUQuota':'50%',
        'NoNewPrivileges':'yes','CapabilityBoundingSet':'','AmbientCapabilities':'','ProtectSystem':'strict',
        'ProtectHome':'read-only','ReadWritePaths':str(root),'PrivateTmp':'yes','PrivateDevices':'yes',
        'ProtectKernelTunables':'yes','ProtectKernelModules':'yes','ProtectControlGroups':'yes','RestrictNamespaces':'yes',
        'RestrictSUIDSGID':'yes','LockPersonality':'yes','MemoryDenyWriteExecute':'yes','RestrictAddressFamilies':'AF_INET AF_UNIX',
        'UMask':'0077','KillMode':'control-group','Restart':'no','RuntimeMaxSec':'7200','TimeoutStopSec':'5',
        'StandardOutput':'null','StandardError':'null',
    }
    args=['systemd-run','--quiet','--unit='+unit_for(value)]
    args += ['--property='+key+'='+val for key,val in properties.items()]
    return args+['/usr/bin/python3','-B',str(root/'relay.py'),'serve','--owner',value]


def manage(action, value, expected_sha):
    guard();root,record=verified_files(value,expected_sha)
    if action=='start':
        # Do not replace an existing unit or listener, including an uncertain prior start.
        existing=systemd_state(record['unit'],absent_ok=True)
        if existing.get('ActiveState')=='active': return verified_status(value,expected_sha)
        if existing.get('MainPID','0')!='0' or (root/'process.json').exists(): raise ValueError('relay_start_uncertain')
        if command(['ss','-H','-ltn','sport = :19444']).strip(): raise ValueError('relay_port_owned_elsewhere')
        command(unit_command(value))
        until=time.monotonic()+10
        while time.monotonic()<until:
            try: return verified_status(value,expected_sha)
            except (ValueError,FileNotFoundError): time.sleep(0.1)
        raise ValueError('relay_start_uncertain')
    if action=='status': return verified_status(value,expected_sha)
    if action=='stop':
        status=verified_status(value,expected_sha)  # No broad unit/pid kill on ownership uncertainty.
        command(['systemctl','stop',record['unit']])
        try:
            if proc_start(status['pid'])==status['startTimeTicks']: raise ValueError('relay_process_still_present')
        except FileNotFoundError: pass
        expected={'relay.py','server.crt','server.key','ownership.json','process.json'}
        if {path.name for path in root.iterdir()}!=expected: raise ValueError('relay_cleanup_scope_changed')
        for name in expected: (root/name).unlink()
        root.rmdir()
        return {'owner':value,'stopped':True,'exactPrivateFilesRemoved':True}
    raise ValueError('invalid_relay_action')


async def copy_stream(reader, writer, eof=False):
    while True:
        chunk=await reader.read(CHUNK)
        if not chunk: break
        writer.write(chunk)
        await writer.drain()
    if eof and writer.can_write_eof():
        writer.write_eof();await writer.drain()


async def relay_socket(connection, context):
    loop=asyncio.get_running_loop(); client=None; upstream=None; tasks=[]
    try:
        async with asyncio.timeout(LIFETIME):
            reader=asyncio.StreamReader(limit=CHUNK)
            protocol=asyncio.StreamReaderProtocol(reader)
            transport,_=await loop.connect_accepted_socket(lambda:protocol,connection,ssl=context,ssl_handshake_timeout=5,ssl_shutdown_timeout=2)
            client=asyncio.StreamWriter(transport,protocol,reader,loop)
            transport.set_write_buffer_limits(high=CHUNK,low=CHUNK//2)
            source,upstream=await asyncio.wait_for(asyncio.open_connection(*UPSTREAM,limit=CHUNK),timeout=3)
            upstream.transport.set_write_buffer_limits(high=CHUNK,low=CHUNK//2)
            tasks=[asyncio.create_task(copy_stream(reader,upstream,eof=True)),asyncio.create_task(copy_stream(source,client))]
            done,_=await asyncio.wait(tasks,return_when=asyncio.FIRST_COMPLETED)
            for completed in done: completed.result()
            if tasks[1] not in done: await tasks[1]
    except (OSError,ValueError,TimeoutError,ssl.SSLError,ConnectionError):
        pass  # Never emit raw TLS/network exceptions or any credential-bearing request data.
    finally:
        for task in tasks: task.cancel()
        await asyncio.gather(*tasks,return_exceptions=True)
        for writer in [upstream,client]:
            if writer is not None:
                writer.close()
                try: await asyncio.wait_for(writer.wait_closed(),timeout=2)
                except (OSError,TimeoutError,ssl.SSLError): pass
        connection.close()


async def serve(value):
    guard();root,record=verified_files(value,digest(private_read(root_for(value)/'relay.py')))
    context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);context.minimum_version=ssl.TLSVersion.TLSv1_2
    context.options|=ssl.OP_NO_COMPRESSION;context.set_alpn_protocols(['http/1.1'])
    context.load_cert_chain(root/'server.crt',root/'server.key')
    listener=socket.socket(socket.AF_INET,socket.SOCK_STREAM);listener.setsockopt(socket.SOL_SOCKET,socket.SO_REUSEADDR,1)
    listener.bind(LISTEN);listener.listen(4);listener.setblocking(False)
    process={'owner':value,'pid':os.getpid(),'startTimeTicks':proc_start(os.getpid()),'bootID':Path('/proc/sys/kernel/random/boot_id').read_text().strip(),'scriptSHA256':record['scriptSHA256']}
    write_new(root/'process.json',json.dumps(process).encode())
    loop=asyncio.get_running_loop()
    try:
        while True:
            connection,_=await loop.sock_accept(listener)
            connection.setblocking(False)
            # One accepted socket, one handshake and two bounded copy tasks; kernel backlog is four.
            await relay_socket(connection,context)
    finally: listener.close()


BOOTSTRAP = r'''
import base64,hashlib,json,os,re,socket,stat,subprocess,sys
from pathlib import Path
try:
    data=json.load(sys.stdin);value=data['owner']
    if not re.fullmatch(r'native-[a-z0-9][a-z0-9-]{1,47}',value):raise ValueError()
    marker=Path('/etc/webdock-build-host')
    if os.geteuid()!=0 or socket.gethostname()!='webdock-build-eu-01' or marker.is_symlink() or not marker.is_file() or marker.stat().st_uid!=0:raise ValueError()
    root=Path('/root/webdock-build-upload-relay-'+value)
    if data['action']=='start' and not root.exists():
        root.mkdir(mode=0o700)
        for name,field in [('relay.py','script'),('server.crt','certificate'),('server.key','key')]:
            raw=base64.b64decode(data[field],validate=True)
            if len(raw)>65536:raise ValueError()
            fd=os.open(root/name,os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
            with os.fdopen(fd,'wb') as output:output.write(raw);output.flush();os.fsync(output.fileno())
        record={'owner':value,'host':'167.86.75.144','unit':'webdock-build-upload-relay-'+value+'.service','scriptSHA256':data['expectedSHA'],'certificateSHA256':hashlib.sha256(base64.b64decode(data['certificate'])).hexdigest(),'keySHA256':hashlib.sha256(base64.b64decode(data['key'])).hexdigest()}
        fd=os.open(root/'ownership.json',os.O_WRONLY|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
        with os.fdopen(fd,'w') as output:json.dump(record,output);output.flush();os.fsync(output.fileno())
    info=root.lstat()
    if root.resolve()!=root or not stat.S_ISDIR(info.st_mode) or info.st_uid!=0 or info.st_mode&0o777!=0o700:raise ValueError()
    fd=os.open(root/'relay.py',os.O_RDONLY|os.O_NOFOLLOW)
    with os.fdopen(fd,'rb') as source:
        info=os.fstat(source.fileno());raw=source.read(65537)
        if not stat.S_ISREG(info.st_mode) or info.st_uid!=0 or info.st_nlink!=1 or info.st_mode&0o777!=0o600 or hashlib.sha256(raw).hexdigest()!=data['expectedSHA']:raise ValueError()
    result=subprocess.run(['/usr/bin/python3','-B',str(root/'relay.py'),'manage',data['action'],'--owner',value,'--expected-sha',data['expectedSHA']],capture_output=True,timeout=30)
    if result.returncode:raise ValueError()
    sys.stdout.buffer.write(result.stdout)
except Exception:
    print('relay_operation_failed',file=sys.stderr);sys.exit(1)
'''


def local(action,value,fixture):
    raw=Path(__file__).read_bytes()
    data={'action':action,'owner':owner(value),'expectedSHA':digest(raw)}
    if action=='start':
        fixture=fixture.resolve();private_directory(fixture)
        certificate=private_read(fixture/'server.crt');key=private_read(fixture/'server.key')
        context=ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER);context.load_cert_chain(fixture/'server.crt',fixture/'server.key')
        data.update(script=base64.b64encode(raw).decode(),certificate=base64.b64encode(certificate).decode(),key=base64.b64encode(key).decode())
    result=subprocess.run(SSH+['python3 -c '+shlex.quote(BOOTSTRAP)],input=json.dumps(data).encode(),capture_output=True,timeout=45)
    if result.returncode: raise ValueError('relay_remote_operation_failed')
    observed=json.loads(result.stdout)
    if action=='start' and observed['certificateSHA256']!=digest(certificate):
        raise ValueError('relay_existing_certificate_changed')
    return observed


def main():
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('action',choices=['start','status','stop','manage','serve'])
    parser.add_argument('remote_action',nargs='?',choices=['start','status','stop'])
    parser.add_argument('--owner',required=True,type=owner);parser.add_argument('--expected-sha')
    parser.add_argument('--fixture-root',type=Path,default=Path('.runtime-test/build-cluster/native-20261030-full1-staging'))
    args=parser.parse_args()
    if args.action=='serve': asyncio.run(serve(args.owner));return
    result=manage(args.remote_action,args.owner,args.expected_sha) if args.action=='manage' else local(args.action,args.owner,args.fixture_root)
    print(json.dumps(result,sort_keys=True))


if __name__=='__main__':
    try: main()
    except Exception:
        print('relay_operation_failed',file=sys.stderr);sys.exit(1)
