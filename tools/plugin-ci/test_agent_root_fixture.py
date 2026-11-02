import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('agent_root_fixture',Path(__file__).with_name('agent_root_fixture.py'))
fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)

class RootFixtureProfile(unittest.TestCase):
    def test_wrong_physical_host_stops_before_config_or_host_writes(self):
        with patch.object(fixture.os,'geteuid',return_value=0),patch.object(Path,'read_text',return_value='different-system'),patch.object(fixture,'owned_directory') as writes:
            with self.assertRaises(ValueError):fixture.run(Path('/nonexistent'), 'a'*64)
            writes.assert_not_called()
    def test_nonroot_cannot_enter_infrastructure_qualification(self):
        with patch.object(fixture.os,'geteuid',return_value=1001),patch.object(fixture,'command') as calls:
            with self.assertRaises(ValueError):fixture.pins()
            calls.assert_not_called()
    def test_profile_has_no_production_context_or_config_override(self):
        self.assertEqual(str(fixture.CONFIG),'/etc/rancher/k3s/k3s.yaml')
        self.assertEqual(fixture.API,'https://10.83.0.1:6443')
        self.assertEqual(fixture.NODE,'webdock-build-eu-01')
        self.assertEqual(str(fixture.BASE),'/opt/spitzli-plugin-ci/legacy-attachment-fixture')
        self.assertEqual(fixture.DMI,'e6d9884c-e6bd-4699-8a0c-99f2358ae8a1')

if __name__=='__main__':unittest.main()
