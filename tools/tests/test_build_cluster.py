"""Offline checks for the standalone build host; never connect to a server."""
from pathlib import Path
import unittest
import subprocess
import sys
import yaml
ROOT = Path(__file__).resolve().parents[2] / 'deploy/build-cluster'

class BuildCluster(unittest.TestCase):
    def test_distinct_private_server(self):
        config = yaml.safe_load((ROOT / 'config.yaml').read_text())
        self.assertEqual(config['node-name'], 'webdock-build-eu-01')
        self.assertEqual(config['bind-address'], '10.83.0.1')
        self.assertEqual(config['cluster-cidr'], '10.84.0.0/16')
        self.assertEqual(config['service-cidr'], '10.85.0.0/16')
        self.assertNotIn('server', config)
        self.assertNotIn('token', config)
        self.assertTrue(config['secrets-encryption'])
        self.assertIn('traefik', config['disable'])
        self.assertEqual(config['write-kubeconfig-mode'], '0600')

    def test_namespace_credentials_quota_and_deny(self):
        docs = [x for x in yaml.safe_load_all((ROOT/'foundation.yaml').read_text()) if x]
        ns = next(x for x in docs if x['kind']=='Namespace')
        self.assertEqual(ns['metadata']['labels']['pod-security.kubernetes.io/enforce'], 'restricted')
        for sa in [x for x in docs if x['kind']=='ServiceAccount']:
            self.assertFalse(sa['automountServiceAccountToken'])
        role=next(x for x in docs if x['kind']=='Role')
        for rule in role['rules']:
            self.assertNotIn('*', rule['verbs'])
            self.assertFalse(set(rule['resources']) & {'secrets','pods/exec','pods/attach','nodes'})
        policies=[x for x in docs if x['kind']=='NetworkPolicy']
        deny=next(x for x in policies if x['metadata']['name']=='default-deny')
        self.assertEqual(deny['spec']['podSelector'], {})
        self.assertEqual(set(deny['spec']['policyTypes']), {'Ingress','Egress'})
        self.assertNotIn('egress',deny['spec'])
        quota=next(x for x in docs if x['kind']=='ResourceQuota')
        self.assertIn('limits.ephemeral-storage',quota['spec']['hard'])
        admission=next(x for x in docs if x['kind']=='ValidatingAdmissionPolicy')
        self.assertEqual(admission['spec']['failurePolicy'],'Fail')
        text=yaml.safe_dump(admission)
        for required in ['automountServiceAccountToken','hostNetwork','emptyDir','image','activeDeadlineSeconds','backoffLimit']:
            self.assertIn(required,text)
        self.assertNotIn('Unconfined',text)

    def test_node_pid_budget_is_finite_and_retains_system_and_kubernetes_headroom(self):
        config = yaml.safe_load((ROOT / 'config.yaml').read_text())
        args = dict(value.split('=', 1) for value in config['kubelet-arg'])
        self.assertGreater(int(args.get('pod-max-pids', '-1')), 0)
        self.assertLessEqual(int(args['pod-max-pids']), 1024)
        for key in ['system-reserved', 'kube-reserved']:
            resources = dict(value.split('=', 1) for value in args[key].split(','))
            self.assertGreater(int(resources.get('pid', '0')), 0)
            self.assertIn('memory', resources)
            self.assertIn('ephemeral-storage', resources)

    def test_bootstrap_preserves_existing_clusters(self):
        script=(ROOT/'bootstrap.sh').read_text()
        self.assertIn('sha256sum -c',script)
        self.assertIn('Existing K3s installation is not managed by this bootstrap',script)
        self.assertNotIn('get.k3s.io',script)
        self.assertNotIn('k3s-uninstall',script)
        self.assertNotIn('flush ruleset',script)
        self.assertIn('server --config',script)

    def test_verifier_requires_explicit_connection(self):
        result=subprocess.run([sys.executable,str(ROOT/'verify.py'),'--job-json','unused.json'],capture_output=True,text=True)
        self.assertEqual(result.returncode,2)
        self.assertIn('explicit connection',result.stderr)

    def test_qualification_keeps_process_sandbox_and_profiles(self):
        job=yaml.safe_load((ROOT/'buildkit-qualification.yaml').read_text())
        container=job['spec']['template']['spec']['containers'][0]
        self.assertIn('@sha256:',container['image'])
        self.assertNotIn('no-process-sandbox',str(container))
        self.assertNotIn('Unconfined',str(container))
        self.assertFalse(container['securityContext']['allowPrivilegeEscalation'])
        self.assertTrue(container['securityContext']['readOnlyRootFilesystem'])

    def test_firewall_never_derives_host_api_authority_from_pod_addresses(self):
        import importlib.util
        spec=importlib.util.spec_from_file_location('system_pods',ROOT/'sync-system-pods.py')
        module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
        def pod(namespace,address,node='webdock-build-eu-01'):
            return {'metadata':{'namespace':namespace},'spec':{'nodeName':node},'status':{'podIPs':[{'ip':address}]}}
        result=module.rules([pod('kube-system','10.84.0.3'),pod('webdock-builds','10.84.0.4')])
        self.assertEqual(result,'flush set inet webdock_build_host system_pods\n')
        firewall=(ROOT/'host.nft').read_text()
        self.assertNotIn('system_pods',firewall)
        self.assertNotIn('timeout 20s',firewall)
        self.assertIn('iifname "cni0" counter drop',firewall)
        self.assertIn('tcp dport { 53, 3128 } accept',firewall)

    def test_idmap_exception_remains_explicit_and_namespace_scoped(self):
        import importlib.util
        spec=importlib.util.spec_from_file_location('sandbox_profile',ROOT/'render-sandbox.py')
        module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
        for unsafe in ['runner:latest','runner@sha256:abc','../runner@sha256:'+'a'*64]:
            with self.assertRaises(ValueError): module.render(unsafe)
        image='registry.invalid/owned/runner@sha256:'+'a'*64
        docs=module.render(image)
        ns=next(item for item in docs if item['kind']=='Namespace')
        self.assertEqual(ns['metadata']['name'],'webdock-build-qualification')
        self.assertEqual(ns['metadata']['labels']['webdock.dev/profile'],'rootless-idmap-sandbox')
        self.assertEqual(ns['metadata']['annotations']['webdock.dev/runner-image'],image)
        role=next(item for item in docs if item['kind']=='Role')
        secrets=next(rule for rule in role['rules'] if rule['resources']==['secrets'])
        self.assertEqual(set(secrets['verbs']),{'create','get','delete'})
        for rule in role['rules']:
            self.assertFalse(set(rule['resources']) & {'pods/exec','pods/attach','nodes','serviceaccounts/token'})
        runtime=yaml.safe_load((ROOT/'sandbox-runtime.yaml').read_text())
        self.assertEqual(runtime['handler'],'webdock-build-sandbox')
        admission=next(item for item in yaml.safe_load_all((ROOT/'foundation.yaml').read_text()) if item and item['kind']=='ValidatingAdmissionPolicy')
        source=str(admission['spec'])
        for gate in ['rootless-idmap-sandbox','webdock-build-sandbox','approvedRunner','SETUID','SETGID']:
            self.assertIn(gate,source)

    def test_handoff_admission_excludes_tokens_and_unbounded_secret_keys(self):
        docs=list(yaml.safe_load_all((ROOT/'handoff-admission.yaml').read_text()))
        policy=docs[0]['spec']
        self.assertEqual(policy['failurePolicy'],'Fail')
        self.assertEqual(policy['matchConstraints']['namespaceSelector']['matchLabels'],{'webdock.dev/role':'isolated-builds'})
        self.assertEqual(policy['matchConstraints']['resourceRules'][0]['resources'],['secrets'])
        source=str(policy['validations'])
        for gate in ['Opaque','immutable','attempt.json','98304','handoff-sha256','size(object.data) == 1']:
            self.assertIn(gate,source)
        self.assertEqual(docs[1]['spec']['validationActions'],['Deny'])

    def test_staging_egress_is_one_literal_endpoint_and_build_selector(self):
        import importlib.util
        spec=importlib.util.spec_from_file_location('staging_policy',ROOT/'render-staging-policy.py')
        module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
        for address in ['0.0.0.0/0','::/0','0.0.0.0','::','127.0.0.1','169.254.169.254','ff02::1','example.com']:
            with self.assertRaises(ValueError): module.render(address)
        for port in [0,65536,True]:
            with self.assertRaises(ValueError): module.render('192.0.2.1',port)
        for address,cidr in [('192.0.2.1','192.0.2.1/32'),('2001:db8::1','2001:db8::1/128')]:
            policy=module.render(address,19443)
            self.assertEqual(policy['metadata']['namespace'],'webdock-build-qualification')
            self.assertEqual(policy['spec']['podSelector'],{'matchLabels':{'webdock.dev/component':'build-job'}})
            self.assertEqual(policy['spec']['egress'],[{'to':[{'ipBlock':{'cidr':cidr}}],'ports':[{'protocol':'TCP','port':19443}]}])

if __name__=='__main__': unittest.main()
