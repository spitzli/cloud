import importlib.util
from pathlib import Path
import unittest

ROOT=Path(__file__).resolve().parents[2]/"deploy/build-cluster"

class BuildStorageProfile(unittest.TestCase):
    def test_control_channels_separate_request_and_receipt_authority_and_bound_metadata(self):
        spec=importlib.util.spec_from_file_location("storage_profile",ROOT/"render-storage.py")
        module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
        documents=module.render()
        namespaces=[d for d in documents if d["kind"]=="Namespace"]
        self.assertEqual(len(namespaces),3)
        for sa in [d for d in documents if d["kind"]=="ServiceAccount"]:self.assertFalse(sa["automountServiceAccountToken"])
        for role in [d for d in documents if d["kind"] in ["Role","ClusterRole"]]:
            for rule in role["rules"]:
                self.assertNotIn("*",rule["resources"]);self.assertNotIn("*",rule["verbs"])
                self.assertFalse(set(rule["resources"]) & {"secrets","pods/exec","nodes"})
        producer=[d for d in documents if d["kind"]=="Role" and d["metadata"]["name"]=="build-volume-producer"]
        self.assertEqual(len(producer),2)
        replies=next(d for d in producer if "receipts" in d["metadata"]["namespace"])
        self.assertFalse(set(replies["rules"][0]["verbs"]) & {"create","update","patch"})
        for quota in [d for d in documents if d["kind"]=="ResourceQuota"]:
            self.assertLessEqual(int(quota["spec"]["hard"]["configmaps"]),256)
            self.assertEqual(quota["spec"]["hard"]["pods"],"0")
        policies=[d for d in documents if d["kind"]=="ValidatingAdmissionPolicy"]
        self.assertEqual(len(policies),3)
        self.assertTrue(all(p["spec"]["failurePolicy"]=="Fail" for p in policies))

if __name__=="__main__":unittest.main()
