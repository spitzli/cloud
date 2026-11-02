"""Finite one-test/six-retained Mail selection; no external effects."""
import hashlib,importlib.util,json,pathlib,tempfile,unittest
from unittest.mock import patch
P=pathlib.Path;ROOT=P(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('mail_retained_runner',P(__file__).with_name('recovery-admission-component.py'));r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
class MailRetainedBindingTests(unittest.TestCase):
 def select(self,manifest=None):
  # Only this manifest semantic poison is substituted; every parent/ELF/archive still uses real FD admission.
  original=r.verified_file;pin=ROOT/r.AUTHORITY_PIN_PATH
  old=json.loads((ROOT/'.runtime-test/recovery-mail-pg18-tools-overlay-54ff153d08ca/overlay.json').read_bytes());old['baseArtifactPinSHA256']=r.AUTHORITY_PIN_SHA256;old['files']['tools/tests/recovery-admission-component.py']=hashlib.sha256((ROOT/'tools/tests/recovery-admission-component.py').read_bytes()).hexdigest();old['files'][r.GATEWAY_RECIPE]=hashlib.sha256((ROOT/r.GATEWAY_RECIPE).read_bytes()).hexdigest()
  with tempfile.TemporaryDirectory(dir=ROOT/'.runtime-test',prefix='mail-binding-unit-') as tmp:
   overlay=P(tmp)/'overlay.json';overlay.write_text(json.dumps(old));digest=hashlib.sha256(overlay.read_bytes()).hexdigest()
   def admitted(path,expected,*args,**kw):
    path=P(path)
    if path==overlay:return overlay.read_bytes()
    if manifest is not None and path==pin:return json.dumps(manifest).encode()
    if path.is_relative_to(overlay.parent/'files'):
     return original(ROOT/path.relative_to(overlay.parent/'files'),expected,*args,**kw)
    return original(path,expected,*args,**kw)
   # Chart tree admission is real: copy only the four finite public text inputs.
   import shutil
   chart=overlay.parent/'files/deploy/helm/webdock-agent';chart.parent.mkdir(parents=True)
   shutil.copytree(ROOT/'deploy/helm/webdock-agent',chart)
   with patch.object(r,'verified_file',side_effect=admitted),patch('subprocess.run',side_effect=AssertionError('selection must not acquire/execute')):
    return r.select_whole_inputs(root=ROOT,artifact_pin=pin,driver='kubernetes',input_overlay=overlay,input_overlay_sha256=digest)
 def test_actual_current_test_and_six_retained_paths(self):
  selected=self.select();parent=ROOT/'.runtime-test/recovery-matched-mail-candidate-e3312f1bff72'
  self.assertEqual(selected['binaries']['webdock-migrate']['path'],parent/'webdock-migrate');self.assertEqual(selected['binaries']['webdock-mail']['path'],(ROOT/r.AUTHORITY_PIN_PATH).parent/'webdock-mail')
  self.assertEqual(selected['binaries']['bootstrap-test']['path'],(ROOT/r.RETAINED_TEST_PIN_PATH).parent/'bootstrap-test')
  self.assertEqual(selected['retainedMatchedMailPin'],parent/'pin.json');self.assertEqual(set(selected['compilation']['runtimeExecutables']),{'bootstrap-test'})
  with tempfile.TemporaryDirectory() as tmp:
   destination=P(tmp);r.copy_source_provenance(selected,destination)
   retained=json.loads((parent/'pin.json').read_bytes())
   self.assertEqual(hashlib.sha256((destination/'retained-mail-pin.json').read_bytes()).hexdigest(),r.RETAINED_MATCHED_MAIL_PIN_SHA256)
   self.assertEqual(hashlib.sha256((destination/'retained-mail-source.tar.gz').read_bytes()).hexdigest(),retained['sourceArchiveSHA256'])
   self.assertEqual(hashlib.sha256((destination/'retained-mail-compilation.json').read_bytes()).hexdigest(),retained['compilationSHA256'])
   worker=selected['mixedManifest']['diagnosticWorker']
   self.assertEqual(hashlib.sha256((destination/'diagnostic-mail-worker-source.tar.gz').read_bytes()).hexdigest(),worker['sourceArchiveSHA256'])
   self.assertEqual(hashlib.sha256((destination/'diagnostic-mail-worker-compilation.json').read_bytes()).hexdigest(),worker['compilationSHA256'])
   external=selected['mixedManifest']['externalMaylily']
   self.assertEqual(hashlib.sha256((destination/'external-maylily-source.tar.gz').read_bytes()).hexdigest(),external['archiveSHA256'])
   self.assertEqual(hashlib.sha256((destination/'external-maylily-receipt.json').read_bytes()).hexdigest(),external['receiptSHA256'])
 def test_foreign_parent_and_retained_binary_path_deny_before_effects(self):
  original=json.loads((ROOT/r.AUTHORITY_PIN_PATH).read_bytes())
  missing=json.loads(json.dumps(original));missing.pop('retainedMatchedMailPin')
  with self.assertRaises(ValueError):self.select(missing)
  for field in ('path','sha256'):
   value=json.loads(json.dumps(original));value['retainedMatchedMailPin'][field]='foreign'
   with self.assertRaises(ValueError):self.select(value)
  for name in ('webdock-migrate','webdock-mail'):
   value=json.loads(json.dumps(original));value['binaries'][name]['path']=str(((ROOT/r.RETAINED_MATCHED_MAIL_PIN_PATH).parent if name=='webdock-mail' else (ROOT/r.AUTHORITY_PIN_PATH).parent)/name)
   with self.assertRaises(ValueError):self.select(value)
if __name__=='__main__':unittest.main()
