import importlib.util
import json
import os
from pathlib import Path
import tempfile
import subprocess
import sys
import time
import unittest

spec=importlib.util.spec_from_file_location('build_inventory',Path(__file__).resolve().parents[1]/'operations/build-object-inventory.py')
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)
BINDING='a'*64
KEY='builds/100/200/1/00000000-0000-4000-8000-000000000001'
OTHER='builds/100/200/1/00000000-0000-4000-8000-000000000002'

def ledger(state='succeeded',generation=1):
 return {'version':1,'capturedAt':1000,'builds':[{'id':'200','customerID':'100','generation':generation,'status':state}],'references':[{'buildID':'200','customerID':'100','receipt':{'backendId':'fixture','binding':BINDING,'objectKey':KEY,'sha256':'b'*64,'sizeBytes':10}}]}
def listing(*keys):return [{'status':'success','key':key,'size':10,'type':'file'} for key in keys]
def report(data,items):return module.analyze(data,items,'builds',binding=BINDING,now=1001)

class InventoryTests(unittest.TestCase):
 def test_preserves_every_reference_and_reports_only_unused_terminal_keys(self):
  result=report(ledger(),listing(KEY,OTHER));self.assertTrue(result['readOnly']);self.assertEqual([x['key'] for x in result['referenced']],[KEY]);self.assertEqual([x['key'] for x in result['candidates']],[OTHER])
 def test_active_future_and_unknown_keys_are_held(self):
  future=OTHER.replace('/1/','/2/')
  result=report(ledger('running'),listing(OTHER,future,'builds/100/999/1/00000000-0000-4000-8000-000000000003'))
  self.assertFalse(result['candidates']);self.assertEqual(len(result['held']),3)
 def test_old_generation_can_be_observed_but_local_migration_copy_is_held(self):
  data=ledger('running',2);self.assertEqual(len(report(data,listing(OTHER))['candidates']),1)
  data['references'].append({'buildID':'200','customerID':'100','receipt':None});self.assertFalse(report(data,listing(OTHER))['candidates'])
 def test_degraded_metadata_never_proposes_deletion(self):
  result=report(ledger(),[{'key':KEY,'size':11},{'key':OTHER,'size':10}]);self.assertEqual(result['status'],'degraded');self.assertFalse(result['candidates'])
  data=ledger();data['references'][0]['customerID']='999';self.assertEqual(report(data,listing(OTHER))['status'],'degraded')
 def test_foreign_binding_copy_and_unknown_strings_remain_private(self):
  data=ledger();data['references'][0]['receipt']['binding']='c'*64
  result=report(data,listing(KEY,'builds/postgres://user:password@host/secret'))
  self.assertFalse(result['candidates']);self.assertNotIn('password',json.dumps(result));self.assertEqual(result['held'][0]['reason'],'foreign_binding_reference')
 def test_cli_writes_a_private_report_and_never_overwrites(self):
  with tempfile.TemporaryDirectory() as directory:
   root=Path(directory);data=ledger();data['capturedAt']=int(time.time())
   left=root/'ledger.json';right=root/'listing.jsonl';output=root/'report.json'
   left.write_text(json.dumps(data));right.write_text(''.join(json.dumps(x)+'\n' for x in listing(KEY,OTHER)))
   for path in [left,right]:path.chmod(0o600)
   command=[sys.executable,str(Path(module.__file__)),'--ledger',str(left),'--listing',str(right),'--namespace','builds','--binding',BINDING,'--output',str(output)]
   result=subprocess.run(command,text=True,capture_output=True);self.assertEqual(result.returncode,0,result.stderr);self.assertTrue(json.loads(result.stdout)['readOnly']);self.assertEqual(output.stat().st_mode&0o777,0o600)
   before=output.read_bytes();again=subprocess.run(command,text=True,capture_output=True);self.assertNotEqual(again.returncode,0);self.assertEqual(output.read_bytes(),before)
 def test_explicit_version_metadata_suppresses_all_candidates(self):
  for field in ['versionId','versionID','version_id','isDeleteMarker']:
   rows=listing(OTHER);rows[0][field]='v1'
   result=report(ledger(),rows);self.assertEqual(result['status'],'degraded');self.assertFalse(result['candidates'])
 def test_null_receipt_requires_matching_existing_build_owner(self):
  for build,customer in [('999','100'),('200','999')]:
   data=ledger();data['references'].append({'buildID':build,'customerID':customer,'receipt':None})
   result=report(data,listing(OTHER));self.assertEqual(result['status'],'degraded');self.assertFalse(result['candidates'])
 def test_private_fifo_refuses_without_waiting_for_a_writer(self):
  with tempfile.TemporaryDirectory() as directory:
   path=Path(directory)/'fifo';os.mkfifo(path,0o600)
   script="import importlib.util,sys; s=importlib.util.spec_from_file_location('i',sys.argv[1]); m=importlib.util.module_from_spec(s);s.loader.exec_module(m);m.private_read(sys.argv[2])"
   try: result=subprocess.run([sys.executable,'-c',script,str(Path(module.__file__)),str(path)],capture_output=True,text=True,timeout=0.6)
   except subprocess.TimeoutExpired:self.fail('FIFO input blocked before its descriptor could be validated')
   self.assertNotEqual(result.returncode,0)
 def test_stale_inputs_and_public_or_linked_files_refuse(self):
  with self.assertRaises(ValueError):module.analyze(ledger(),[], 'builds',binding=BINDING,now=2000)
  with tempfile.TemporaryDirectory() as directory:
   path=Path(directory)/'input';path.write_text('{}');path.chmod(0o644)
   with self.assertRaises(ValueError):module.private_read(path)
   path.chmod(0o600);self.assertEqual(module.private_read(path),'{}')
   link=Path(directory)/'link';link.symlink_to(path)
   with self.assertRaises(OSError):module.private_read(link)

if __name__=='__main__':unittest.main()
