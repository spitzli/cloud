import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('qualified_host',Path(__file__).with_name('qualified_host.py'))
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)

class RunnerSetup(unittest.TestCase):
    def test_service_is_persistent_bounded_unprivileged_and_has_no_credentials(self):
        unit=module.service_unit()
        self.assertIn('User=spitzli-plugin-ci',unit)
        self.assertIn('CPUQuota=200%',unit)
        self.assertIn('MemoryMax=3G',unit)
        self.assertIn('TasksMax=512',unit)
        self.assertIn('NoNewPrivileges=yes',unit)
        self.assertNotIn('--ephemeral',unit)
        self.assertNotIn('TOKEN',unit)
        self.assertNotIn('docker.sock',unit)

    def test_registration_token_is_env_only_and_not_in_arguments(self):
        captured=[]
        def run(args,**kwargs):captured.append((args,kwargs));return module.subprocess.CompletedProcess(args,0,'','')
        payload={'token':'test-registration-token','group':'cloud-plugin-trusted-eu2'}
        with patch.object(module,'run',side_effect=run),patch.object(module,'RUNNER',Path('/tmp/runner-unit-fixture')):
            module.register(payload,write_service=False)
        args,kwargs=captured[0]
        self.assertNotIn(payload['token'],' '.join(args))
        self.assertEqual(kwargs['env']['ACTIONS_RUNNER_INPUT_TOKEN'],payload['token'])
        self.assertIn('--disableupdate',args)
        self.assertNotIn('--ephemeral',args)
        with self.assertRaises(ValueError):module.register({'token':'','group':'Default'},write_service=False)

    def test_archive_path_escape_rejected(self):
        with tempfile.TemporaryDirectory() as td:
            root=Path(td)
            for path in ['/etc/passwd','../escape','safe/../../escape']:
                with self.assertRaises(ValueError):module.safe_member(root,path)
            self.assertEqual(module.safe_member(root,'safe/file'),root/'safe/file')

if __name__=='__main__':unittest.main()
