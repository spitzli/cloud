import unittest
import registry

class RegistryTests(unittest.TestCase):
    def setUp(self):
        self.packet={'projectID':'2','spec':{'image':'registry.example/tenant-1/project-2@sha256:'+'a'*64}}
        self.config={'version':1,'euStorageEvidence':'verified DE','projects':{'2':{'repository':'registry.example/tenant-1/project-2','pullSecret':'registry-project-2','readOnly':True,'credentialScopeEvidence':'read-only repository-scoped robot'}}}
    def test_exact_project_digest(self):
        self.assertEqual(registry.pull_configuration(self.packet,self.config),{'name':'registry-project-2'})
    def test_unverified_geography_is_truthful_metadata_not_weaker_pull_authorization(self):
        self.config['euStorageEvidence']='Precise country/EU placement unverified; best-effort geography accepted.'
        self.assertEqual(registry.pull_configuration(self.packet,self.config),{'name':'registry-project-2'})
        self.packet['projectID']='3'
        with self.assertRaises(registry.RegistryError):registry.pull_configuration(self.packet,self.config)
        self.packet['projectID']='2'
        self.config['projects']['2']['readOnly']=False
        with self.assertRaises(registry.RegistryError):registry.pull_configuration(self.packet,self.config)

    def test_existing_local_images_do_not_acquire_registry_credentials(self):
        self.packet['spec']['image']='docker.io/library/nginx@sha256:'+'b'*64
        self.assertIsNone(registry.pull_configuration(self.packet,self.config))
    def test_other_projects_cannot_use_cached_managed_registry_images(self):
        self.packet['projectID']='3'
        with self.assertRaises(registry.RegistryError):registry.pull_configuration(self.packet,self.config)
    def test_tag_wrong_tenant_and_write_credential_rejected(self):
        for image in ['registry.example/tenant-1/project-2:latest','registry.example/tenant-2/project-2@sha256:'+'a'*64]:
            self.packet['spec']['image']=image
            with self.assertRaises(registry.RegistryError):registry.pull_configuration(self.packet,self.config)
        self.config['projects']['2']['readOnly']=False
        with self.assertRaises(registry.RegistryError):registry.pull_configuration(self.packet,self.config)

if __name__=='__main__':unittest.main()

class CapabilityTests(unittest.TestCase):
    def test_no_config_never_advertises(self):
        from unittest.mock import patch
        with patch.dict('os.environ',{},clear=True):self.assertFalse(registry.capability(['kubectl']))
    def test_missing_secret_never_advertises(self):
        from unittest.mock import patch
        import tempfile,json
        from pathlib import Path
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as d:
            p=Path(d)/'config';p.write_text(json.dumps({'version':1,'euStorageEvidence':'DE','projects':{'2':{'repository':'r.example/c-1/p-2','pullSecret':'pull','readOnly':True,'credentialScopeEvidence':'verified'}}}))
            with patch.dict('os.environ',{'SPITZLI_REGISTRY_CONFIG':str(p)}),patch.object(Path,'lstat',return_value=SimpleNamespace(st_mode=0o100600,st_uid=0)):
                self.assertFalse(registry.capability(['kubectl'],lambda *a,**k:SimpleNamespace(stdout='Opaque')))
                calls=[]
                def ready(command,**kwargs):calls.append(command);return SimpleNamespace(stdout='kubernetes.io/dockerconfigjson')
                self.assertTrue(registry.capability(['kubectl'],ready));self.assertEqual(len(calls),2)
