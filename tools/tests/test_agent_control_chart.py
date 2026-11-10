"""Offline optional Core agent-listener chart contract; never contacts a cluster."""
import copy,json,shutil,subprocess,tempfile,unittest
from pathlib import Path
import yaml
ROOT=Path(__file__).resolve().parents[2]
CHART=ROOT/'deploy/helm/webdock'
BASE={'image':'registry.example/webdock/backend@sha256:'+'a'*64,'existingSecret':'owned-http-env','apiHost':'api.example'}
def enabled():
    value=copy.deepcopy(BASE)
    value['agentControl']={'enabled':True,'existingEnvSecret':'owned-agent-env','existingPrivatePVC':'owned-agent-private','privateMTLSReviewed':True,'networkAdmissionExistingPolicy':'owned-agent-admission','privateSourceIPv4':['10.0.0.1','192.168.1.2']}
    return value
@unittest.skipUnless(shutil.which('helm'),'helm required')
class AgentControlChart(unittest.TestCase):
    def render(self,values,success=True):
        with tempfile.TemporaryDirectory() as root:
            path=Path(root)/'values.json';path.write_text(json.dumps(values))
            result=subprocess.run(['helm','template','owned',str(CHART),'--namespace','owned-core','-f',str(path)],capture_output=True,text=True)
        self.assertEqual(result.returncode==0,success,result.stderr)
        return list(yaml.safe_load_all(result.stdout)) if success else result.stderr
    def test_default_disabled_output_is_unchanged(self):
        docs=self.render(BASE)
        self.assertEqual({(d['kind'],d['metadata']['name']) for d in docs},{('Deployment','owned-core'),('Service','owned-core')})
        configured=enabled();configured['agentControl']['enabled']=False
        self.assertEqual(self.render(configured),docs)
    def test_separate_listener_uses_same_image_dedicated_secret_and_private_readonly_files(self):
        docs=self.render(enabled());deployment=next(d for d in docs if d['kind']=='Deployment' and d['metadata']['name']=='owned-agent-control')
        self.assertEqual(deployment['spec']['replicas'],1);self.assertEqual(deployment['spec']['strategy'],{'type':'Recreate'})
        pod=deployment['spec']['template']['spec'];self.assertFalse(pod['automountServiceAccountToken']);self.assertFalse(pod['enableServiceLinks']);self.assertNotIn('initContainers',pod);self.assertNotIn('hostNetwork',pod);self.assertNotIn('fsGroup',pod['securityContext'])
        self.assertEqual(pod['securityContext']['runAsUser'],65532);self.assertTrue(pod['securityContext']['runAsNonRoot']);self.assertEqual(pod['securityContext']['seccompProfile'],{'type':'RuntimeDefault'})
        self.assertEqual(len(pod['containers']),1);container=pod['containers'][0]
        self.assertEqual(container['image'],BASE['image']);self.assertEqual(container['command'],['/usr/local/bin/spitzli-agent-control']);self.assertEqual(container['args'],['--config','/private/agent-control.json'])
        self.assertEqual(container['envFrom'],[{'secretRef':{'name':'owned-agent-env'}}]);self.assertNotIn('owned-http-env',json.dumps(deployment))
        self.assertEqual(container['securityContext'],{'readOnlyRootFilesystem':True,'allowPrivilegeEscalation':False,'capabilities':{'drop':['ALL']}})
        self.assertIn({'name':'private','mountPath':'/private','readOnly':True},container['volumeMounts'])
        private=next(v for v in pod['volumes'] if v['name']=='private');self.assertEqual(private,{'name':'private','persistentVolumeClaim':{'claimName':'owned-agent-private','readOnly':True}})
        self.assertFalse(any(d['kind'] in ['Secret','PersistentVolumeClaim','Ingress','RoleBinding','ClusterRoleBinding'] for d in docs))
    def test_listener_has_ephemeral_budget_for_core_namespace_quota(self):
        docs=self.render(enabled())
        pod=next(d for d in docs if d['kind']=='Deployment' and d['metadata']['name']=='owned-agent-control')['spec']['template']['spec']
        resources=pod['containers'][0]['resources']
        self.assertEqual(resources['requests'].get('ephemeral-storage'),'64Mi')
        self.assertEqual(resources['limits'].get('ephemeral-storage'),'256Mi')
    def test_private_service_and_ingress_are_only_reviewed_mtls_port(self):
        docs=self.render(enabled());service=next(d for d in docs if d['kind']=='Service' and d['metadata']['name']=='owned-agent-control')
        self.assertEqual(service['spec']['type'],'ClusterIP');self.assertEqual(service['spec']['ports'],[{'name':'mtls','port':3131,'targetPort':'mtls','protocol':'TCP'}])
        policy=next(d for d in docs if d['kind']=='NetworkPolicy');self.assertEqual(policy['spec']['podSelector']['matchLabels']['app.kubernetes.io/component'],'agent-control')
        self.assertEqual(policy['spec']['policyTypes'],['Ingress']);self.assertEqual(policy['spec']['ingress'],[{'from':[{'ipBlock':{'cidr':'10.0.0.1/32'}},{'ipBlock':{'cidr':'192.168.1.2/32'}}],'ports':[{'protocol':'TCP','port':3131}]}])
        self.assertEqual(service['spec']['selector']['app.kubernetes.io/component'],'agent-control')
    def test_enable_requires_refs_dedicated_env_private_source_and_mtls_review(self):
        for key,value in [('existingEnvSecret',''),('existingEnvSecret',BASE['existingSecret']),('existingPrivatePVC',''),('privateMTLSReviewed',False),('networkAdmissionExistingPolicy',''),('privateSourceIPv4',[])]:
            with self.subTest(key=key,value=value):
                config=enabled();config['agentControl'][key]=value;self.render(config,False)
        for ip in ['0.0.0.0','127.0.0.1','169.254.169.254','203.0.113.1','10.0.0.0/8','999.0.0.1','10.0.0.1\nallow all']:
            config=enabled();config['agentControl']['privateSourceIPv4']=[ip];self.render(config,False)
    def test_no_false_activation_mutable_pin_or_privilege_override(self):
        for key,value in [('enabled','true'),('privateMTLSReviewed','true'),('hostNetwork',True),('securityContext',{'privileged':True}),('port',80)]:
            config=enabled();config['agentControl'][key]=value;self.render(config,False)
        config=enabled();config['image']='registry.example/webdock/backend:latest';self.render(config,False)
if __name__=='__main__':unittest.main()
