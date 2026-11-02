import copy,importlib.util,json,pathlib,unittest
R=pathlib.Path(__file__).resolve().parents[2];spec=importlib.util.spec_from_file_location('agent_binding',R/'tools/tests/recovery-admission-component.py');r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
class CurrentAgentBindingTests(unittest.TestCase):
 def setUp(self):
  p=R/r.CURRENT_AGENT_PIN_PATH;self.pin=json.loads(p.read_bytes());self.agent=json.loads((p.parent/'agent-compilation-v2.json').read_bytes());m=json.loads((R/r.AUTHORITY_PIN_PATH).read_bytes());self.core=json.loads((pathlib.Path(m['retainedTestPin']['path']).parent/'compilation.json').read_bytes());self.binary=m['binaries']['webdock-agent']
 def test_actual_current_agent_provenance_matches_its_compiler_and_exact_finite_sources(self):
  r.validate_current_agent_provenance(self.pin,self.agent,self.core,self.binary)
  self.assertIn('crates/agent/src/plugin_runtime.rs',self.pin['compilerInputs'])
 def test_source_binary_root_lock_and_build_feature_poisons_are_denied(self):
  for change in ['source','binary','lock','profile','feature','extra_path']:
   p,a,c,b=map(copy.deepcopy,(self.pin,self.agent,self.core,self.binary))
   if change=='source':a['inputs']['crates/agent/src/plugin_runtime.rs']='0'*64
   elif change=='binary':a['runtimeExecutables']['webdock-agent']['copiedStrippedSHA256']='0'*64
   elif change=='lock':a['rootLockSHA256']='0'*64
   elif change=='profile':a['actualCargoArtifact']['profile']['debuginfo']=2
   elif change=='feature':a['actualCargoArtifact']['features'].remove('kubernetes')
   else:p['compilerInputs']['../outside']='0'*64
   with self.subTest(change=change),self.assertRaises(ValueError):r.validate_current_agent_provenance(p,a,c,b)
if __name__=='__main__':unittest.main()
