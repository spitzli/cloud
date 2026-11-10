"""Offline only: no host access, key generation, token issuance or deployment."""
import base64
import copy
import importlib.util
import json
import subprocess
import sys
from pathlib import Path
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]


class Profile(unittest.TestCase):
    def module(self):
        path=ROOT/'profile.py';self.assertTrue(path.is_file(),'disabled controller profile is missing')
        spec=importlib.util.spec_from_file_location('controller_profile',path);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m

    def config(self):
        c=json.loads((ROOT/'connection.example.json').read_text())
        c.update(controllerHostName='owned-controller-test',controllerPublicIPv4='8.8.8.8',controllerTunnelIPv4='10.83.0.2',controllerPublicKey=base64.b64encode(bytes(range(32))).decode(),buildPublicKey=base64.b64encode(bytes(range(1,33))).decode(),buildCASHA256='a'*64,serviceAccountUID='service-account-test-uid',revocationSecretUID='revocation-test-uid',audience='https://kubernetes.default.svc.cluster.local')
        return c

    def test_disabled_unknown_owner_inputs_never_create_operational_files(self):
        m=self.module();c=json.loads((ROOT/'connection.example.json').read_text());self.assertFalse(c['enabled'])
        with self.assertRaises(ValueError):m.render(c)
        result=subprocess.run([sys.executable,str(ROOT/'profile.py'),'--render','--config',str(ROOT/'connection.example.json')],capture_output=True,text=True)
        self.assertEqual(result.returncode,2)
        self.assertEqual(result.stdout,'')

    def test_peer_endpoint_routes_and_file_references_reject_broader_scope(self):
        m=self.module();c=self.config();m.validate(c)
        for key,value in [('apiEndpoint','https://167.86.75.144:6443'),('apiEndpoint','https://127.0.0.1:16443'),('controllerPublicIPv4','0.0.0.0/0'),('controllerTunnelIPv4','10.84.0.2'),('controllerTunnelIPv4','10.83.0.1'),('controllerPrivateKeyFile','relative.key'),('controllerPrivateKeyFile','/tmp/unowned.key'),('buildPublicKey',base64.b64encode(bytes(32)).decode())]:
            bad=copy.deepcopy(c);bad[key]=value
            with self.subTest(key=key,value=value),self.assertRaises(ValueError):m.validate(bad)

    def test_rendered_guards_admit_only_one_private_peer_api_and_never_forward(self):
        m=self.module();files=m.render(self.config())
        allow=files['build-host-input.review.nft']
        self.assertIn('iifname "wd-build-ctl" ip saddr 10.83.0.2 ip daddr 10.83.0.1 tcp dport 6443 accept',allow)
        self.assertNotIn('10250',allow)
        self.assertIn('iifname "wd-build-ctl" counter drop',files['build-guard.nft'])
        self.assertIn('oifname "wd-build-ctl" counter drop',files['build-guard.nft'])
        self.assertIn('ip saddr != 8.8.8.8 counter drop',files['build-guard.nft'])
        self.assertIn('meta nfproto ipv6 udp dport 51820 counter drop',files['controller-guard.nft'])
        self.assertIn('ip saddr 167.86.75.144 ip daddr 8.8.8.8 udp dport 51820 accept',files['controller-host-input.review.nft'])
        self.assertIn('allowed-ips 10.83.0.1/32',files['controller-link.service'])
        self.assertNotIn('0.0.0.0/0',''.join(files.values()))
        self.assertIn('network-approved.sha256',files['controller-link.service'])
        self.assertIn('worker-reload-qualified',files['worker-auth.disabled.conf'])

    def test_existing_run_cluster_is_a_review_target_but_canary_and_activation_stay_gated(self):
        m=self.module();c=self.config();c['controllerPublicIPv4']='213.136.65.22'
        files=m.render(c)
        activation=json.loads(files['activation.json'])
        self.assertFalse(activation['networkEnabledInConfig'])
        self.assertTrue(activation['workerActivationBlocked'])
        gate=json.loads(files['bootstrap-readiness.review.json'])
        self.assertEqual(gate['request']['spec']['expirationSeconds'],600)
        self.assertEqual(gate['request']['spec']['boundObjectRef']['uid'],c['revocationSecretUID'])
        self.assertNotEqual(gate['request']['spec']['audiences'],[c['audience']])
        self.assertEqual(gate['requiredDenial']['statuses'],[403,422])
        self.assertEqual(gate['requiredDenial']['policy'],'webdock-controller-token-request')
        self.assertTrue(gate['mustPassBeforeBootstrapDelivery'])
        self.assertTrue(gate['mustPassBeforeWorkerStart'])

    def test_scope_is_only_existing_namespaced_roles_and_self_token_request(self):
        m=self.module();items=json.loads(m.render(self.config())['identity-rbac.review.json'])['items']
        self.assertFalse(any(x['kind'] in ['ClusterRole','ClusterRoleBinding'] for x in items))
        role=next(x for x in items if x['kind']=='Role')
        self.assertEqual(role['rules'],[{'apiGroups':[''],'resources':['serviceaccounts/token'],'resourceNames':['webdock-build-controller'],'verbs':['create']}])
        bindings=[x for x in items if x['kind']=='RoleBinding']
        self.assertTrue(all(x['subjects']==[{'kind':'ServiceAccount','name':'webdock-build-controller','namespace':'webdock-build-controller'}] for x in bindings))
        self.assertNotIn('spitzli-build-storage-node',str(items))
        request=json.loads(m.render(self.config())['token-request.review.json'])
        self.assertEqual(request['spec']['expirationSeconds'],3600)
        self.assertEqual(request['spec']['boundObjectRef'],{'kind':'Secret','apiVersion':'v1','name':'webdock-controller-revocation','uid':'revocation-test-uid'})

    def test_private_identity_input_rejects_open_parent_and_replaced_paths(self):
        m=self.module()
        with tempfile.TemporaryDirectory() as temporary:
            directory=Path(temporary);path=directory/'token';path.write_bytes(b'private-token');path.chmod(0o600)
            self.assertEqual(m.private_read(path),b'private-token')
            directory.chmod(0o755)
            with self.assertRaises(ValueError):m.private_read(path)
            directory.chmod(0o700);linked=directory/'linked';linked.symlink_to(path)
            with self.assertRaises(ValueError):m.private_read(linked)
            path.chmod(0o644)
            with self.assertRaises(ValueError):m.private_read(path)

    def test_claim_checks_reject_expiry_scope_and_replaced_identity(self):
        m=self.module();c=self.config();now=10000
        claims={'sub':'system:serviceaccount:webdock-build-controller:webdock-build-controller','aud':[c['audience']],'iat':now,'nbf':now,'exp':now+3600,'kubernetes.io':{'namespace':'webdock-build-controller','serviceaccount':{'name':'webdock-build-controller','uid':c['serviceAccountUID']},'secret':{'name':'webdock-controller-revocation','uid':c['revocationSecretUID']}}}
        m.check_claims(claims,c,now)
        for patch in [{'exp':now-1},{'exp':now+7200},{'sub':'system:serviceaccount:kube-system:admin'},{'aud':['foreign-audience']},{'iat':now+300},{'kubernetes.io':{'namespace':'foreign'}}]:
            bad=copy.deepcopy(claims);bad.update(patch)
            with self.subTest(patch=patch),self.assertRaises(ValueError):m.check_claims(bad,c,now)
        for kind in ['serviceaccount','secret']:
            bad=copy.deepcopy(claims);bad['kubernetes.io'][kind]['uid']='replacement'
            with self.assertRaises(ValueError):m.check_claims(bad,c,now)


if __name__=='__main__':unittest.main()
