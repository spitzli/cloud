import importlib.util,unittest
from pathlib import Path
ROOT=Path(__file__).resolve().parents[2]
class Volumes(unittest.TestCase):
 def module(self):
  p=ROOT/'deploy/observability/fixed-volumes.py';self.assertTrue(p.exists(),'OBS needs fixed retained filesystems before stateful rollout')
  s=importlib.util.spec_from_file_location('obs_fixed',p);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);return m
 def test_exact_nonroot_owners_and_total_reserve(self):
  m=self.module();self.assertEqual(sum(x[0] for x in m.VOLUMES.values()),53);self.assertEqual(m.VOLUMES['grafana'],(2,472,472));self.assertEqual(m.VOLUMES['loki'],(20,10001,10001));self.assertTrue(all(x[1]>0 for x in m.VOLUMES.values()))
 def test_boot_dependency_and_no_public_provisioner(self):
  m=self.module();s=m.dropin(['owned-a.mount','owned-b.mount']);self.assertIn('BindsTo=owned-a.mount owned-b.mount',s);self.assertIn('ExecStartPre=',s)
  for name,(gib,_,_) in m.VOLUMES.items():
   docs=m.resources(name,gib);self.assertEqual(docs[0]['provisioner'],'kubernetes.io/no-provisioner');self.assertEqual(docs[1]['spec']['persistentVolumeReclaimPolicy'],'Retain');self.assertEqual(docs[1]['spec']['nodeAffinity']['required']['nodeSelectorTerms'][0]['matchExpressions'][0]['values'],['webdock-obs-eu-01'])

class FinalizationAndFsGroup(unittest.TestCase):
 def fixture(self,phase,mode):
  import tempfile,os,json,types
  from unittest.mock import patch
  loader=Volumes();m=loader.module();tmp=tempfile.TemporaryDirectory();self.addCleanup(tmp.cleanup);root=Path(tmp.name)
  m.ROOT=root;m.STATE=root/'state.json';m.VOLUMES={'probe':(1,os.getuid(),os.getgid())}
  target=root/'mounts/probe';target.mkdir(parents=True);target.chmod(mode)
  state={'owner':'webdock-obs-eu-01','phase':phase,'volumes':{'probe':{'state':'ready','image':str(root/'images/probe.ext4'),'target':str(target),'bytes':1024**3}}};m.STATE.write_text(json.dumps(state))
  fake=types.SimpleNamespace(private=lambda *a:None,verify=lambda r:None)
  p=patch.object(m,'verify_dependencies',lambda *a:None,create=True);p.start();self.addCleanup(p.stop)
  return m,fake,state
 def test_partial_finalization_never_reports_idempotent_success(self):
  m,fake,_=self.fixture('initializing',0o700)
  with self.assertRaisesRegex(RuntimeError,'finalization'):m.install(fake)
 def test_chart_fs_group_mode_survives_verification(self):
  m,fake,state=self.fixture('ready',0o2770)
  self.assertEqual(m.verify(fake),state)

class DependencyNames(unittest.TestCase):
 def test_systemctl_quotes_literal_escaped_mount_names(self):
  import json
  m=Volumes().module();name=r'var-lib-webdock\x2dobservability.mount'
  self.assertEqual(m.dependency_names(json.dumps(name)+' sysinit.target'),{name,'sysinit.target'})

if __name__=='__main__':unittest.main()
