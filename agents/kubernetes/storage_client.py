"""Private local storage RPC; the normal hosting agent keeps its capability sandbox."""
import json
import os
import socket
import stat


def request(action, value, socket_path=None):
    path=socket_path or os.environ.get('SPITZLI_STORAGE_SOCKET','/run/webdock-storage/control.sock')
    info=os.stat(path,follow_symlinks=False)
    if not stat.S_ISSOCK(info.st_mode) or info.st_uid!=0 or info.st_mode & 0o077:raise ValueError('Unsafe storage socket')
    with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as client:
        client.settimeout(100);client.connect(path)
        client.sendall(json.dumps({'action':action,'value':value}).encode()+b'\n')
        with client.makefile('rb') as stream:data=stream.readline(8193)
    if not data:raise ConnectionError('Storage peer closed without a response')
    if len(data)>8192:raise ValueError('Invalid storage response')
    result=json.loads(data)
    if result.get('ok') is not True:raise ValueError('Storage operation failed')
    return result['value']


def packet(p):
    return {**{k:p[k] for k in ['appID','projectID','clusterID','revision','leaseUntil']},'spec':{'volumeBytes':p['spec']['volumeBytes']}}


def ensure(p):return request('ensure',packet(p))
def retained(p):return request('retained',packet(p))
def purge(p):return request('purge',packet(p))


def capability(nodes):
    try:return request('capability',nodes)
    except (OSError,ValueError,KeyError):return None
