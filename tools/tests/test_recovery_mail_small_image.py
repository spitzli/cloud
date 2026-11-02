"""Finite Mail image/snapshot safety and safe ownership projections; no services."""
import importlib.util,json,os,pathlib,tempfile,types,unittest
from unittest.mock import patch
P=pathlib.Path
spec=importlib.util.spec_from_file_location('small_mail_runner',P(__file__).with_name('recovery-admission-component.py'));r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
state=types.ModuleType('safe_mail_state');exec(compile(r.MAIL_STATE_CAPTURE,'verified-mail-state','exec'),state.__dict__)
class MailSmallImageTests(unittest.TestCase):
 def test_budget_accounts_source_and_cold_snapshots_without_raising_store(self):
  images={n:{'fileBytesUpper':100,'inodeEntriesUpper':10} for n in ('postgres','mail','ce','gateway','pause')};budget=r.mail_snapshot_budget(images)
  self.assertEqual(budget['maxConcurrentRetainedSnapshots'],{'postgres':5,'mail':3,'ce':2,'gateway':1,'pause':3});self.assertEqual(budget['additionalBytesRequired'],1400);self.assertEqual(budget['additionalInodesRequired'],140);self.assertEqual(budget['storeHardBytes'],8*1024**3)
  with self.assertRaises(ValueError):r.mail_snapshot_budget({**images,'foreign':{}})
 def test_all_matching_terminal_and_active_pods_are_retained_with_exact_owner_chain(self):
  uid=lambda n:f'00000000-0000-0000-0000-{n:012d}'
  svc={'metadata':{'namespace':'owned','uid':uid(1)},'spec':{'selector':{'own':'yes'}}};dep={'metadata':{'namespace':'owned','uid':uid(2),'generation':3},'spec':{'replicas':1,'selector':{'matchLabels':{'own':'yes'}}},'status':{'observedGeneration':3,'readyReplicas':1}}
  rs={'metadata':{'namespace':'owned','uid':uid(3),'ownerReferences':[{'kind':'Deployment','uid':uid(2),'controller':True}]},'spec':{'replicas':1},'status':{'replicas':1}}
  pods=[]
  for i,phase in enumerate(('Failed','Succeeded','Running')):
   pods.append({'metadata':{'namespace':'owned','uid':uid(4+i),'labels':{'own':'yes'},'ownerReferences':[{'kind':'ReplicaSet','uid':uid(3),'controller':True}],'deletionTimestamp':'owned-static' if i==0 else None},'spec':{'env':'poison-secret'},'status':{'phase':phase,'reason':'Evicted' if phase=='Failed' else None,'message':'poison-private','containerStatuses':[{'restartCount':i,'state':{'terminated':{'reason':'Error','exitCode':1,'message':'poison'}}}]}})
  projection=state.ownership_projection(svc,pods,dep,[rs]);self.assertEqual(projection['matchingCount'],3);self.assertEqual(projection['podsWithExactReplicaSetController'],3);self.assertEqual([p['phase'] for p in projection['pods']],['Failed','Succeeded','Running']);self.assertNotIn('poison',json.dumps(projection));self.assertTrue(projection['pods'][0]['deleting'])
  rs['metadata']['ownerReferences'][0]['uid']=uid(99);self.assertEqual(state.ownership_projection(svc,pods,dep,[rs])['podsWithExactReplicaSetController'],0)
  with self.assertRaises(ValueError):state.ownership_projection(svc,pods*11,dep,[rs])
 def test_actual_precore_gate_persists_receipt_and_denies_byte_inode_or_unknown_pressure(self):
  with tempfile.TemporaryDirectory() as tmp:
   directory=P(tmp);budget=directory/'budget.json';budget.write_text(json.dumps({'additionalBytesRequired':100,'additionalInodesRequired':10}));original_path=state.Path;original_open=state.os.open
   for variant in ('valid','bytes','inodes','pressure_unknown'):
    receipt=directory/(variant+'.json');available_bytes=3*1024**3 if variant!='bytes' else 99;available_inodes=6000 if variant!='inodes' else 9
    fs={'imageStore':{'capacityBytes':8*1024**3,'availableBytes':available_bytes,'inodeCapacity':10000,'availableInodes':available_inodes}}
    node={'kind':'Node','metadata':{'name':'ownednode'},'status':{'conditions':[] if variant=='pressure_unknown' else [{'type':'DiskPressure','status':'False'}]}}
    def paths(name):return budget if str(name)=='/mail-budget.json' else original_path(name)
    def opened(name,*args,**kwargs):
     self.assertEqual(str(name),'/diagnostics/mail-state-precore.json');return original_open(receipt,*args,**kwargs)
    with patch.object(state,'filesystem_projection',return_value=fs),patch.object(state,'query',return_value=node),patch.object(state,'Path',side_effect=paths),patch.object(state.os,'open',side_effect=opened),patch.dict(os.environ,{'HOSTNAME':'ownednode'}):
     if variant=='valid':state.capture('precore')
     else:
      with self.assertRaises(ValueError):state.capture('precore')
    self.assertEqual(json.loads(receipt.read_bytes())['budgetPassed'],variant=='valid')
 def test_main_generated_script_gates_both_byte_and_inode_free_space_before_core(self):
  source=r.MAIL_STATE_CAPTURE;compile(source,'mail-state','exec')
  self.assertIn("store['availableBytes']>=budget['additionalBytesRequired']",source);self.assertIn("store['availableInodes']>=budget['additionalInodesRequired']",source);self.assertIn("store['capacityBytes']//5",source);self.assertIn("store['inodeCapacity']//10",source)
  self.assertIn('python3 -B /owned/mail-state.py precore',r.MAIL_PRECORE)
  self.assertIn('/bin/sleep',P('crates/agent/src/recovery/kubernetes.rs').read_text());self.assertIn('/bin/tar',P('crates/agent/src/recovery/kubernetes.rs').read_text())
  fixture=P('crates/control-plane/tests/support/recovery_mail_component.rs').read_text();self.assertIn('let helper_image = std::env::var("WEBDOCK_RECOVERY_MAIL_IMAGE").unwrap();',fixture)
if __name__=='__main__':unittest.main()

class WorkerStageProjectionTests(unittest.TestCase):
 def test_only_full_anchored_stage_lines_and_failure_survive_poison(self):
  raw=b'mail_startup_stage stage=args\npassword=poison-secret\nmail_startup_stage stage=config_read EXTRA=poison\nmail_startup_stage stage=unknown\nmail_plugin_start_failed\n'
  receipt=state.worker_log_projection(raw,b'private stderr poison',1,'complete')
  self.assertEqual(receipt['lines'],['mail_startup_stage stage=args','mail_plugin_start_failed']);self.assertEqual(receipt['discardedLineCount'],3);self.assertNotIn('poison',json.dumps(receipt));self.assertEqual(receipt['returnCode'],1)
 def test_oversized_or_too_many_lines_fail_closed(self):
  for raw in (b'x'*8193,b'mail_plugin_start_failed\n'*65):
   receipt=state.worker_log_projection(raw,b'',0,'complete');self.assertEqual(receipt['lines'],[]);self.assertEqual(receipt['category'],'over_limit')
 def test_fixture_flag_is_only_baked_in_owned_mail_image(self):
  recipe=r.mail_image_recipe('a'*12,'run','kind','own','wd-owned-recovery-component:'+'b'*12)
  self.assertIn('ENV WEBDOCK_RECOVERY_COMPONENT_TEST=1',recipe)

 def test_log_grammar_matches_every_static_rust_enum_variant(self):
  import re
  source=P('plugins/mail/src/main.rs').read_text();variants=re.findall(r'Self::[A-Za-z]+ => "([a-z_]+)"',source)
  self.assertEqual(set(variants),set(state.STARTUP_STAGES));self.assertEqual(len(variants),len(state.STARTUP_STAGES))
 def test_unbound_forensic_history_never_requests_logs(self):
  from unittest.mock import patch
  with patch.object(state,'worker_log_request',side_effect=AssertionError('no unverified log request')):
   self.assertEqual(state.capture_worker_logs('wd-plugin-'+'a'*24,[],{'matchingCount':2},0)['category'],'pinned_forensic_binding_unavailable')

class ReadMetadataProjectionTests(unittest.TestCase):
 def value(self):return {'expected_fixed_path':True,'absolute':True,'file_type':'regular','current_euid':65532,'file_uid':0,'file_gid':65532,'mode':384,'nlink':1,'len':1024,'error_kind':'non_io','errno':None,'metadata_ok':True,'metadata_errno':None}
 def test_exact_scalar_receipt_is_retained_but_extra_or_poison_fields_are_not(self):
  value=self.value();raw=('mail_startup_read_metadata '+json.dumps(value)+'\n').encode();result=state.worker_log_projection(raw,b'',0,'complete');self.assertEqual(len(result['lines']),1);self.assertEqual(json.loads(result['lines'][0].split(' ',1)[1]),value)
  for key,bad in [('mode',True),('mode',-1),('file_uid','poison-secret'),('file_type','poison-secret'),('error_kind','poison-secret'),('unexpected','poison-secret')]:
   changed={**value,key:bad};result=state.worker_log_projection(('mail_startup_read_metadata '+json.dumps(changed)+'\n').encode(),b'',0,'complete');self.assertEqual(result['lines'],[]);self.assertNotIn('poison',json.dumps(result))
 def test_duplicate_metadata_keys_and_unanchored_lines_are_discarded(self):
  valid=json.dumps(self.value());duplicate=valid[:-1]+',"mode":384}';raw=('prefix mail_startup_read_metadata '+valid+'\nmail_startup_read_metadata '+duplicate+'\n').encode();self.assertEqual(state.worker_log_projection(raw,b'',0,'complete')['lines'],[])
 def test_multiple_metadata_receipts_are_not_silently_combined(self):
  line=('mail_startup_read_metadata '+json.dumps(self.value())+'\n').encode();value=state.worker_log_projection(line+line,b'',0,'complete');self.assertEqual(value['lines'],[]);self.assertEqual(value['category'],'multiple_metadata')
