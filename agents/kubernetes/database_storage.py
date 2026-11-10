"""Versioned CNPG storage adapter for an operator-created, separate bounded pool."""
from datetime import datetime, timezone
from pathlib import Path
import re
import os
import uuid
import storage


def validate(value):
    fields={'version','installationID','clusterID','revision','node','poolID','volumeBytes','leaseUntil'}
    if not isinstance(value,dict) or set(value)!=fields:raise ValueError('Invalid database storage request')
    for key in ['installationID','clusterID']:
        if not isinstance(value[key],str) or not re.fullmatch(r'[1-9][0-9]{0,18}',value[key]) or int(value[key])>9223372036854775807:raise ValueError('Invalid storage identity')
    if type(value['version']) is not int or value['version']!=1:raise ValueError('Unsupported database storage version')
    if type(value['revision']) is not int or not 1<=value['revision']<=2147483646:raise ValueError('Invalid storage revision')
    if type(value['volumeBytes']) is not int or not 67108864<=value['volumeBytes']<=1099511627776:raise ValueError('Invalid database volume size')
    if not isinstance(value['node'],str) or not re.fullmatch(r'[a-z0-9][a-z0-9.-]{0,252}',value['node']):raise ValueError('Invalid storage node')
    if str(uuid.UUID(value['poolID']))!=value['poolID']:raise ValueError('Invalid pool identity')
    lease=datetime.fromisoformat(value['leaseUntil'].replace('Z','+00:00'))
    if lease.tzinfo is None or not 0<(lease-datetime.now(timezone.utc)).total_seconds()<=300:raise ValueError('Expired or invalid storage lease')
    return value


def ensure(value):
    value=validate(value);config=storage.configuration()
    if any(config[k]!=value[k] for k in ['clusterID','poolID','node']):raise ValueError('Database storage ownership changed')
    existing=value['installationID'] in storage.pool(config)['volumes']
    # The physical installation owns storage; Shared customer DBs do not own separate PVs.
    config,path=storage.ensure({'appID':value['installationID'],'projectID':value['installationID'],
        'clusterID':value['clusterID'],'revision':value['revision'],'spec':{'volumeBytes':value['volumeBytes']}})
    data=Path(path)
    if data.is_symlink() or not data.is_dir():raise ValueError('Invalid database data directory')
    if not existing:os.chown(data,26,26)
    info=data.stat()
    if info.st_uid!=26 or info.st_gid!=26:raise ValueError('Database filesystem ownership changed')
    return {'version':1,'installationID':value['installationID'],'node':config['node'],'poolID':config['poolID'],
            'volumeBytes':value['volumeBytes'],'persistentVolume':persistent_volume(value,path)}


def persistent_volume(value, path):
    value=validate(value);identity=value['installationID'];name='wd-db-'+identity
    return {'apiVersion':'v1','kind':'PersistentVolume','metadata':{'name':name,
        'labels':{'app.kubernetes.io/managed-by':'webdock','webdock.dev/database-installation':identity}},
        'spec':{'capacity':{'storage':str(value['volumeBytes'])},'accessModes':['ReadWriteOnce'],
            'volumeMode':'Filesystem','persistentVolumeReclaimPolicy':'Retain','storageClassName':'',
            'claimRef':{'namespace':name,'name':'postgres-1'},'local':{'path':path},
            'nodeAffinity':{'required':{'nodeSelectorTerms':[{'matchExpressions':[
                {'key':'kubernetes.io/hostname','operator':'In','values':[value['node']]}]}]}}}}


def dispatch(action,value):
    if os.environ.get('SPITZLI_DATABASE_STORAGE')!='1':raise ValueError('Separate PostgreSQL storage service required')
    if action=='database.capability':
        if not isinstance(value,list) or any(not isinstance(n,str) for n in value):raise ValueError('Invalid database nodes')
        config=storage.configuration()
        if config['node'] not in value:raise ValueError('Database pool node missing')
        records=storage.records(config)
        ready=all(record['state']=='purged' or (record['state']=='ready' and storage.mounted(directory)) for directory,record in records)
        for directory,record in records:
            if record['state']=='ready':
                storage.verify_image(directory,record)
                data=directory/'mount'/'data';info=data.stat()
                if data.is_symlink() or info.st_uid!=26 or info.st_gid!=26:ready=False
        free=os.statvfs(config['root'])
        return {k:config[k] for k in ['node','poolID','capacityBytes']}|{'ready':ready,'availableBytes':max(0,free.f_bavail*free.f_frsize-config['reserveBytes']), 'allocatedBytes':sum(r['bytes'] for _,r in records if r['state']!='purged'), 'allocations':[{'installationID':r['appID'],'volumeBytes':r['bytes']} for _,r in records if r['state']!='purged']}
    value=validate(value)
    if action=='database.ensure':return ensure(value)
    config=storage.configuration()
    if any(config[k]!=value[k] for k in ['clusterID','poolID','node']):raise ValueError('Database pool identity mismatch')
    packet={'appID':value['installationID'],'projectID':value['installationID'],'clusterID':value['clusterID'],'revision':value['revision'],'spec':{'volumeBytes':value['volumeBytes']}}
    if action=='database.state':
        record=storage.read_record(Path(config['root'])/value['installationID']);storage.identity(packet,record)
        return {'state':record['state'],'volumeBytes':record['bytes']}
    if action=='database.retained':return storage.retained(packet)
    if action=='database.purge':return storage.purge(packet)
    raise ValueError('Unsupported database storage action')
