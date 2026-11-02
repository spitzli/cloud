"""Private database DNS/CA delivery for the Core HTTP workload."""
import unittest
from tools.tests import test_agent_control_chart as helpers
BASE=helpers.BASE

class CoreDatabaseChart(unittest.TestCase):
    render=helpers.AgentControlChart.render
    def test_private_database_route_keeps_tls_hostname_and_public_ca_readonly(self):
        values={**BASE,'databaseCAConfigMap':'owned-postgres-ca','databaseHostAliases':[{'ip':'10.0.0.2','hostnames':['db-owned.pg.example']} ]}
        docs=self.render(values)
        pod=next(d for d in docs if d['kind']=='Deployment')['spec']['template']['spec']
        self.assertEqual(pod['hostAliases'],values['databaseHostAliases'])
        self.assertIn({'name':'database-ca','mountPath':'/etc/webdock/database-ca','readOnly':True},pod['containers'][0]['volumeMounts'])
        self.assertIn({'name':'database-ca','configMap':{'name':'owned-postgres-ca','defaultMode':292}},pod['volumes'])
        self.assertFalse(pod['automountServiceAccountToken'])
        self.assertNotIn('hostNetwork',pod)

if __name__=='__main__':unittest.main()
