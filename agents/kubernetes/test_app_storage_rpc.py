"""Read-only native-agent compatibility against the retained Python pool format."""
from datetime import datetime, timedelta, timezone
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import storage
from storage_service import dispatch


class AppStorageReadRPCTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name)
        self.root.chmod(0o700)
        self.pool=self.root/'volumes';self.pool.mkdir(mode=0o700)
        self.config={'root':str(self.pool),'node':'storage-node','clusterID':'789','poolID':'9cd1dd12-6d6f-4478-96ae-d2cae036269a','capacityBytes':268435456,'reserveBytes':67108864}
        path=self.root/'config.json';path.write_text(json.dumps(self.config));path.chmod(0o600)
        self.environment=patch.dict(os.environ,{'SPITZLI_STORAGE_CONFIG':str(path)})
        self.environment.start();self.addCleanup(self.environment.stop)
        # Only filesystem ownership is substituted for unprivileged test runners.
        self.private=patch('storage.private',side_effect=lambda p,directory=False:Path(p))
        self.private.start();self.addCleanup(self.private.stop)
        (self.pool/'_pool').mkdir(mode=0o700)
        self.record={'appID':'123','projectID':'456','clusterID':'789','bytes':134217728,'uuid':'fd679f86-f8c5-4f94-aacb-17154ab2b5d8','state':'ready'}
        self.directory=self.pool/'123';self.directory.mkdir(mode=0o700)
        self.save()
        self.packet={'appID':'123','projectID':'456','clusterID':'789','revision':2,'leaseUntil':(datetime.now(timezone.utc)+timedelta(minutes=2)).isoformat(),'spec':{'volumeBytes':134217728}}

    def save(self):
        storage.write_record(self.pool/'_pool',{'poolID':self.config['poolID'],'volumes':{'123':self.record}})

    def test_info_returns_existing_configuration_and_requires_null(self):
        before=(self.pool/'_pool'/'state.json').read_bytes()
        self.assertEqual(dispatch({'action':'info','value':None}),self.config)
        self.assertEqual((self.pool/'_pool'/'state.json').read_bytes(),before)
        for invalid in [{},[],self.packet]:
            with self.assertRaises(ValueError):dispatch({'action':'info','value':invalid})

    def test_state_returns_verified_ready_without_mounting_or_writing(self):
        image=self.directory/'volume.ext4';image.write_bytes(b'image')
        target=self.directory/'mount';target.mkdir();data=target/'data';data.mkdir()
        before=(self.pool/'_pool'/'state.json').read_bytes()
        with patch('storage.verify_image',return_value=image),patch('storage.mounted',return_value=True),patch('storage.os.lstat',return_value=type('Info',(),{'st_mode':0o40770,'st_uid':65532,'st_gid':65532})()),patch('storage.mount_record',side_effect=AssertionError('must not mount')):
            self.assertEqual(dispatch({'action':'state','value':self.packet}),'ready')
        self.assertEqual((self.pool/'_pool'/'state.json').read_bytes(),before)

    def test_info_refuses_changed_pool_identity_without_reinitializing(self):
        storage.write_record(self.pool/'_pool',{'poolID':'fd679f86-f8c5-4f94-aacb-17154ab2b5d8','volumes':{}})
        before=(self.pool/'_pool'/'state.json').read_bytes()
        with self.assertRaises(storage.StorageError):dispatch({'action':'info','value':None})
        self.assertEqual((self.pool/'_pool'/'state.json').read_bytes(),before)

    def test_state_checks_actual_image_length_links_and_filesystem_uuid(self):
        image=self.directory/'volume.ext4';image.write_bytes(b'wrong-size')
        with self.assertRaises(storage.StorageError):dispatch({'action':'state','value':self.packet})
        with image.open('wb') as stream:stream.truncate(134217728)
        with patch('storage.run',return_value='fd679f86-f8c5-4f94-aacb-17154ab2b5d9'):
            with self.assertRaises(storage.StorageError):dispatch({'action':'state','value':self.packet})
        os.link(image,self.directory/'extra-link')
        with self.assertRaises(storage.StorageError):dispatch({'action':'state','value':self.packet})

    def test_state_rejects_expired_foreign_missing_and_resized_volumes(self):
        cases=[{**self.packet,'leaseUntil':'2000-01-01T00:00:00Z'}, {**self.packet,'appID':'124'}, {**self.packet,'projectID':'999'}, {**self.packet,'clusterID':'999'}, {**self.packet,'spec':{'volumeBytes':67108864}}, {**self.packet,'spec':{'volumeBytes':True}}]
        for value in cases:
            with self.subTest(value=value),self.assertRaises((ValueError,storage.StorageError)):
                dispatch({'action':'state','value':value})

    def test_state_rejects_unmounted_corrupt_foreign_data_and_incomplete_allocations(self):
        for state in ['initializing','purging']:
            self.record['state']=state;self.save()
            with self.assertRaises(storage.StorageError):dispatch({'action':'state','value':self.packet})
        self.record['state']='ready';self.save()
        with patch('storage.verify_image',side_effect=storage.StorageError('wrong uuid')):
            with self.assertRaises(storage.StorageError):dispatch({'action':'state','value':self.packet})
        with patch('storage.verify_image'),patch('storage.mounted',return_value=False):
            with self.assertRaises(storage.StorageError):dispatch({'action':'state','value':self.packet})
        for mode,uid,gid in [(0o40770,0,65532),(0o40770,65532,0),(0o120777,65532,65532)]:
            with patch('storage.verify_image'),patch('storage.mounted',return_value=True),patch('storage.os.lstat',return_value=type('Info',(),{'st_mode':mode,'st_uid':uid,'st_gid':gid})()):
                with self.assertRaises(storage.StorageError):dispatch({'action':'state','value':self.packet})

    def test_purged_state_requires_empty_existing_directory(self):
        self.record['state']='purged';self.save()
        self.assertEqual(dispatch({'action':'state','value':self.packet}),'purged')
        (self.directory/'unexpected').write_bytes(b'data')
        with self.assertRaises(storage.StorageError):dispatch({'action':'state','value':self.packet})

    def test_capability_rejects_partial_or_foreign_records_and_never_aggregates_nodes(self):
        for state in ['initializing','purging']:
            self.record['state']=state;self.save()
            self.assertEqual(dispatch({'action':'capability','value':['storage-node']}),{'capacityBytes':0,'ready':False})
        self.record['state']='ready';self.save()
        with patch('storage.verify_image'),patch('storage.mounted',return_value=True):
            self.assertEqual(dispatch({'action':'capability','value':['storage-node']}),{'capacityBytes':268435456,'ready':True})
            self.assertEqual(dispatch({'action':'capability','value':['storage-node','other']}),{'capacityBytes':0,'ready':False})
        self.record['clusterID']='999';self.save()
        self.assertEqual(dispatch({'action':'capability','value':['storage-node']}),{'capacityBytes':0,'ready':False})


if __name__=='__main__':unittest.main()
