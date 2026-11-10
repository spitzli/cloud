"""Root-only, fixed-size local filesystems. No tenant paths or shell commands."""
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import uuid


class StorageError(Exception):
    pass


def run(*args, timeout=45):
    result=subprocess.run(args,capture_output=True,text=True,timeout=timeout)
    if result.returncode:raise StorageError('Storage operation failed')
    return result.stdout.strip()


def private(path, directory=False):
    path=Path(path)
    if path.resolve()!=path or path.is_symlink():raise StorageError('Unsafe storage path')
    info=path.stat()
    if info.st_uid!=0 or info.st_mode & 0o077 or not (stat.S_ISDIR(info.st_mode) if directory else stat.S_ISREG(info.st_mode)):
        raise StorageError('Storage configuration must be root-only')
    return path


def configuration(validate_pool=True):
    path=private(Path(os.environ.get('SPITZLI_STORAGE_CONFIG','/etc/webdock-storage.json')))
    value=json.loads(path.read_text())
    if set(value)!={'root','node','clusterID','poolID','capacityBytes','reserveBytes'}:raise StorageError('Invalid storage configuration')
    private(Path(value['root']),directory=True)
    if not re.fullmatch(r'[a-z0-9][a-z0-9.-]{0,252}',value['node']) or not re.fullmatch(r'[1-9][0-9]{0,18}',value['clusterID']):raise StorageError('Invalid storage owner')
    for key in ['capacityBytes','reserveBytes']:
        if type(value[key]) is not int or not 67108864<=value[key]<=1099511627776:raise StorageError('Invalid storage capacity')
    if str(uuid.UUID(value['poolID']))!=value['poolID']:raise StorageError('Invalid storage pool identity')
    if validate_pool:pool(value)
    return value


def sync_directory(directory):
    fd=os.open(directory,os.O_DIRECTORY|os.O_NOFOLLOW)
    try:os.fsync(fd)
    finally:os.close(fd)


def write_record(directory, record):
    temporary=directory/'state.new'
    fd=os.open(temporary,os.O_WRONLY|os.O_CREAT|os.O_TRUNC|os.O_NOFOLLOW,0o600)
    with os.fdopen(fd,'w') as stream:
        json.dump(record,stream);stream.flush();os.fsync(stream.fileno())
    os.replace(temporary,directory/'state.json')
    sync_directory(directory)


def pool(config):
    path=private(Path(config['root'])/'_pool'/'state.json')
    value=json.loads(path.read_text())
    if set(value)!={'poolID','volumes'} or value['poolID']!=config['poolID'] or not isinstance(value['volumes'],dict):raise StorageError('Storage pool identity changed')
    return value


def initialize():
    config=configuration(False);root=Path(config['root'])
    if list(root.iterdir()):raise StorageError('Initialize requires a new empty storage directory')
    (root/'_pool').mkdir(mode=0o700)
    write_record(root/'_pool',{'poolID':config['poolID'],'volumes':{}})
    sync_directory(root)


def write_volume(directory, record):
    config=configuration();value=pool(config)
    value['volumes'][directory.name]=record
    write_record(directory.parent/'_pool',value)


def read_record(directory):
    value=pool(configuration())['volumes'].get(directory.name)
    if not isinstance(value,dict) or set(value)!={'appID','projectID','clusterID','bytes','uuid','state'} or value['appID']!=directory.name:
        raise StorageError('Invalid volume record')
    for key in ['appID','projectID','clusterID']:
        if not isinstance(value[key],str) or not re.fullmatch(r'[1-9][0-9]{0,18}',value[key]):raise StorageError('Invalid volume owner')
    if type(value['bytes']) is not int or not 67108864<=value['bytes']<=1099511627776 or value['state'] not in ['initializing','ready','purging','purged']:
        raise StorageError('Invalid volume record')
    if str(uuid.UUID(value['uuid']))!=value['uuid']:raise StorageError('Invalid filesystem identity')
    return value


def records(config):
    result=[];root=Path(config['root']);value=pool(config)
    if any(p.name!='_pool' and p.name not in value['volumes'] for p in root.iterdir()):raise StorageError('Unknown storage entry')
    for name in value['volumes']:
        directory=root/name;record=read_record(directory)
        if record['clusterID']!=config['clusterID']:raise StorageError('Storage cluster changed')
        result.append((directory,record))
    return result


def identity(packet, record):
    if any(packet[k]!=record[k] for k in ['appID','projectID','clusterID']) or packet['spec']['volumeBytes']!=record['bytes']:
        raise StorageError('Storage ownership changed')


def mounted(directory):
    target=directory/'mount'
    if target.is_symlink():raise StorageError('Unsafe mount target')
    result=subprocess.run(['findmnt','--json','--mountpoint',str(target),'-o','SOURCE,TARGET,FSTYPE'],capture_output=True,text=True,timeout=10)
    if result.returncode==1:return False
    if result.returncode:raise StorageError('Cannot inspect mount')
    mounts=json.loads(result.stdout)['filesystems']
    if len(mounts)!=1 or mounts[0]['fstype']!='ext4':raise StorageError('Unexpected storage mount')
    source=mounts[0]['source']
    if not re.fullmatch(r'/dev/loop[0-9]+',source):raise StorageError('Unexpected storage device')
    loops=json.loads(run('losetup','--json','--list','--output','NAME,BACK-FILE',source))['loopdevices']
    if len(loops)!=1 or loops[0]['back-file']!=str(directory/'volume.ext4'):raise StorageError('Wrong backing image')
    return True


def verify_image(directory, record):
    image=private(directory/'volume.ext4')
    if image.stat().st_size!=record['bytes'] or image.stat().st_nlink!=1:raise StorageError('Storage image changed')
    if run('blkid','-p','-s','UUID','-o','value',str(image))!=record['uuid']:raise StorageError('Filesystem identity changed')
    return image


def mount_record(directory, record):
    if record['state']!='ready':raise StorageError('Volume needs operator reconciliation')
    image=verify_image(directory,record)
    if not mounted(directory):
        target=directory/'mount'
        if target.exists() and (target.is_symlink() or list(target.iterdir())):raise StorageError('Mount target is not empty')
        target.mkdir(mode=0o700,exist_ok=True)
        run('mount','-t','ext4','-o','loop,nodev,nosuid,noexec',str(image),str(target))
    if not mounted(directory):raise StorageError('Volume is not mounted')
    # This path does not exist on the underlying filesystem when unmounted.
    data=directory/'mount'/'data'
    if not data.is_dir() or data.is_symlink():raise StorageError('Volume data directory missing')
    return str(data)


def ensure(packet):
    config=configuration()
    if packet['clusterID']!=config['clusterID']:raise StorageError('Wrong storage cluster')
    directory=Path(config['root'])/packet['appID']
    if packet['appID'] in pool(config)['volumes']:
        record=read_record(directory);identity(packet,record)
        private(directory,directory=True)
        return config,mount_record(directory,record)
    if packet['revision']!=1:raise StorageError('Missing volume; refusing empty replacement')
    size=packet['spec']['volumeBytes']
    if type(size) is not int or not 67108864<=size<=1099511627776:raise StorageError('Invalid storage size')
    used=sum(r['bytes'] for _,r in records(config) if r['state']!='purged')
    available=os.statvfs(config['root'])
    if used+size>config['capacityBytes'] or available.f_bavail*available.f_frsize<size+config['reserveBytes']:
        raise StorageError('Storage capacity unavailable')
    record={k:packet[k] for k in ['appID','projectID','clusterID']}
    record.update(bytes=size,uuid=str(uuid.uuid4()),state='initializing');write_volume(directory,record)
    directory.mkdir(mode=0o700)
    image=directory/'volume.ext4'
    fd=os.open(image,os.O_RDWR|os.O_CREAT|os.O_EXCL|os.O_NOFOLLOW,0o600)
    try:os.posix_fallocate(fd,0,size);os.fsync(fd)
    finally:os.close(fd)
    run('mkfs.ext4','-q','-m','0','-U',record['uuid'],'-E','nodiscard,lazy_itable_init=0,lazy_journal_init=0',str(image))
    target=directory/'mount';target.mkdir(mode=0o700)
    run('mount','-t','ext4','-o','loop,nodev,nosuid,noexec',str(image),str(target))
    if not mounted(directory):raise StorageError('Volume mount failed')
    data=target/'data';data.mkdir(mode=0o770);os.chown(data,65532,65532)
    run('sync','-f',str(target))
    sync_directory(directory);sync_directory(directory.parent)
    record['state']='ready';write_volume(directory,record)
    return config,mount_record(directory,record)


def retained(packet):
    config=configuration()
    if packet['clusterID']!=config['clusterID']:raise StorageError('Wrong storage cluster')
    directory=Path(config['root'])/packet['appID']
    # An interrupted first allocation may have no filesystem; do not create one on delete.
    if packet['appID'] in pool(config)['volumes']:identity(packet,read_record(directory))


def state(packet):
    """Inspect existing storage without mounting, repairing, or allocating it."""
    config=configuration()
    if packet['clusterID']!=config['clusterID']:raise StorageError('Wrong storage cluster')
    directory=Path(config['root'])/packet['appID']
    record=read_record(directory);identity(packet,record)
    private(directory,directory=True)
    if record['state']=='ready':
        verify_image(directory,record)
        if not mounted(directory):raise StorageError('Volume is not mounted')
        data=os.lstat(directory/'mount'/'data')
        if not stat.S_ISDIR(data.st_mode) or data.st_uid!=65532 or data.st_gid!=65532:
            raise StorageError('Volume data ownership changed')
    elif record['state']=='purged':
        if any(directory.iterdir()):raise StorageError('Purged volume contains data')
    else:raise StorageError('Volume needs operator reconciliation')
    return record['state']


def purge(packet):
    config=configuration()
    if packet['clusterID']!=config['clusterID']:raise StorageError('Wrong storage cluster')
    directory=Path(config['root'])/packet['appID']
    if packet['appID'] not in pool(config)['volumes']:return
    record=read_record(directory);identity(packet,record)
    if record['state']=='purged':return
    records(config)
    private(directory,directory=True)
    if any(child.name not in ['volume.ext4','mount'] for child in directory.iterdir()):raise StorageError('Unknown volume files')
    image=directory/'volume.ext4'
    if record['state']=='ready':verify_image(directory,record)
    if image.is_symlink():raise StorageError('Unsafe image path')
    if record['state']!='purging':
        record['state']='purging';write_volume(directory,record)
    if mounted(directory):run('umount',str(directory/'mount'))
    if image.exists():
        private(image)
        loops=json.loads(run('losetup','--json','--list','--associated',str(image)))['loopdevices']
        if loops:raise StorageError('Volume is still attached')
        image.unlink()
    target=directory/'mount'
    if target.exists():target.rmdir()
    sync_directory(directory)
    record['state']='purged';write_volume(directory,record)


def restore():
    config=configuration()
    for directory,record in records(config):
        if record['state']=='ready':
            try:mount_record(directory,record)
            except (OSError,ValueError,StorageError,subprocess.SubprocessError):pass


def capability(node_names):
    try:
        config=configuration()
        if node_names!=[config['node']]:return None
        for directory,record in records(config):
            if record['state'] not in ['ready','purged']:return None
            if record['state']=='ready':
                verify_image(directory,record)
                if not mounted(directory):return None
        return config['capacityBytes']
    except (OSError,ValueError,KeyError,StorageError,subprocess.SubprocessError):return None


if __name__=='__main__':
    import sys
    if len(sys.argv)==2 and sys.argv[1]=='initialize' and os.geteuid()==0:initialize()
    else:raise SystemExit('Usage: storage.py initialize (root, new empty pool only)')
