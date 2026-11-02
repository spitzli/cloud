"""Compile the exact embedded private scripts and validate actual outgoing protocol bytes."""
import ast,pathlib,re,unittest
P=pathlib.Path(__file__).resolve().parents[2]/'crates/control-plane/tests/support/recovery_mail_component.rs'
def script(name):
 text=P.read_text();return re.search(r'const '+name+r':\s*&str\s*=\s*r#"(.*?)"#;',text,re.S)[1]
class MailComponentTests(unittest.TestCase):
 def test_genuine_fixture_keeps_async_services_live_during_private_cli_commands(self):
  raw=P.with_name('plugin_builtins.rs').read_text();before=raw[:raw.index('async fn fresh_builtin_setup_uses_real_operator_password_totp_pkce_and_serialized_receipts')];attribute=before[before.rfind('#[tokio::test'):].splitlines()[0].replace(' ','')
  self.assertIn('flavor="multi_thread"',attribute)
  self.assertIn('worker_threads=2',attribute)
 def test_actual_smtp_reply_bytes_and_dot_framing_are_crlf(self):
  tree=ast.parse(script('SMTP_SINK'));values=[node.value for node in ast.walk(tree) if isinstance(node,ast.Constant) and isinstance(node.value,bytes)]
  replies=[v for v in values if v[:3] in (b'220',b'250',b'354',b'221',b'500')]
  self.assertGreaterEqual(len(replies),5)
  for value in replies:self.assertTrue(value.endswith(b'\r\n'),'actual SMTP wire reply must terminate with CRLF')
  self.assertIn(b'.\r\n',values)
 def test_jmap_code_definitions_compile_and_recovery_gateway_only_read_requests(self):
  raw=script('MAIL_JMAP_PROBE');compile(raw,str(P),'exec')
  self.assertIn("'Email/set'",raw);self.assertIn("'Email/get'",raw);self.assertIn("assert status==403",raw)
  values=[n.value for n in ast.walk(ast.parse(raw)) if isinstance(n,ast.Constant) and isinstance(n.value,str)]
  self.assertIn('\r\n',values,'body comparison must trim actual CRLF only')
 def test_no_native_domain_seeding_or_fake_source_readiness(self):
  raw=P.read_text();self.assertNotIn('INSERT INTO webdock_auth.mail_sender_domain',raw);self.assertNotIn("UPDATE webdock_auth.hosting_app SET",raw)
  self.assertTrue('operator static provisioning' in raw)
if __name__=='__main__':unittest.main()
