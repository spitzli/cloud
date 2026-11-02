"""Root-only storage helper in the host mount namespace; no network listener."""
from datetime import datetime, timezone
import json
import fcntl
import os
from pathlib import Path
import re
import socket
import socketserver
import struct
import storage


def dispatch(message):
    if set(message)!={'action','value'}:raise ValueError('Invalid request')
    action,value=message['action'],message['value']
    if action in ['database.capability','database.ensure','database.retained','database.purge','database.state']:
        from database_storage import dispatch as database_dispatch
        return database_dispatch(action,value)
    if action=='info':
        if value is not None:raise ValueError('Invalid information request')
        return storage.configuration()
    if action=='capability':
        if not isinstance(value,list) or len(value)>200 or any(not isinstance(v,str) or len(v)>253 for v in value):raise ValueError('Invalid nodes')
        capacity=storage.capability(value)
        return {"capacityBytes":capacity or 0,"ready":capacity is not None}
    if action not in ['ensure','retained','purge','state'] or not isinstance(value,dict) or set(value)!={'appID','projectID','clusterID','revision','leaseUntil','spec'}:raise ValueError('Invalid storage operation')
    for key in ['appID','projectID','clusterID']:
        if not isinstance(value[key],str) or not re.fullmatch(r'[1-9][0-9]{0,18}',value[key]):raise ValueError('Invalid owner')
    if type(value['revision']) is not int or value['revision']<1:raise ValueError('Invalid revision')
    if datetime.fromisoformat(value['leaseUntil'].replace('Z','+00:00'))<=datetime.now(timezone.utc):raise ValueError('Expired lease')
    if not isinstance(value['spec'],dict) or set(value['spec'])!={'volumeBytes'} or type(value['spec']['volumeBytes']) is not int or not 67108864<=value['spec']['volumeBytes']<=1099511627776:raise ValueError('Invalid size')
    return getattr(storage,action)(value)


class Handler(socketserver.StreamRequestHandler):
    def handle(self):
        self.connection.settimeout(10)
        if struct.unpack('3i',self.connection.getsockopt(socket.SOL_SOCKET,socket.SO_PEERCRED,12))[1]!=0:return
        try:
            raw=self.rfile.readline(4097)
            if len(raw)>4096 or not raw.endswith(b'\n'):raise ValueError('Oversized request')
            result={'ok':True,'value':dispatch(json.loads(raw))}
        except Exception:result={'ok':False,'value':None}
        self.wfile.write(json.dumps(result).encode()+b'\n')


def main():
    if os.geteuid()!=0:raise SystemExit('Root storage service required')
    config=storage.configuration()
    lock=os.open(Path(config['root'])/'_pool'/'service.lock',os.O_WRONLY|os.O_CREAT|os.O_NOFOLLOW,0o600)
    fcntl.flock(lock,fcntl.LOCK_EX|fcntl.LOCK_NB)
    storage.restore()
    path=Path(os.environ.get('WEBDOCK_STORAGE_SOCKET','/run/webdock-storage/control.sock'))
    storage.private(path.parent,directory=True)
    if path.exists():
        if not path.is_socket() or path.is_symlink() or path.stat().st_uid!=0:raise SystemExit('Unsafe socket path')
        path.unlink()
    os.umask(0o077)
    with socketserver.UnixStreamServer(str(path),Handler) as server:
        address=os.environ.get('NOTIFY_SOCKET')
        if address:
            with socket.socket(socket.AF_UNIX,socket.SOCK_DGRAM) as notify:
                notify.connect('\0'+address[1:] if address.startswith('@') else address);notify.sendall(b'READY=1')
        server.serve_forever()


if __name__=='__main__':main()
