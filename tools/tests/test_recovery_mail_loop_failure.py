"""Actual preCore shell order with real unprivileged FDs/failures; external loop/mount calls mocked.
This proves failure ordering/FD lifetime only, not kernel loop ownership or autoclear behavior.
"""
import importlib.util,os,pathlib,shlex,subprocess,tempfile,unittest
P=pathlib.Path
spec=importlib.util.spec_from_file_location('loop_order_runner',P(__file__).with_name('recovery-admission-component.py'));r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
class MailLoopFailureTests(unittest.TestCase):
 def run_failure(self,block,pin,*,diagnostic=False):
  self.assertNotEqual(os.geteuid(),0)
  with tempfile.TemporaryDirectory(prefix='wd-loop-order-') as tmp:
   data=P(tmp)/'data';data.write_bytes(b'owned unit file, no real loop');log=P(tmp)/'diagnostic';log.write_bytes(b'original');log.chmod(0o400)
   block=block.replace('/diagnostics/mail-filesystem-loop',shlex.quote(str(log)))
   script='set -eu;maildev=$1;imagedev=$1;'+f'''losetup() {{ test "$1" = --detach;test -e /proc/$$/fd/"${{{pin}}}";printf 'autoclear-set\\n'; }}
owned_mail_loop_receipt() {{ test -e /proc/$$/fd/"$5"; }}
mkfs.ext4() {{ :; }}
mount() {{ printf 'mount-attempted\\n';return 1; }}
chmod() {{ :; }}
'''+block
   child=subprocess.Popen(['bash','-c',script,'owned-unit',str(data)],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
   out,_=child.communicate(timeout=3);self.assertNotEqual(child.returncode,0)
   self.assertIn(b'autoclear-set\n',out)
   if diagnostic:self.assertNotIn(b'mount-attempted',out)
   else:self.assertIn(b'mount-attempted',out)
   self.assertFalse(P('/proc',str(child.pid)).exists());self.assertEqual(log.read_bytes(),b'original')
 def test_actual_data_loop_is_autoclear_before_readonly_diagnostic_error(self):
  block=r.MAIL_FILESYSTEM_SETUP.split('maildev=$(losetup',1)[1].split('esac\n',1)[1].split("python3 -B - <<'MAIL_INITIAL_SPACE'",1)[0]
  self.run_failure(block,'mail_loop_pin',diagnostic=True)
 def test_actual_image_loop_is_autoclear_before_mount_failure(self):
  block=r.MAIL_FILESYSTEM_SETUP.split('imagedev=$(losetup',1)[1].split('\n',1)[1].split('mkdir -m 700 /owned/mail-data/image-store/engine',1)[0]
  self.run_failure(block,'image_loop_pin')
if __name__=='__main__':unittest.main()
