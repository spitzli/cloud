"""Explicit local-only filesystem acceptance probe, run as root in a disposable node."""
import errno
import json
import os
from pathlib import Path
import sqlite3
import sys
import tempfile
import uuid
from unittest.mock import patch
import storage
from storage import ensure, purge, configuration, capability, read_record, run, StorageError, initialize


def main():
    if os.geteuid()!=0 or os.environ.get('SPITZLI_DISPOSABLE_STORAGE_TEST')!='yes':
        raise SystemExit('Requires explicit disposable root test environment')
    base=Path(tempfile.mkdtemp(prefix='webdock-storage-acceptance-'));base.chmod(0o700)
    root=base/'volumes';root.mkdir(mode=0o700)
    config=base/'config.json';config.write_text(json.dumps({'root':str(root),'poolID':str(uuid.uuid4()),'node':'test-node','clusterID':'789','capacityBytes':268435456,'reserveBytes':67108864}));config.chmod(0o600)
    os.environ['SPITZLI_STORAGE_CONFIG']=str(config)
    initialize()
    packet={'appID':'123','projectID':'456','clusterID':'789','revision':1,'spec':{'volumeBytes':134217728}}
    _,path=ensure(packet);data=Path(path)
    db=sqlite3.connect(data/'fixture.sqlite');db.execute('create table proof(value text)');db.execute("insert into proof values('persistent-fixture')");db.commit()
    restored=sqlite3.connect(data/'restored.sqlite');db.backup(restored);restored.close();db.close()
    with open(data/'capacity-test','wb',buffering=0) as stream:
        written=0
        try:
            while written<=packet['spec']['volumeBytes']:
                written+=stream.write(b'x'*1048576)
        except OSError as error:
            assert error.errno==errno.ENOSPC,error
        else:raise AssertionError('Filesystem exceeded its requested capacity')
    assert 0<written<packet['spec']['volumeBytes']
    (data/'capacity-test').unlink()
    # Restoring a cold mount preserves the original SQLite files and their contents.
    run('umount',str(root/'123'/'mount'))
    assert not data.exists()
    assert capability(['test-node']) is None
    _,newpath=ensure({**packet,'revision':2});assert newpath==path
    for name in ['fixture.sqlite','restored.sqlite']:
        db=sqlite3.connect(data/name)
        try:assert db.execute('select value from proof').fetchone()[0]=='persistent-fixture'
        finally:db.close()
    assert capability(['test-node'])==268435456
    for invalid in [{**packet,'projectID':'999'},{**packet,'spec':{'volumeBytes':67108864}}]:
        try:ensure(invalid)
        except StorageError:pass
        else:raise AssertionError('Ownership/size change accepted')
    run('umount',str(root/'123'/'mount'))
    image=root/'123'/'volume.ext4';image.rename(image.with_suffix('.missing'))
    try:ensure({**packet,'revision':2})
    except (StorageError,OSError):pass
    else:raise AssertionError('Lost image silently initialized')
    assert not image.exists()
    image.with_suffix('.missing').rename(image)
    directory=root/'123';directory.rename(root/'missing-directory')
    try:ensure(packet)
    except (StorageError,OSError):pass
    else:raise AssertionError('Revision-one retry created replacement storage')
    assert not directory.exists()
    try:purge(packet)
    except (StorageError,OSError):pass
    else:raise AssertionError('Missing directory falsely released allocation')
    (root/'missing-directory').rename(directory)
    ensure({**packet,'revision':2})
    write_volume=storage.write_volume
    def interrupted(directory,record):
        if record['state']=='purged':raise OSError('Simulated crash after unlink')
        return write_volume(directory,record)
    with patch('storage.write_volume',side_effect=interrupted):
        try:purge(packet)
        except OSError:pass
        else:raise AssertionError('Crash was not simulated')
    assert not image.exists() and read_record(root/'123')['state']=='purging'
    purge(packet);purge(packet)
    assert not image.exists() and read_record(root/'123')['state']=='purged'
    print(json.dumps({'testRoot':str(base),'hardLimitBytes':134217728,'writtenBeforeENOSPC':written,'remountPersistence':True,'sqliteBackupRestore':True,'missingImageRejected':True,'ownershipRejected':True,'purgeIdempotent':True}))


if __name__=='__main__':main()
