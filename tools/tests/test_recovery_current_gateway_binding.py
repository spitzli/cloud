"""Finite current gateway provenance; no Docker, provider, or network effects."""
import copy,importlib.util,json,pathlib,unittest
R=pathlib.Path(__file__).resolve().parents[2];spec=importlib.util.spec_from_file_location('gateway_binding',R/'tools/tests/recovery-admission-component.py');r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
class CurrentGatewayBindingTests(unittest.TestCase):
 def setUp(self):
  path=R/r.CURRENT_GATEWAY_PIN_PATH
  if not path.exists():self.skipTest('private compiled gateway capture is not present')
  self.pin=json.loads(path.read_bytes());self.record=json.loads((path.parent/'gateway-compilation.json').read_bytes());self.core={'rootLockSHA256':self.pin['rootLockSHA256']};self.binary=self.pin['binaries']['webdock-recovery-gateway']
 def test_actual_current_gateway_is_an_exact_non_test_runtime_artifact(self):r.validate_current_gateway_provenance(self.pin,self.record,self.core,self.binary)
 def test_altered_source_binary_lock_features_and_test_build_are_denied(self):
  for poison in ('source','binary','lock','feature','test','path'):
   p,a,c,b=map(copy.deepcopy,(self.pin,self.record,self.core,self.binary))
   if poison=='source':a['inputs']['crates/agent/src/recovery/gateway.rs']='0'*64
   elif poison=='binary':a['runtimeExecutables']['webdock-recovery-gateway']['copiedStrippedSHA256']='0'*64
   elif poison=='lock':a['rootLockSHA256']='0'*64
   elif poison=='feature':a['actualCargoArtifact']['features'].append('unknown')
   elif poison=='test':a['actualCargoArtifact']['profile']['test']=True
   else:p['compilerInputs']['../foreign']='0'*64
   with self.subTest(poison=poison),self.assertRaises(ValueError):r.validate_current_gateway_provenance(p,a,c,b)
if __name__=='__main__':unittest.main()
