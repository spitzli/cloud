"""Root-only disposable multi-pool Unix-socket acceptance; no mounts or real databases.

Run in a disposable root container with this directory mounted read-only. Uses
real storage_service handlers, peer credentials, private sockets and pool ledgers.
Only premarked purged fixture allocations are present; ensure must reject them.
"""
import json
import os
from pathlib import Path
import subprocess
import socketserver
import threading
from contextlib import contextmanager
import sys
import tempfile
import time
from datetime import datetime, timezone, timedelta
from unittest.mock import patch
import databases


@contextmanager
def proxy_response(path,response):
    class Handler(socketserver.StreamRequestHandler):
        def handle(self):
            self.rfile.readline(4097)
            if response:self.wfile.write(response)
    with socketserver.UnixStreamServer(path,Handler) as server:
        os.chmod(path,0o600)
        thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
        try:yield
        finally:
            server.shutdown();thread.join()
    Path(path).unlink()


def main():
    if os.geteuid()!=0:raise RuntimeError('Run this acceptance in a disposable root container')
    processes=[];socket_root=Path('/run/webdock-postgres-storage');socket_root.mkdir(mode=0o700)
    try:
        with tempfile.TemporaryDirectory(prefix='database-pool-rpc-') as directory:
            root=Path(directory);routes=[]
            for index,pool_id in enumerate(['bd0ba8e4-e801-4c16-bf1b-060129437ccb','cc0ba8e4-e801-4c16-bf1b-060129437ccb']):
                pool=root/str(index);pool.mkdir(mode=0o700);(pool/'_pool').mkdir(mode=0o700)
                installation=str(100+index);(pool/installation).mkdir(mode=0o700)
                config={'root':str(pool),'node':'pool-'+str(index),'clusterID':'456','poolID':pool_id,'capacityBytes':1073741824,'reserveBytes':67108864}
                ledger={'poolID':pool_id,'volumes':{installation:{'appID':installation,'projectID':installation,'clusterID':'456','bytes':67108864,'uuid':pool_id,'state':'purged'}}}
                config_path=root/('config-'+str(index)+'.json');config_path.write_text(json.dumps(config));config_path.chmod(0o600)
                state=pool/'_pool'/'state.json';state.write_text(json.dumps(ledger));state.chmod(0o600)
                socket=str(socket_root/('pool-'+str(index)+'.sock'))
                process=subprocess.Popen([sys.executable,str(Path(__file__).with_name('storage_service.py'))],env={**os.environ,'SPITZLI_STORAGE_CONFIG':str(config_path),'SPITZLI_STORAGE_SOCKET':socket,'SPITZLI_DATABASE_STORAGE':'1'},stdout=subprocess.PIPE,stderr=subprocess.PIPE)
                processes.append(process);routes.append({'node':config['node'],'poolID':pool_id,'storageSocket':socket})
                deadline=time.monotonic()+5
                while not Path(socket).exists():
                    if process.poll() is not None:raise RuntimeError('Fixture storage helper failed')
                    if time.monotonic()>deadline:raise RuntimeError('Fixture storage helper timed out')
                    time.sleep(.02)
            configured={'storageSocket':routes[0]['storageSocket'],'additionalStoragePools':[routes[1]]}
            nodes=[r['node'] for r in routes]
            observed=databases.storage_pools(configured,nodes)
            assert [p['poolID'] for _,p in observed]==[r['poolID'] for r in routes]
            with patch.object(databases,'kubectl',return_value=json.dumps({'items':[{'metadata':{'name':n}} for n in nodes]})):
                for index,route in enumerate(routes):
                    value={'version':1,'installationID':str(100+index),'clusterID':'456','revision':2,'node':route['node'],'poolID':route['poolID'],'volumeBytes':67108864,'leaseUntil':(datetime.now(timezone.utc)+timedelta(minutes=2)).isoformat()}
                    assert databases.storage_request(configured,'database.state',value)=={'state':'purged','volumeBytes':67108864}
                    databases.storage_request(configured,'database.retained',value)
                    databases.storage_request(configured,'database.purge',value)
                    try:databases.storage_request(configured,'database.ensure',value)
                    except ValueError:pass
                    else:raise AssertionError('Purged storage was recreated')
                    assert databases.storage_request(configured,'database.state',value)['state']=='purged'
                try:databases.storage_request(configured,'database.purge',{**value,'node':routes[0]['node']})
                except ValueError:pass
                else:raise AssertionError('Cross-pool node substitution was accepted')
                # A stopped additional helper cannot make the healthy primary depend on it.
                processes[1].terminate();processes[1].wait(timeout=3)
                Path(routes[1]['storageSocket']).unlink()
                primary={**value,'installationID':'100','node':routes[0]['node'],'poolID':routes[0]['poolID']}
                assert databases.storage_request(configured,'database.state',primary)['state']=='purged'
                databases.storage_request(configured,'database.retained',primary)
                databases.storage_request(configured,'database.purge',primary)
                assert [p['poolID'] for _,p in databases.storage_pools(configured,nodes)]==[routes[0]['poolID']]
                try:databases.storage_request(configured,'database.state',value)
                except (FileNotFoundError,ConnectionError):pass
                else:raise AssertionError('Offline additional pool fell back to primary')
                try:databases.storage_request(configured,'database.state',{**primary,'node':'unknown-node','poolID':'dd0ba8e4-e801-4c16-bf1b-060129437ccb'})
                except ValueError:pass
                else:raise AssertionError('Unknown pool was accepted by primary')
            # A root-only local proxy can accept the request but lose its SSH peer.
            with proxy_response(routes[1]['storageSocket'],b''):
                assert [p['poolID'] for _,p in databases.storage_pools(configured,nodes)]==[routes[0]['poolID']]
                assert databases.storage_request(configured,'database.state',primary)['state']=='purged'
                try:databases.storage_request(configured,'database.retained',value)
                except ConnectionError:pass
                else:raise AssertionError('Zero-byte mutation response was not ambiguous/unavailable')
            with proxy_response(routes[1]['storageSocket'],b'not-json\n'):
                try:databases.storage_pools(configured,nodes)
                except ValueError:pass
                else:raise AssertionError('Malformed nonempty response was hidden as unavailability')
            print(json.dumps({'passed':True,'actualPrivateUnixSocketPools':2,'rootPeerAuthentication':True,'checks':['exact state routing','retained routing','idempotent purged routing','no empty replacement','cross-pool denial','primary survives offline extra','offline extra has no fallback','unknown pool rejected','zero-byte proxy EOF omits only extra','malformed nonempty proxy response rejected']}))
    finally:
        for process in processes:
            process.terminate()
            try:process.wait(timeout=3)
            except subprocess.TimeoutExpired:process.kill();process.wait()

if __name__=='__main__':main()
