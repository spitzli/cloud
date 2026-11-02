import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('runtime_qa',Path(__file__).with_name('agent_runtime_qa.py'))
qa=importlib.util.module_from_spec(spec);spec.loader.exec_module(qa)

class FixedRuntimeQualification(unittest.TestCase):
    def test_build_or_other_host_fails_before_any_qa_write(self):
        with patch.object(qa.os,'geteuid',return_value=0),patch.object(Path,'read_text',return_value='e6d9884c-e6bd-4699-8a0c-99f2358ae8a1'),patch.object(qa,'owned_directory') as writes:
            with self.assertRaises(ValueError):qa.run(Path('/nonexistent'), 'a'*64)
            writes.assert_not_called()
    def test_only_reviewed_literal_runtime_profile_is_exposed(self):
        self.assertEqual(qa.DMI,'91961f5a-b06b-44d1-9684-3f07ae5ca5bc')
        self.assertEqual(qa.API,'https://10.0.0.2:6443')
        self.assertEqual(qa.CA_SHA,'12381eae493b6fb156175929e9858d1c74770077d42ea12b115d8bffe0cf596a')
        self.assertEqual(qa.NODE,'wd-eu-02')
        self.assertEqual(str(qa.BASE),'/opt/webdock-agent-qualification/retained-attachment-fixture')
        self.assertEqual(str(qa.CONFIG),'/etc/rancher/k3s/k3s.yaml')

if __name__=='__main__':unittest.main()
