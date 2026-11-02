#!/usr/bin/env python3
"""Nonsecret connectivity smoke on the qualified host; no tenant requests/data."""
import json
import subprocess

PROGRAM=r'''
import errno,json,socket,struct,urllib.request
results=[]
for host,port in [('127.0.0.1',22),('167.86.75.144',22),('10.83.0.1',6443),('79.143.177.157',5432),('169.254.169.254',80),('::1',22),('2a02:c207:3021:866::1',22)]:
 try:
  connection=socket.create_connection((host,port),2);connection.close();results.append({'target':host+':'+str(port),'blocked':False})
 except OSError as error:results.append({'target':host+':'+str(port),'blocked':error.errno in [errno.EACCES,errno.EPERM,errno.EHOSTUNREACH],'errno':error.errno})
query=struct.pack('!HHHHHH',1001,0x100,1,0,0,0)+b'\x06github\x03com\x00'+struct.pack('!HH',1,1)
for kind,label in [(socket.SOCK_DGRAM,'udp'),(socket.SOCK_STREAM,'tcp')]:
 with socket.socket(socket.AF_INET,kind) as stream:
  stream.settimeout(5);stream.connect(('127.0.0.53',53));stream.sendall((struct.pack('!H',len(query)) if kind==socket.SOCK_STREAM else b'')+query)
  result=stream.recv(4096);results.append({'stubDNS':label,'response':len(result)>12})
for url in ['https://api.github.com','https://registry.webdock.dev/.well-known/registry-key.json']:
 with urllib.request.urlopen(urllib.request.Request(url,headers={'User-Agent':'Spitzli-CI-network-qualification'}),timeout=15) as response:results.append({'url':url,'status':response.status})
print(json.dumps(results))
assert all(item.get('blocked',True) and item.get('response',True) and item.get('status',200)==200 for item in results)
'''


def counters():
    data=json.loads(subprocess.check_output(['/usr/sbin/nft','--json','list','table','inet','spitzli_plugin_ci']))
    result={}
    for entry in data['nftables']:
        rule=entry.get('rule',{})
        if 'comment' in rule:
            result[rule['comment']]=sum(expression.get('counter',{}).get('packets',0) for expression in rule['expr'])
    return result

before=counters()
result=subprocess.run(['/usr/sbin/runuser','-u','spitzli-plugin-ci','--','python3','-c',PROGRAM],capture_output=True,text=True,check=True)
after=counters()
deltas={name:after[name]-before.get(name,0) for name in after}
assert deltas['CI private metadata and platform IPv4 denied']>=5
assert deltas['CI private metadata and platform IPv6 denied']>=2
assert deltas['Exact host stub DNS']>=1 and deltas['Exact host stub DNS TCP']>=1
print(json.dumps({'probes':json.loads(result.stdout),'nftCounterDeltas':deltas,'passed':True}))
