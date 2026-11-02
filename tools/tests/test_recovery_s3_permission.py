"""Execute the exact embedded permission client without network or credentials."""
import hashlib,pathlib,re,types,unittest
SOURCE=pathlib.Path(__file__).resolve().parents[2]/'crates/control-plane/tests/support/recovery_byte_component.rs'
def client():
 text=SOURCE.read_text();match=re.search(r'const S3_PERMISSION_PROBE: &str = r#"(.*?)"#;',text,re.S)
 if not match:raise AssertionError('embedded signed S3 permission probe missing')
 module=types.ModuleType('permission_definitions');exec(compile(match[1],str(SOURCE),'exec'),module.__dict__);return module
class S3PermissionTests(unittest.TestCase):
 def test_aws_published_get_object_signature(self):
  # AWS public GET Object example; example credentials are not fixture secrets.
  m=client();headers={'host':'examplebucket.s3.amazonaws.com','range':'bytes=0-9','x-amz-content-sha256':hashlib.sha256(b'').hexdigest(),'x-amz-date':'20130524T000000Z'}
  signature=m.signature('GET','/test.txt',headers,'wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY')
  self.assertEqual(signature,'f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41')
 def test_exact_xml_access_denied_only(self):
  m=client();body=b'<?xml version="1.0"?><Error><Code>AccessDenied</Code><Message>private ignored</Message><Resource>private ignored</Resource></Error>'
  for method in ('PUT','DELETE'):
   self.assertEqual(m.project(method,403,body),{'method':method,'httpStatus':403,'code':'AccessDenied','bytes':len(body),'sha256':hashlib.sha256(body).hexdigest()})
  self.assertEqual(m.project('GET',200,b'')['code'],'EmptyObject')
 def test_status_code_and_whole_xml_poison_fail_closed(self):
  m=client();good=b'<Error><Code>AccessDenied</Code></Error>'
  cases=[('PUT',200,good),('PUT',500,good),('GET',403,good),('GET',200,b'x'),('PUT',403,b''),('PUT',403,b'<Error><Code>SignatureDoesNotMatch</Code></Error>'),('DELETE',403,b'<Error><Code>InvalidAccessKeyId</Code></Error>'),('PUT',403,b'<Error><Code>AccessDenied</Code><Code>AccessDenied</Code></Error>'),('PUT',403,b'<Error><Nested><Code>AccessDenied</Code></Nested></Error>'),('PUT',403,b'<Other><Code>AccessDenied</Code></Other>'),('PUT',403,good+b'<Error/>'),('PUT',403,good+b'garbage'),('PUT',403,b'<Error><Code>AccessDenied</Code>'),('PUT',403,b'<!DOCTYPE Error [<!ENTITY x "AccessDenied">]><Error><Code>&x;</Code></Error>'),('PUT',403,b'<Error><Code>AccessDenied<child/></Code></Error>'),('PUT',403,b' '*(65536+1)),('POST',403,good)]
  for args in cases:
   with self.subTest(method=args[0],status=args[1],size=len(args[2])),self.assertRaises(ValueError):m.project(*args)
 def test_inputs_bound_to_exact_owned_targets_and_no_endpoint(self):
  m=client();valid={'accessKeyId':'public-unit-key','secretAccessKey':'public-unit-secret','bucket':'wd-app-authority-'+'a'*32,'canary':'authority/permission-delete-'+'b'*32,'putKey':'authority/permission-put-'+'c'*32}
  self.assertEqual(m.validate(valid),valid)
  for patch in ({'bucket':'foreign'},{'canary':'../secret'},{'putKey':'authority/other'},{'endpoint':'http://foreign'},{'accessKeyId':'bad\r\nvalue'},{'secretAccessKey':None}):
   with self.subTest(patch=tuple(patch)),self.assertRaises(ValueError):m.validate({**valid,**patch})
 def test_static_network_contract_no_proxy_redirect_or_extra_requests(self):
  text=SOURCE.read_text();raw=re.search(r'const S3_PERMISSION_PROBE: &str = r#"(.*?)"#;',text,re.S)[1]
  self.assertIn("HTTPConnection('127.0.0.1',55900,timeout=2)",raw);self.assertIn('settimeout(10)',raw);self.assertIn('response.read(65537)',raw)
  self.assertNotIn('urllib.request',raw);self.assertNotIn('os.environ',raw)
if __name__=='__main__':unittest.main()
