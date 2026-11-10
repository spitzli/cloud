"""Explicit disposable PostgreSQL-volume ENOSPC and lost-storage acceptance check."""
import errno
import json
import os
from pathlib import Path
import socket
import sys
from datetime import datetime,timedelta,timezone
if os.environ.get('SPITZLI_DISPOSABLE_POSTGRES_TEST')!='yes' or socket.gethostname()!='postgres-pilot':raise SystemExit('Disposable test node required')
sys.path.insert(0,'/tmp/webdock-e2e-agent')
import storage,database_storage
root=Path('/var/lib/webdock-db-pilot');volume=root/'125';data=volume/'mount/data'
config={'root':str(root),'node':'postgres-pilot','clusterID':'456','poolID':'bd0ba8e4-e801-4c16-bf1b-060129437ccb','capacityBytes':4294967296,'reserveBytes':1073741824}
path=Path('/tmp/bounded-postgres-config.json');path.write_text(json.dumps(config));path.chmod(0o600);os.environ['SPITZLI_STORAGE_CONFIG']=str(path)
# The caller must hibernate this exact disposable Cluster before unmounting.
import subprocess
pods=json.loads(subprocess.check_output(['kubectl','get','pods','-n','wd-db-125','-o','json']))['items']
assert not any(p.get('status',{}).get('phase') not in ['Succeeded','Failed'] for p in pods),'Stop the disposable database first'
record=storage.read_record(volume);assert record['bytes']==1073741824
marker=data/'bounded-storage-sentinel';marker.write_text('postgres-volume-retained')
probe=data/'capacity-probe';written=0
try:
 with probe.open('wb',buffering=0) as output:
  try:
   while written<=record['bytes']:written+=output.write(b'x'*1048576)
  except OSError as error:assert error.errno==errno.ENOSPC
  else:raise AssertionError('Bounded filesystem did not return ENOSPC')
finally:probe.unlink(missing_ok=True)
assert 0<written<record['bytes']
storage.run('umount',str(volume/'mount'));assert not data.exists()
packet={'version':1,'installationID':'125','clusterID':'456','revision':2,'node':'postgres-pilot','poolID':config['poolID'],'volumeBytes':record['bytes'],'leaseUntil':(datetime.now(timezone.utc)+timedelta(minutes=4)).isoformat()}
image=volume/'volume.ext4';missing=volume/'volume.missing';image.rename(missing)
try:
 try:database_storage.ensure(packet)
 except (OSError,ValueError,storage.StorageError):pass
 else:raise AssertionError('Lost image was replaced')
 assert not image.exists()
finally:missing.rename(image)
journal=root/'_pool/state.json';hidden=root/'_pool/state.testing';journal.rename(hidden)
try:
 try:database_storage.ensure(packet)
 except (OSError,ValueError,storage.StorageError):pass
 else:raise AssertionError('Lost journal was ignored')
finally:hidden.rename(journal)
proof=database_storage.ensure(packet)
assert marker.read_text()=='postgres-volume-retained'
assert storage.read_record(volume)['uuid']==record['uuid']
assert proof['volumeBytes']==record['bytes']
print(json.dumps({'filesystemBytes':record['bytes'],'writtenBeforeENOSPC':written,'missingMountDetected':True,'missingImageRejected':True,'missingJournalRejected':True,'filesystemIdentityPreserved':True,'retainedDataPreserved':True}))
