"""Actual generated Mail builder and strict client-major gates; no Docker/services."""
import ast,importlib.util,pathlib,re,types,unittest
P=pathlib.Path
spec=importlib.util.spec_from_file_location('pg_mail_runner',P(__file__).with_name('recovery-admission-component.py'));m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class MailPgToolsTests(unittest.TestCase):
 def test_actual_main_recipe_installs_only_audited_cached_pg18_closure_before_build_audit(self):
  self.assertTrue(hasattr(m,'MAIL_PG18_STAGE'),'current main never installs required PostgreSQL clients')
  tree=ast.parse(P(m.__file__).read_text());main=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='main')
  call=next(n for n in ast.walk(main) if isinstance(n,ast.Call) and isinstance(n.func,ast.Attribute) and n.func.attr=='write_text' and isinstance(n.func.value,ast.BinOp) and isinstance(n.func.value.right,ast.Constant) and n.func.value.right.value=='Dockerfile')
  value=eval(compile(ast.Expression(call.args[0]),m.__file__,'eval'),{**vars(m),'args':types.SimpleNamespace(mail=True),'RUN_LABEL':'own.run','KIND_LABEL':'own.kind','KIND':'own','run':'a'*12,'extra_binaries':' webdock-migrate webdock-mail','relay_copy':'','tool_build_receipt':'RUN actual-tool-build-audit\n'})
  self.assertIn('FROM '+m.PG+' AS pg18',value);self.assertIn(m.MAIL_PG18_INSTALL,value)
  self.assertLess(value.index(m.MAIL_PG18_INSTALL),value.index('RUN actual-tool-build-audit'))
  self.assertNotIn('postgresql-client',value)
  for tool in ('psql','pg_dump','pg_restore'):
   self.assertIn('/usr/local/bin/'+tool,m.MAIL_PG18_STAGE);self.assertIn(repr(tool)+":('/usr/bin/"+tool+"',['--version'])",m.MAIL_TOOL_AUDIT)
  for library in ('libpq.so.5','libedit.so.0','libssl.so.3','libcrypto.so.3','libgssapi_krb5.so.2','libldap.so.2','libncursesw.so.6','libkeyutils.so.1'):
   self.assertIn(library,m.MAIL_PG18_STAGE)
  self.assertIn('/opt/webdock-pg18/lib/ld-musl-x86_64.so.1 --library-path /opt/webdock-pg18/lib',m.MAIL_PG18_INSTALL)
  self.assertIn('"$@"',m.MAIL_PG18_INSTALL)
  for block in (m.MAIL_PG18_STAGE,m.MAIL_PG18_INSTALL):
   for line in block.splitlines():
    if line.startswith('RUN '):__import__('subprocess').run(['bash','-n'],input=line[4:].encode(),capture_output=True,check=True,timeout=2)
  self.assertIn('/tmp/rootfs.py /tmp/mail /mail-rootfs mail',m.mail_image_recipe('a'*12,'own.run','own.kind','own','wd-owned-recovery-component:'+'b'*12))
  rootfs=P(__file__).with_name('recovery_gateway_rootfs.py').read_text();self.assertIn('smoke_mail(sys.argv[2],sys.argv[4],sys.argv[5])',rootfs);self.assertIn("'--userspec=65532:65532'",rootfs)
 def test_actual_major_check_denies_other_versions_and_malformed_output(self):
  self.assertTrue(hasattr(m,'MAIL_TOOL_AUDIT'))
  tree=ast.parse(m.MAIL_TOOL_AUDIT)
  check=next(n for n in ast.walk(tree) if isinstance(n,ast.If) and any(isinstance(x,ast.Constant) and x.value=='wrong-pg-major' for x in ast.walk(n)) and any(isinstance(x,ast.Call) and isinstance(x.func,ast.Attribute) and x.func.attr=='fullmatch' for x in ast.walk(n.test)))
  for tool in ('psql','pg_dump','pg_restore'):
   for raw,denied in [((tool+' (PostgreSQL) 18.3\n').encode(),False),((tool+' (PostgreSQL) 17.9\n').encode(),True),(b'not a version\n',True),((tool+' (PostgreSQL) 18.3\npoison').encode(),True)]:
    self.assertEqual(bool(eval(compile(ast.Expression(check.test),'<actual-major-check>','eval'),{'name':tool,'raw':raw,'re':re})),denied)
if __name__=='__main__':unittest.main()
