import importlib.util
from pathlib import Path
from types import SimpleNamespace
import unittest

ROOT=Path(__file__).resolve().parents[2]/'deploy/build-cluster'
spec=importlib.util.spec_from_file_location('bounded_logs',ROOT/'bounded-logs.py')
logs=importlib.util.module_from_spec(spec);spec.loader.exec_module(logs)

class BuildLogStore(unittest.TestCase):
    def test_mount_identity_rejects_replaced_backing_unbounded_target_or_weakened_flags(self):
        record={'image':'/owned/log.ext4','target':'/owned/logs','uuid':'owned-fs'}
        image=SimpleNamespace(st_ino=123,st_dev=0)
        mount={'source':'/dev/loop0','target':'/owned/logs','fstype':'ext4','uuid':'owned-fs','options':'rw,nodev,nosuid,noexec'}
        loop={'back-file':'/owned/log.ext4','back-ino':123,'back-maj:min':'0:0'}
        logs.validate_mount(record,image,mount,loop)
        for key,value in [('uuid','replacement'),('target','/var/log'),('options','rw'),('fstype','overlay')]:
            changed={**mount,key:value}
            with self.assertRaises(RuntimeError):logs.validate_mount(record,image,changed,loop)
        with self.assertRaises(RuntimeError):logs.validate_mount(record,image,mount,{**loop,'back-ino':124})

if __name__=='__main__':unittest.main()
