"""Pure safety contracts; no existing infrastructure is touched."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
spec=importlib.util.spec_from_file_location('qualification',Path(__file__).with_name('qualification.py'))
module=importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
class Ownership(unittest.TestCase):
 def test_cleanup_requires_exact_owned_cluster_label(self):
  self.assertFalse(module.owned_cluster({'Name':'/foreign','Config':{'Labels':{}}},'wd-pg-backup-a1','a1'))
  self.assertFalse(module.owned_cluster({'Name':'/wd-pg-backup-a1','Config':{'Labels':{'webdock.backup.fixture':'foreign'}}},'wd-pg-backup-a1','a1'))
  self.assertTrue(module.owned_cluster({'Name':'/wd-pg-backup-a1','Config':{'Labels':{'webdock.backup.fixture':'a1'}}},'wd-pg-backup-a1','a1'))
 def test_recovery_is_exact_base_and_numeric_timeline(self):
  r=module.recovery_target('20261025T010203','2026-10-25T02:00:00Z',1)
  self.assertEqual(r,{'backupID':'20261025T010203','targetTime':'2026-10-25T02:00:00Z','targetTLI':'1'})
  for base,timeline in [('latest',1),('20261025T010203','latest'),('../foreign',1)]:
   with self.assertRaises(ValueError):module.recovery_target(base,'2026-10-25T02:00:00Z',timeline)
if __name__=='__main__':unittest.main()

class Admission(unittest.TestCase):
 def test_backup_boundary_requires_two_distinct_pins(self):
  import importlib.util
  spec=importlib.util.spec_from_file_location('admission',Path(__file__).with_name('qualification_admission.py'));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
  pin='fixture@sha256:'+'a'*64
  with self.assertRaises(ValueError):m.documents(pin,pin)
  with self.assertRaises(ValueError):m.documents('fixture:latest','sidecar@sha256:'+'b'*64)
  objects=m.documents(pin,'sidecar@sha256:'+'b'*64)
  self.assertEqual(objects[0]['metadata']['name'],'webdock-database-backup-pods')
