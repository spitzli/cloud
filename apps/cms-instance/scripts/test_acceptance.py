import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('cms_acceptance', Path(__file__).with_name('acceptance.py'))
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class AcceptanceSafetyTests(unittest.TestCase):
    def test_inherited_live_configuration_and_preloads_are_not_forwarded(self):
        self.assertEqual(module.clean_environment({'PATH':'/bin','HOME':'/home/test','DATABASE_URL':'live','PGPASSWORD':'live','PAYLOAD_SECRET':'live','CMS_MIGRATING':'true','NODE_OPTIONS':'--import=evil','WEBDOCK_SSO_CLIENT_SECRET':'live'}),{'PATH':'/bin','HOME':'/home/test'})

    def test_cleanup_requires_the_exact_random_run_directory(self):
        self.assertEqual(module.fixture_path('/tmp/webdock-cms-acceptance-0123456789abcdef','0123456789abcdef').name,'webdock-cms-acceptance-0123456789abcdef')
        for directory, token in [('/tmp','0123456789abcdef'),('/tmp/webdock-cms-acceptance-old','old'),('/tmp/webdock-cms-acceptance-0123456789abcdef/..','0123456789abcdef')]:
            with self.assertRaises(RuntimeError): module.fixture_path(directory,token)

    def test_cleanup_ownership_uses_exact_labels_not_resource_names(self):
        self.assertTrue(module.owned({'Config':{'Labels':{module.LABEL:'one'}}},'one'))
        self.assertFalse(module.owned({'Name':'webdock-cms-acceptance-one','Config':{'Labels':{module.LABEL:'other'}}},'one'))
        self.assertFalse(module.owned({'Labels':{}},'one'))

    def test_existing_resources_are_rejected_before_creation(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)/'webdock-cms-acceptance-0123456789abcdef';root.mkdir(mode=0o700)
            fixture=module.Fixture(root,'0123456789abcdef');fixture.inspect=lambda *args:{'Id':'existing'}
            with self.assertRaisesRegex(RuntimeError,'Refusing to reuse'): fixture.create('container','existing',[])
            self.assertEqual(fixture.resources,[])

    def test_cleanup_never_invokes_remove_for_a_foreign_label(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)/'webdock-cms-acceptance-0123456789abcdef';root.mkdir(mode=0o700)
            fixture=module.Fixture(root,'0123456789abcdef');fixture.inspect=lambda *args:{'Config':{'Labels':{module.LABEL:'foreign'}}}
            called=[];fixture.run=lambda *args,**kwargs:called.append(args)
            with self.assertRaisesRegex(RuntimeError,'Ownership label mismatch'): fixture.remove({'kind':'container','id':'foreign-id','name':'foreign'})
            self.assertEqual(called,[])

    def test_cleanup_checks_resource_absence_before_reporting_success(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)/'webdock-cms-acceptance-0123456789abcdef';root.mkdir(mode=0o700)
            fixture=module.Fixture(root,'0123456789abcdef',[{'kind':'container','id':'owned','name':'owned'}])
            fixture.remove=lambda resource:None
            fixture.inspect=lambda *args:{'Config':{'Labels':{module.LABEL:'0123456789abcdef'}}}
            with self.assertRaisesRegex(RuntimeError,'remained after cleanup'):fixture.cleanup()

    def test_private_artifacts_have_no_group_or_world_access(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)/'webdock-cms-acceptance-0123456789abcdef';root.mkdir(mode=0o700)
            fixture=module.Fixture(root,'0123456789abcdef');target=fixture.private('runtime.env','DATABASE_URL=test\n')
            self.assertEqual(target.stat().st_mode&0o777,0o600)
            self.assertEqual((root/'fixture.json').stat().st_mode&0o777,0o600)

if __name__=='__main__': unittest.main()
