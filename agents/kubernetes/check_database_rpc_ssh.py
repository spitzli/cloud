"""Disposable root-container proof of Unix -> real SSH -> DB-only forced RPC.

Requires openssh client/server in the fixture image. No production network,
credentials, mounts, databases or filesystems are used; all ledgers are purged.
"""
import json
import os
from pathlib import Path
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
from contextlib import contextmanager
from datetime import datetime, timezone, timedelta
import databases


@contextmanager
def server(path, handler):
    with socketserver.UnixStreamServer(str(path), handler) as listener:
        path.chmod(0o600)
        thread = threading.Thread(target=listener.serve_forever, daemon=True)
        thread.start()
        try:yield
        finally:listener.shutdown();thread.join()
    path.unlink()


def wait_socket(path, process):
    for _ in range(100):
        if path.exists():return
        if process.poll() is not None:raise AssertionError('Fixture helper exited')
        time.sleep(.02)
    raise AssertionError('Fixture helper did not become ready')


def main():
    if os.geteuid() != 0:raise RuntimeError('Use a disposable root container')
    processes = []
    log = None
    socket_root = Path('/run/webdock-postgres-storage')
    socket_root.mkdir(mode=0o700)
    source = Path(__file__).parent
    with tempfile.TemporaryDirectory(prefix='db-rpc-ssh-', dir='/run') as directory:
        root = Path(directory)
        try:
            routes = []
            for i, pool_id in enumerate(['bd0ba8e4-e801-4c16-bf1b-060129437ccb','cc0ba8e4-e801-4c16-bf1b-060129437ccb']):
                pool = root / str(i);pool.mkdir(mode=0o700);(pool / '_pool').mkdir(mode=0o700)
                identity = str(100+i);(pool / identity).mkdir(mode=0o700)
                record = {'appID':identity,'projectID':identity,'clusterID':'456','bytes':67108864,'uuid':pool_id,'state':'purged'}
                ledger = pool / '_pool/state.json';ledger.write_text(json.dumps({'poolID':pool_id,'volumes':{identity:record}}));ledger.chmod(0o600)
                config = root / ('pool-'+str(i)+'.json')
                config.write_text(json.dumps({'root':str(pool),'node':'pool-'+str(i),'clusterID':'456','poolID':pool_id,'capacityBytes':1073741824,'reserveBytes':67108864}));config.chmod(0o600)
                path = socket_root / ('primary.sock' if i == 0 else 'control.sock')
                process = subprocess.Popen([sys.executable,str(source/'storage_service.py')], env={**os.environ,'SPITZLI_STORAGE_CONFIG':str(config),'SPITZLI_STORAGE_SOCKET':str(path),'SPITZLI_DATABASE_STORAGE':'1'}, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                processes.append(process);wait_socket(path,process)
                routes.append({'node':'pool-'+str(i),'poolID':pool_id,'storageSocket':str(path)})
            denied = subprocess.run([sys.executable,str(source/'database_storage_rpc.py')],input=b'{"action":"database.capability","value":[]}\n',capture_output=True,user=65532,group=65532,timeout=5)
            assert denied.returncode != 0 and json.loads(denied.stdout) == {'ok':False,'value':None}
            for key in ['host','client']:
                subprocess.run(['ssh-keygen','-q','-t','ed25519','-N','','-f',str(root/key)],check=True,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
            authorized = root/'authorized_keys'
            authorized.write_text('restrict,from="127.0.0.1",command="'+sys.executable+' '+str(source/'database_storage_rpc.py')+'" '+(root/'client.pub').read_text());authorized.chmod(0o600)
            known = root/'known_hosts';known.write_text('[127.0.0.1]:22222 '+(root/'host.pub').read_text());known.chmod(0o600)
            config = root/'sshd_config'
            config.write_text(f'Port 22222\nListenAddress 127.0.0.1\nHostKey {root}/host\nPidFile {root}/sshd.pid\nAuthorizedKeysFile {authorized}\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPermitRootLogin prohibit-password\nUsePAM no\nAllowUsers root\nStrictModes yes\nLogLevel ERROR\n')
            log = open(root/'sshd.log','wb');os.chmod(log.name,0o600)
            sshd = subprocess.Popen(['/usr/sbin/sshd','-D','-e','-f',str(config)],stdout=subprocess.DEVNULL,stderr=log)
            processes.append(sshd)
            for _ in range(100):
                if sshd.poll() is not None:raise AssertionError('Disposable sshd failed')
                try:
                    with socket.create_connection(('127.0.0.1',22222),timeout=.1):break
                except OSError:time.sleep(.02)
            else:raise AssertionError('Disposable sshd timed out')
            ssh = ['ssh','-T','-p','22222','-i',str(root/'client'),'-o','IdentitiesOnly=yes','-o','IdentityAgent=none','-o','BatchMode=yes','-o','StrictHostKeyChecking=yes','-o','UserKnownHostsFile='+str(known),'-o','ConnectTimeout=2','root@127.0.0.1']
            probe = subprocess.run(ssh+['untrusted-client-command-must-not-run'],input=b'{"action":"database.capability","value":["pool-1"]}\n',capture_output=True,timeout=15)
            if probe.returncode != 0:raise AssertionError('SSH fixture failed: '+probe.stderr.decode(errors='replace')[:300])
            assert json.loads(probe.stdout)['ok'] is True
            class SSH(socketserver.BaseRequestHandler):
                def handle(self):
                    # Match systemd Accept=yes + StandardInput/Output=socket.
                    subprocess.run(ssh,stdin=self.request,stdout=self.request,stderr=subprocess.DEVNULL,timeout=15)
            proxy = socket_root/'node01.sock'
            configured = {'storageSocket':routes[0]['storageSocket'],'additionalStoragePools':[{**routes[1],'storageSocket':str(proxy)}]}
            nodes = [r['node'] for r in routes]
            packet = lambda i:{'version':1,'installationID':str(100+i),'clusterID':'456','revision':2,'node':routes[i]['node'],'poolID':routes[i]['poolID'],'volumeBytes':67108864,'leaseUntil':(datetime.now(timezone.utc)+timedelta(minutes=2)).isoformat()}
            def primary_works():
                for action in ['database.state','database.retained','database.purge']:
                    result=databases.storage_request(configured,action,packet(0))
                    if action=='database.state':assert result['state']=='purged'
            with server(proxy,SSH):
                observed=databases.storage_pools(configured,nodes)
                assert [p['poolID'] for _,p in observed]==[r['poolID'] for r in routes]
                for action in ['database.state','database.retained','database.purge']:
                    result=databases.storage_request(configured,action,packet(1))
                    if action=='database.state':assert result['state']=='purged'
                for body in [b'not-json\n',json.dumps({'action':'inventory','value':'private-inline-secret'}).encode()+b'\n',b'x'*4097+b'\n']:
                    reply=subprocess.run(ssh,input=body,capture_output=True,timeout=15)
                    assert reply.returncode!=0 and json.loads(reply.stdout)=={'ok':False,'value':None}
                    assert b'private-inline-secret' not in reply.stdout
                processes[1].terminate();processes[1].wait(timeout=3)
                Path(routes[1]['storageSocket']).unlink()
                unavailable=subprocess.run(ssh,input=b'{"action":"database.capability","value":["pool-1"]}\n',capture_output=True,timeout=15)
                assert unavailable.returncode!=0 and unavailable.stdout==b''
                assert [p['poolID'] for _,p in databases.storage_pools(configured,nodes)]==[routes[0]['poolID']]
                primary_works()
                try:databases.storage_request(configured,'database.retained',packet(1))
                except ConnectionError:pass
                else:raise AssertionError('Offline additional mutation did not fail as unavailable')
                class Malformed(socketserver.StreamRequestHandler):
                    def handle(self):self.rfile.readline(4097);self.wfile.write(b'not-json\n')
                with server(Path(routes[1]['storageSocket']),Malformed):
                    try:databases.storage_pools(configured,nodes)
                    except ValueError:pass
                    else:raise AssertionError('Nonempty malformed SSH helper reply was hidden')
                sshd.terminate();sshd.wait(timeout=3)
                assert [p['poolID'] for _,p in databases.storage_pools(configured,nodes)]==[routes[0]['poolID']]
                primary_works()
            print(json.dumps({'passed':True,'realSSHForcedCommand':True,'rootOnlyPrivateUnixSockets':True,'checks':['non-root rejected','client command cannot bypass forced wrapper','exact remote DB payload','remote state/retain/purge','malformed/oversize/app RPC rejected nonempty','missing remote helper exits empty','additional pool EOF omitted without primary loss','malformed helper hard-fails','SSH listener disconnect EOF preserves primary'],'productionMutations':False}))
        finally:
            for process in reversed(processes):
                if process.poll() is None:
                    process.terminate()
                    try:process.wait(timeout=3)
                    except subprocess.TimeoutExpired:process.kill();process.wait()
            if log is not None:log.close()


if __name__=='__main__':main()
