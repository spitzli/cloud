"""Real private filesystem checks; only root identity/chown effects are mocked."""
import importlib.util,json,os,pathlib,subprocess,tempfile,unittest
P=pathlib.Path
spec=importlib.util.spec_from_file_location('install_runner',P(__file__).with_name('recovery-admission-component.py'));r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
NAMES=('inputs.json','writer.json','reader.json','identity.agekey','operator.pub')
class OperatorInstallTests(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup);self.root=P(self.tmp.name);self.source=self.root/'source';self.target=self.root/'target';self.source.mkdir(mode=0o700)
  for name in NAMES:(self.source/name).write_bytes(b'owned-test-material');(self.source/name).chmod(0o600)
  self.receipt=self.root/'receipt';self.calls=self.root/'install-calls'
 def run_install(self,wrong_owner=False):
  # UID0 installation itself is a fixture-only root effect. Exercise actual install,
  # files/modes/links while mocking only privileged ownership and its observation.
  prefix=r'''
id(){ if test "$1" = -u; then printf '0\n'; else command id "$@"; fi; }
stat(){
 local path="${@: -1}" raw uid gid mode links bytes
 raw=$(command stat "$@") || return
 read -r uid gid mode links bytes <<<"$raw"
 if [[ "$path" = "$TARGET" || "$path" = "$TARGET/"* ]]; then uid=0;gid=0;fi
 if test "$WRONG_OWNER" = yes && test "$path" = "$SOURCE/writer.json"; then uid=999999;gid=999999;fi
 printf '%s %s %s %s %s\n' "$uid" "$gid" "$mode" "$links" "$bytes"
}
install(){
 printf 'install\n' >>"$CALLS"
 local kept=()
 while test "$#" -gt 0; do
  case "$1" in -o|-g) test "$2" = 0 || return 90;shift 2;; *) kept+=("$1");shift;; esac
 done
 command install "${kept[@]}"
}
'''
  env={**os.environ,'SOURCE':str(self.source),'TARGET':str(self.target),'RECEIPT':str(self.receipt),'CALLS':str(self.calls),'WRONG_OWNER':'yes' if wrong_owner else 'no'}
  script=prefix+r.OPERATOR_INPUT_INSTALL+'\nowned_install_operator_inputs "$SOURCE" "$TARGET" "$RECEIPT" '+str(os.getuid())+' '+str(os.getgid())+'\n'
  return subprocess.run(['bash','-c',script],env=env,capture_output=True,timeout=5)
 def assert_denied_without_install(self,**kw):
  result=self.run_install(**kw);self.assertNotEqual(result.returncode,0);self.assertFalse(self.calls.exists());self.assertFalse(self.target.exists());self.assertIn('after\tinputs.json\tmissing',self.receipt.read_text())
 def test_owned_valid_finite_installation_has_private_modes_and_safe_receipt(self):
  result=self.run_install();self.assertEqual(result.returncode,0)
  self.assertEqual({p.name for p in self.target.iterdir()},set(NAMES));self.assertEqual(self.target.stat().st_mode&0o777,0o700)
  for name in NAMES:self.assertEqual((self.target/name).read_bytes(),b'owned-test-material');self.assertEqual((self.target/name).stat().st_mode&0o777,0o600)
  receipt=self.receipt.read_text();self.assertNotIn('owned-test-material',receipt);self.assertIn('before\tinputs.json\tregular',receipt);self.assertIn('after\tinputs.json\tregular\t0\t0\t600\t1\t',receipt)
 def test_wrong_owner_denied_before_install(self):self.assert_denied_without_install(wrong_owner=True)
 def test_extra_entry_denied_before_install(self):
  (self.source/'unexpected').write_bytes(b'not-approved');self.assert_denied_without_install()
 def test_symlink_denied_before_install(self):
  (self.source/'writer.json').unlink();(self.source/'writer.json').symlink_to(self.source/'reader.json');self.assert_denied_without_install()
 def test_hardlink_denied_before_install(self):
  (self.source/'writer.json').unlink();os.link(self.source/'reader.json',self.source/'writer.json');self.assert_denied_without_install()
 def test_missing_entry_denied_before_install(self):
  (self.source/'operator.pub').unlink();self.assert_denied_without_install()
 def test_oversize_denied_before_install(self):
  (self.source/'inputs.json').write_bytes(b'x'*16385);self.assert_denied_without_install()
 def test_existing_target_never_adopted(self):
  self.target.mkdir(mode=0o700);result=self.run_install();self.assertNotEqual(result.returncode,0);self.assertFalse(self.calls.exists())
 def test_actual_script_aborts_before_engine_on_installation_denial(self):
  source=P(r.__file__).read_text()
  call='owned_install_operator_inputs /operator-recovery-inputs /owned/operator-recovery /diagnostics/operator-input-install.tsv __OPERATOR_SOURCE_UID__ __OPERATOR_SOURCE_GID__ || exit 1'
  self.assertIn(call,source);self.assertLess(source.index(call),source.index('dockerd --host=unix:///var/run/docker.sock'))
  self.assertNotIn('mv /operator-recovery-inputs',source);self.assertNotIn('chown -R',r.OPERATOR_INPUT_INSTALL)
if __name__=='__main__':unittest.main()
