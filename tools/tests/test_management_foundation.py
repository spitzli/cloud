import importlib.util,unittest,ipaddress
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
class Profiles(unittest.TestCase):
 def test_core_https_is_explicit_and_preserves_private_api_and_no_bridge(self):
  rules=self.module().render('core',public_https=True)['host.nft']
  self.assertIn('iifname "eth0" tcp dport { 80, 443 } accept',rules)
  self.assertIn('iifname "eth*" oifname "eth*" drop',rules)
  self.assertNotIn('tcp dport 6443 accept',rules)
 def test_only_obs_exposes_operator_https_and_acme_not_cluster_apis(self):
  m=self.module()
  self.assertIn('iifname "eth0" tcp dport { 80, 443 } accept',m.render('obs')['host.nft'])
  self.assertNotIn('tcp dport { 80, 443 } accept',m.render('core')['host.nft'])
 def module(self):
  p=ROOT/'deploy/management-cluster/profile.py';self.assertTrue(p.exists(),'fixed Core/OBS profiles must exist before provisioning')
  spec=importlib.util.spec_from_file_location('management_profile',p);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m);return m
 def test_fixed_roles_private_api_distinct_networks_and_no_run_join(self):
  m=self.module();nets=[]
  for role in ['core','obs']:
   files=m.render(role);config=files['config.yaml'];self.assertIn('secrets-encryption: true',config);self.assertNotIn('\nserver:',config);self.assertIn('local-storage',config)
   for line in config.splitlines():
    if line.startswith(('cluster-cidr:','service-cidr:')):nets.append(ipaddress.ip_network(line.split(': ')[1]))
   self.assertIn('127.0.0.1',config);self.assertNotIn('0.0.0.0',config)
   self.assertIn('iifname \"eth*\" oifname \"eth*\" drop',files['host.nft']);self.assertIn('policy drop',files['host.nft']);self.assertIn('tcp dport 22 accept',files['host.nft']);self.assertNotIn('tcp dport 6443 accept',files['host.nft'])
  self.assertTrue(all(not a.overlaps(b) for i,a in enumerate(nets) for b in nets[i+1:]))
  with self.assertRaises(ValueError):m.render('run')
 def test_bootstrap_refuses_wrong_host_existing_cluster_and_drift(self):
  self.module();s=(ROOT/'deploy/management-cluster/bootstrap.sh').read_text()
  for expected in ['hostname','Refusing existing unowned K3s','Refusing configuration drift','sha256sum','systemctl','--fresh-management-host']:
   self.assertIn(expected,s)
  self.assertNotIn('curl -sfL https://get.k3s.io',s);self.assertNotIn('uninstall',s)
if __name__=='__main__':unittest.main()
