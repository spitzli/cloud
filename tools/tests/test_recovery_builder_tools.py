"""Finite builder custody receipts and the measured public chroot symlink layout; no chroot run."""
import ast,importlib.util,json,os,pathlib,shutil,tempfile,unittest
P=pathlib.Path
spec=importlib.util.spec_from_file_location('builder_rootfs',P(__file__).with_name('recovery_gateway_rootfs.py'));r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
class BuilderToolTests(unittest.TestCase):
 def test_actual_old_parent_predicate_rejects_measured_chroot_layout(self):
  with tempfile.TemporaryDirectory() as tmp:
   base=P(tmp);(base/'usr/bin').mkdir(parents=True);(base/'usr/lib/cargo/bin/coreutils').mkdir(parents=True)
   target=base/'usr/lib/cargo/bin/coreutils/chroot';shutil.copyfile(shutil.which('chroot'),target);target.chmod(0o555);(base/'usr/bin/chroot').symlink_to('../lib/cargo/bin/coreutils/chroot')
   canonical=P('/')/(base/'usr/bin/chroot').resolve().relative_to(base)
   self.assertEqual(canonical,P('/usr/lib/cargo/bin/coreutils/chroot'))
   # The actual local file owner is retained honestly; this is path-shape proof, not BASE UID proof.
   receipt=r.builder_tool_receipt('chroot',P('/usr/bin/chroot'),canonical,target.stat())
   self.assertTrue(receipt['criteria']['canonical']);self.assertEqual(receipt['uid'],os.getuid());self.assertFalse(receipt['criteria']['ownerRoot'])
 def test_unsafe_canonical_owner_and_mode_are_explicit_without_echoing_private_paths(self):
  root_meta=P(shutil.which('chroot')).stat()
  allowed=r.builder_tool_receipt('chroot',P('/usr/bin/chroot'),P('/usr/lib/cargo/bin/coreutils/chroot'),root_meta);self.assertTrue(allowed['passed'])
  poison=r.builder_tool_receipt('chroot',P('/root/poison-secret'),P('/root/poison-secret'),None);self.assertFalse(poison['passed']);self.assertNotIn('poison',json.dumps(poison))
  self.assertFalse(r.builder_tool_receipt('chroot',P('/usr/bin/chroot'),P('/usr/lib/cargo/bin/coreutils/other'),root_meta)['passed'])
  missing=r.builder_tool_receipt('chroot',None,None,None);self.assertFalse(missing['passed']);self.assertIsNone(missing['uid'])
if __name__=='__main__':unittest.main()
