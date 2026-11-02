"""Exact embedded static probe: owned kernel regression and fail-closed metadata cases."""
import contextlib,importlib.util,json,pathlib,re,shutil,subprocess,sys,types,unittest
from unittest.mock import patch
SOURCE=pathlib.Path(__file__).resolve().parents[2]/'crates/control-plane/tests/support/recovery_byte_component.rs'
def probe():
 raw=re.search(r'const NETWORK_ISOLATION_PROBE: &str = r#"(.*?)"#;',SOURCE.read_text(),re.S)[1]
 module=types.ModuleType('network_probe_definitions');exec(compile(raw,str(SOURCE),'exec'),module.__dict__);return raw,module
class NetworkProbeTests(unittest.TestCase):
 def test_empty_and_headerless_route_enumeration(self):
  _,m=probe()
  for text in ('','\n','Iface Destination Gateway Flags RefCnt Use Metric Mask MTU Window IRTT\n'):
   self.assertEqual(m.rows(text,ipv4=True),[])
  self.assertEqual(len(m.rows('lo 00000000 00000000 0001 0 0 0 00000000 0 0 0\n',ipv4=True)),1)
 def test_foreign_interface_denied_before_connect(self):
  _,m=probe()
  with patch.object(m,'read_table',return_value=' lo: 0\n eth0: 0\n'),patch.object(m.socket,'socket',side_effect=AssertionError('must-not-open')):
   code,result=m.inspect_network()
  self.assertEqual(code,1);self.assertEqual(result['stage'],'interfaces');self.assertEqual(result['nonLoopbackCount'],1);self.assertIsNone(result['relayConnected'])
 def test_active_or_wrong_hardware_fallback_denied(self):
  for family,flags in [(1,128),(768,129),(768,192)]:
   _,m=probe();fake=unittest.mock.MagicMock();fake.__enter__.return_value=fake;fake.fileno.return_value=1
   def ioctl(_,command,__):return b'\0'*16+m.struct.pack('H',family if command==0x8927 else flags)+b'\0'*238
   with patch.object(m,'read_table',return_value='lo: 0\ntunl0: 0\n'),patch.object(m.socket,'socket',return_value=fake),patch.object(m.fcntl,'ioctl',side_effect=ioctl):code,result=m.inspect_network()
   self.assertEqual(code,1);self.assertEqual(result['stage'],'fallback');self.assertIsNone(result['relayConnected'])
 def test_failure_class_is_static_and_message_never_projected(self):
  _,m=probe()
  with patch.object(m,'read_table',side_effect=PermissionError('private-value')):code,result=m.inspect_network()
  self.assertEqual(code,1);self.assertEqual(result['exception'],'PermissionError');self.assertNotIn('private-value',json.dumps(result));self.assertEqual(result['stage'],'read-proc')
 def test_actual_owned_user_net_namespace(self):
  raw,_=probe();binary=shutil.which('unshare')
  if binary is None:self.skipTest('unshare unavailable; no kernel proof')
  result=subprocess.run([binary,'--user','--map-root-user','--net',sys.executable,'-B','-c',raw],capture_output=True,timeout=5)
  if result.returncode and result.stderr.startswith(b'unshare: unshare failed: Operation not permitted'):self.skipTest('owned user/net namespace prohibited; no kernel proof')
  self.assertEqual(result.returncode,0,'static owned namespace probe failed; raw bytes withheld');self.assertEqual(result.stderr,b'')
  row=json.loads(result.stdout);self.assertEqual(row['stage'],'complete');self.assertEqual(row['ipv4RouteCount'],0);self.assertEqual(row['ipv6NonLoopbackRouteCount'],0);self.assertEqual(row['ipv6NonLoopbackAddressCount'],0);self.assertFalse(row['relayConnected']);self.assertFalse(row['externalConnected']);self.assertEqual(row['externalErrno'],101)
  if row['fallbackPresent']:
   for name in ('fallbackHardwareTunnel','fallbackDown','fallbackNoIPv4','fallbackNoIPv6'):self.assertIs(row[name],True)
if __name__=='__main__':unittest.main()
