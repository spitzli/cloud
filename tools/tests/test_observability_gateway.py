import importlib.util,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
class Gateway(unittest.TestCase):
 def module(self):
  p=ROOT/'deploy/observability/private-gateway.py';self.assertTrue(p.exists(),'operator-only Loki requires authenticated private routing')
  s=importlib.util.spec_from_file_location('obs_gateway',p);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);return m
 def test_roles_derive_scope_and_strip_client_authorization(self):
  m=self.module();s=m.caddyfile('reader-hash','collector-hash');self.assertIn('method POST',s);self.assertIn('method GET',s);self.assertIn('header_up X-Scope-OrgID platform',s);self.assertIn('header_up -Authorization',s);self.assertNotIn('{$',s);self.assertIn('request_body',s)
 def test_public_s3_ips_only_and_no_raw_loki_namespace_wide_ingress(self):
  m=self.module()
  for address in ['0.0.0.0','127.0.0.1','169.254.169.254','10.0.4.1','154.49.215.100/24']:
   with self.assertRaises(ValueError):m.policies([address])
  docs=m.policies(['154.49.215.100']);general=next(x for x in docs if x['metadata']['name']=='obs-infrastructure-private');self.assertEqual(general['spec']['podSelector']['matchExpressions'][0]['operator'],'NotIn')
  loki=next(x for x in docs if x['metadata']['name']=='obs-loki-gateway-only');self.assertEqual(loki['spec']['ingress'][0]['from'][0]['podSelector']['matchLabels'],{'app.kubernetes.io/name':'obs-loki-auth'})
 def test_gateway_is_nonroot_bounded_tokenless_and_clusterip(self):
  m=self.module();d=m.workloads();pod=d[0]['spec']['template']['spec'];self.assertFalse(pod['automountServiceAccountToken']);self.assertTrue(pod['securityContext']['runAsNonRoot']);self.assertTrue(pod['containers'][0]['securityContext']['readOnlyRootFilesystem']);self.assertEqual(d[1]['spec']['type'],'ClusterIP');self.assertIn('@sha256:',pod['containers'][0]['image']);self.assertEqual(pod['containers'][0]['command'],['/work/caddy'])
class FileCapabilities(unittest.TestCase):
 def test_upstream_file_capability_is_not_added_to_pod(self):
  m=Gateway().module();pod=m.workloads()[0]['spec']['template']['spec'];main=pod['containers'][0];init=pod['initContainers'][0]
  self.assertEqual(init['image'],main['image']);self.assertEqual(init['securityContext']['capabilities'],{'drop':['ALL']});self.assertFalse(init['securityContext']['allowPrivilegeEscalation'])
  self.assertEqual([v['name'] for v in init['volumeMounts']],['binary']);self.assertTrue(next(v for v in main['volumeMounts'] if v['name']=='binary')['readOnly']);self.assertIn('cmp /usr/bin/caddy /work/caddy',init['args'][1])

class CaddySyntax(unittest.TestCase):
 def test_actual_caddy_adapter_accepts_generated_routes(self):
  import os,subprocess,tempfile
  binary=Path(os.environ.get('SPITZLI_CADDY_BINARY','/tmp/webdock-caddy-2.11.7/caddy'))
  if not binary.exists():self.skipTest('Pinned Caddy binary required for actual adapter check')
  m=Gateway().module();hashed=subprocess.check_output([str(binary),'hash-password','--algorithm','bcrypt','--bcrypt-cost','4'],input=b'owned-test-only-not-a-service-secret\n').decode().strip()
  with tempfile.TemporaryDirectory() as root:
   file=Path(root)/'Caddyfile';file.write_text(m.caddyfile(hashed,hashed))
   result=subprocess.run([str(binary),'adapt','--config',str(file),'--adapter','caddyfile'],capture_output=True,text=True)
   self.assertEqual(result.returncode,0,result.stderr)

if __name__=='__main__':unittest.main()
