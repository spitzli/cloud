import importlib.util,pathlib,unittest,json,subprocess,sys,os
from unittest.mock import patch
p=pathlib.Path(__file__).with_name('recovery_operator_fixture.py')
spec=importlib.util.spec_from_file_location('operator_fixture',p);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class OperatorInputsTests(unittest.TestCase):
 def bare(self):
  obj=m.OperatorInputs.__new__(m.OperatorInputs);obj.items=[];obj.persist=lambda:None;return obj
 def test_existing_provider_resource_never_reused(self):
  obj=self.bare();obj.absent=lambda *args:False;calls=[]
  with self.assertRaises(RuntimeError):obj.create('user','foreign',lambda:calls.append('mutation'))
  self.assertEqual(calls,[]);self.assertEqual(obj.items,[])
 def test_pending_custody_precedes_mutation_and_failure_remains_unresolved(self):
  obj=self.bare();obj.absent=lambda *args:True
  def fail():
   self.assertEqual(obj.items,[{'kind':'bucket','name':'unique-owned','state':'pending'}]);raise RuntimeError('response lost')
  with self.assertRaises(RuntimeError):obj.create('bucket','unique-owned',fail)
  self.assertEqual(obj.items[0]['state'],'pending')
 def test_owned_principal_secret_uses_stdin_and_rejects_injected_lines(self):
  self.assertEqual(m.user_add_input('unit-principal','unit-secret'),b'unit-principal\nunit-secret\n')
  for value in ('bad\nline','bad\rline'):
   with self.assertRaises(ValueError):m.user_add_input('unit',value)
 def test_measured_successful_empty_inventory_proves_absence(self):
  for kind in ('bucket','user'):
   obj=self.bare();obj.mc=lambda *a,**k:subprocess.CompletedProcess(a,0,b'',b'')
   self.assertTrue(obj.absent(kind,'owned-fixture'),'successful empty inventory was treated as presence')

class InventoryParsingTests(unittest.TestCase):
 def bucket(self,name):return {'status':'success','type':'folder','lastModified':'2026-10-26T10:00:00Z','size':0,'key':name+'/','etag':''}
 def user(self,name):return {'status':'success','accessKey':name,'userStatus':'enabled'}
 def policy(self,name):return {'status':'success','policy':name,'policyInfo':{'PolicyName':'','Policy':None},'isGroup':False}
 def parse(self,kind,raw,code=0,stderr=b''):return m.parse_inventory(kind,code,raw,stderr)
 def test_valid_single_pretty_json_and_json_stream(self):
  for kind,record in [('bucket',self.bucket),('user',self.user),('policy',self.policy)]:
   self.assertEqual(self.parse(kind,json.dumps(record('owned-fixture'),indent=2).encode()),{'owned-fixture'})
   raw=b'\n'.join(json.dumps(record(n)).encode() for n in ('owned-fixture','other-fixture'))+b'\n'
   self.assertEqual(self.parse(kind,raw),{'owned-fixture','other-fixture'})
 def test_empty_exit0_is_measured_valid_for_all_three_list_contracts(self):
  for kind in ('bucket','user','policy'):self.assertEqual(self.parse(kind,b''),set())
 def test_exact_name_present_and_absent_use_inventory_not_error(self):
  calls=[];obj=OperatorInputsTests().bare()
  def mc(*args,**kw):calls.append(args);return subprocess.CompletedProcess(args,0,json.dumps(self.bucket('owned-fixture')).encode(),b'')
  obj.mc=mc
  self.assertFalse(obj.absent('bucket','owned-fixture'));self.assertTrue(obj.absent('bucket','other-fixture'))
  self.assertTrue(all(a==('ls','fixture') for a in calls))
 def test_failed_exit_stderr_or_error_record_is_unknown(self):
  good=json.dumps(self.bucket('owned-fixture')).encode()
  for raw,code,stderr in [(good,1,b''),(b'',1,b''),(good,0,b'warning'),(b'{"status":"error","error":{}}',0,b'')]:
   with self.assertRaises(RuntimeError):self.parse('bucket',raw,code,stderr)
 def test_truncation_trailing_garbage_nonrecords_arrays_and_invalid_utf8(self):
  good=json.dumps(self.bucket('owned-fixture')).encode()
  for raw in [good[:-1],good+b'garbage',b'[]',b'null',b'"record"',b'\xff',b'NaN',good+good,good+b'\x0b',b'   ']:
   with self.assertRaises(RuntimeError):self.parse('bucket',raw)
 def test_duplicate_records_and_duplicate_json_keys_rejected(self):
  good=json.dumps(self.bucket('owned-fixture')).encode()
  for raw in [good+b'\n'+good,b'{"status":"error","status":"success"}',b'{"status":"success","policy":"a","policy":"b"}']:
   with self.assertRaises(RuntimeError):self.parse('bucket',raw)
 def test_wrong_schema_conflicts_and_noncanonical_names_rejected(self):
  records=[('bucket',{**self.bucket('owned-fixture'),'type':'file'}),('bucket',{**self.bucket('owned-fixture'),'key':'owned-fixture//'}),('bucket',{**self.bucket('owned-fixture'),'size':True}),('user',{**self.user('owned-fixture'),'userStatus':'maybe'}),('user',{**self.user('owned-fixture'),'secretKey':'must-not-retain'}),('policy',{**self.policy('owned-fixture'),'isGroup':True}),('policy',{**self.policy('owned-fixture'),'policyInfo':{'PolicyName':'other','Policy':None}})]
  for kind,value in records:
   with self.assertRaises(RuntimeError):self.parse(kind,json.dumps(value).encode())
 def test_byte_and_record_bounds_rejected(self):
  with self.assertRaises(RuntimeError):self.parse('user',b' '*(262144+1))
  raw=b'\n'.join(json.dumps(self.user('fixture-'+str(n))).encode() for n in range(2049))
  with self.assertRaises(RuntimeError):self.parse('user',raw)
 def test_safe_error_code_never_records_arbitrary_text_or_structures(self):
  for value in ('credential-or-url',[],{'secret':'unit'}):
   raw=json.dumps({'status':'error','error':{'Code':value}}).encode()
   self.assertEqual(m.safe_error_code(raw,b''),'unrecognized')
  self.assertEqual(m.safe_error_code(b'{"status":"error","error":{"Code":"AccessDenied"}}',b''),'AccessDenied')
 def test_unknown_inventory_never_starts_mutation_or_adds_custody(self):
  obj=OperatorInputsTests().bare();obj.receipt={};obj.mc=lambda *a,**k:subprocess.CompletedProcess(a,0,b'{"status":"error"}',b'')
  calls=[]
  with self.assertRaises(RuntimeError):obj.create('bucket','owned-fixture',lambda:calls.append('mutation'))
  self.assertEqual(calls,[]);self.assertEqual(obj.items,[])
 def test_stream_byte_bound_kills_and_returns_sanitized_receipt_only(self):
  with self.assertRaises(m.InventoryError) as result:
   m.bounded_mc([sys.executable,'-c','import os; os.write(1,b"x"*400000)'],os.environ,None,2)
  self.assertEqual(result.exception.category,'byte-bound')
  self.assertTrue(hasattr(result.exception,'safe_receipt'))
  self.assertFalse(hasattr(result.exception,'output'));self.assertFalse(hasattr(result.exception,'stderr'))
 def test_timeout_and_launch_failure_keep_only_sanitized_receipts(self):
  with self.assertRaises(m.InventoryError) as result:
   m.bounded_mc([sys.executable,'-c','import time; time.sleep(2)'],os.environ,None,.03)
  self.assertEqual(result.exception.category,'timeout');self.assertIn('returnCode',result.exception.safe_receipt)
  with patch.object(m.subprocess,'Popen',side_effect=PermissionError('private text')),self.assertRaises(m.InventoryError) as result:
   m.bounded_mc(['unit-tool'],{},None,1)
  self.assertEqual(result.exception.category,'process-io');self.assertNotIn('private text',str(result.exception))
 def test_command_receipt_contains_only_fixed_stage_code_counts_and_hashes(self):
  obj=OperatorInputsTests().bare();obj.receipt={};obj.tools={'mc':{'path':'unit-mc'}};obj.directory=pathlib.Path('/unit-private');obj.environment={};obj.deadline=m.time.monotonic()+1
  secret=b'{"status":"error","error":{"Code":"AccessDenied","message":"private-body"}}'
  with patch.object(m,'bounded_mc',return_value=subprocess.CompletedProcess([],1,secret,b'')):
   with self.assertRaises(m.InventoryError):obj.inventory('user')
  receipt=obj.receipt['commandReceipts'][0]
  self.assertEqual(receipt['stage'],'user-list');self.assertEqual(receipt['safeErrorCode'],'AccessDenied');self.assertEqual(receipt['category'],'failed-exit')
  self.assertNotIn('private-body',json.dumps(receipt));self.assertNotIn('argv',receipt)
 def test_independent_known_cleanup_continues_after_one_failure(self):
  obj=OperatorInputsTests().bare();obj.bucket='owned-fixture';obj.receipt={};obj.directory=pathlib.Path('/not-an-actual-fixture')
  obj.items=[{'kind':'bucket','name':'owned-fixture','state':'created'},{'kind':'user','name':'owned-user','state':'created'},{'kind':'policy','name':'owned-policy','state':'created'}]
  obj.absent=lambda *a:True;calls=[]
  def mc(*a,**kw):
   calls.append(a)
   if a[:3]==('admin','user','remove'):raise RuntimeError('fixed unit failure')
   return subprocess.CompletedProcess(a,0,b'',b'')
  obj.mc=mc
  with patch.object(m.shutil,'rmtree') as erase,self.assertRaises(RuntimeError):obj.cleanup(True)
  self.assertIn(('admin','policy','remove','fixture','owned-policy'),calls);self.assertIn(('rb','fixture/owned-fixture'),calls)
  erase.assert_not_called();self.assertEqual(obj.items[1]['state'],'created')
 def test_removal_requires_successful_full_inventory_absence_after_each_action(self):
  obj=OperatorInputsTests().bare();obj.bucket='owned-fixture';obj.receipt={};obj.directory=pathlib.Path('/unit-private');obj.baseline_buckets=[];obj.before_service={'port':1};obj.service_state=lambda _:obj.before_service
  obj.items=[{'kind':kind,'name':'owned-fixture','state':'created'} for kind in ('bucket','user','policy')];calls=[]
  def mc(*a,**kw):
   calls.append(a);return subprocess.CompletedProcess(a,0,b'',b'')
  obj.mc=mc
  with patch.object(m.shutil,'rmtree'):obj.cleanup(True)
  for removal,inventory in [(('admin','user','remove','fixture','owned-fixture'),('admin','user','list','fixture')),(('admin','policy','remove','fixture','owned-fixture'),('admin','policy','list','fixture')),(('rb','fixture/owned-fixture'),('ls','fixture'))]:
   self.assertEqual(calls[calls.index(removal)+1],inventory)
  self.assertTrue(obj.closed);self.assertTrue(all(item['state']=='closed' for item in obj.items))
if __name__=='__main__':unittest.main()
