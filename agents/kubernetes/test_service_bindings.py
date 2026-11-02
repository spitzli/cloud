import unittest
from pathlib import Path
from test_executor import ExecutorTests
from service_bindings import manifests, validate

class ServiceBindingTests(unittest.TestCase):
    def packet(self):
        p=ExecutorTests().packet()
        p['serviceBindingsVersion']=1
        p['serviceBindings']=[{'id':'111','revision':2,'backendID':'222','caCertificate':Path(__file__).parents[2].joinpath('legacy/control-plane/tests/ported-auth/fixtures/database-ca.crt').read_text()}]
        return p
    def test_binding_grants_only_selected_app_database_and_mounts_ca(self):
        p=self.packet();validate(p)
        secret,policy=manifests(p)
        self.assertEqual(policy['metadata']['namespace'],'wd-456')
        self.assertEqual(policy['spec']['podSelector'],{'matchLabels':{'webdock.dev/app-id':'123'}})
        self.assertEqual(policy['spec']['egress'][0]['to'][0]['namespaceSelector']['matchLabels'],{'kubernetes.io/metadata.name':'wd-db-222'})
        self.assertEqual(policy['spec']['egress'][0]['ports'],[{'protocol':'TCP','port':5432}])
        self.assertIn('binding-111.crt',secret['data'])
        self.assertTrue(secret['immutable'])
    def test_disconnect_has_no_remaining_egress_or_certificates(self):
        p=self.packet();p['serviceBindings']=[]
        secret,policy=manifests(p)
        self.assertIsNone(secret)
        self.assertEqual(policy['spec']['egress'],[])
    def test_untrusted_paths_extra_fields_duplicates_and_unsupported_versions_fail(self):
        for change in [{'backendID':'../../kube-system'},{'caCertificate':'invalid'},{'id':'0'},{'revision':0},{'namespace':'kube-system'}]:
            p=self.packet();p['serviceBindings'][0].update(change)
            with self.assertRaises(ValueError):validate(p)
        p=self.packet();p['serviceBindings']*=2
        with self.assertRaises(ValueError):validate(p)
        p=self.packet();p['serviceBindingsVersion']=2
        with self.assertRaises(ValueError):validate(p)

    def test_mail_egress_is_pinned_to_native_api_hosts_and_https(self):
        p=self.packet();p['serviceBindings']=[{'id':'444','revision':1,'kind':'mail','egressCIDRs':['192.0.2.10/32']}]
        secret,policy=manifests(p)
        self.assertIsNone(secret)
        self.assertEqual(policy['spec']['egress'],[{'to':[{'ipBlock':{'cidr':'192.0.2.10/32'}}],'ports':[{'protocol':'TCP','port':443}]}])
        p['serviceBindings'][0]['egressCIDRs']=['0.0.0.0/0']
        with self.assertRaises(ValueError):validate(p)
