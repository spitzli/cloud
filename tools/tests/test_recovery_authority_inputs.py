"""Finite new authority candidate binding; pure files only, no Docker/provider actions."""
import hashlib,importlib.util,json,pathlib,tempfile,unittest
from unittest.mock import patch
P=pathlib.Path
spec=importlib.util.spec_from_file_location('authority_runner',P(__file__).with_name('recovery-admission-component.py'));r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
sha=lambda p:hashlib.sha256(P(p).read_bytes()).hexdigest()
class AuthorityInputsTests(unittest.TestCase):
 def setUp(self):
  self.tmp=tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup);self.root=P(self.tmp.name)
  self.pin=self.root/r.AUTHORITY_PIN_PATH;self.pin.parent.mkdir(parents=True)
  self.base=self.root/r.BASE_PIN_PATH;self.base.parent.mkdir(parents=True)
  self.retained=self.root/r.RETAINED_AUTHORITY_PIN_PATH;self.retained.parent.mkdir(parents=True)
  self.runtime=self.root/r.RETAINED_RUNTIME_PIN_PATH;self.runtime.parent.mkdir(parents=True)
  self.standalone=self.root/r.RETAINED_CLI_PIN_PATH;self.standalone.parent.mkdir(parents=True)
  helper=self.runtime.parent/'webdock-storage';helper.write_bytes(b'old verified helper');helper.chmod(0o500)
  self.entries={'webdock-storage':{'path':str(helper),'copiedStrippedSHA256':sha(helper),'bytes':helper.stat().st_size,'mode':'0500'}}
  for name in ('bootstrap-test','webdock-agent','webdock-recovery','webdock-recovery-gateway'):
   p=(self.runtime.parent if name=='webdock-agent' else self.standalone.parent if name in ('webdock-recovery','webdock-recovery-gateway') else self.pin.parent)/name;p.write_bytes(name.encode());p.chmod(0o500);self.entries[name]={'path':str(p),'copiedStrippedSHA256':sha(p),'bytes':p.stat().st_size,'mode':'0500'}
  archive=self.base.parent/'public-source.tar.gz';archive.write_bytes(b'original public source')
  self.base.write_text(json.dumps({'binaries':{'webdock-storage':self.entries['webdock-storage']},'publicSourceArchiveSHA256':sha(archive)}))
  source=self.pin.parent/'public-source.tar.gz';source.write_bytes(b'new current source')
  compilation=self.pin.parent/'compilation.json';compilation.write_text(json.dumps({'archiveSHA256':sha(source),'rootLockSHA256':'a'*64,'migrations':{},'inputs':{},'runtimeExecutables':{'bootstrap-test':self.entries['bootstrap-test']}}))
  tools={}
  for name in ('mc','age-keygen','ssh-keygen'):
   p=self.retained.parent/'operator-tools'/name;p.parent.mkdir(exist_ok=True);p.write_bytes(b'fixed tool '+name.encode());p.chmod(0o500);tools[name]={'path':str(p),'sha256':sha(p),'bytes':p.stat().st_size,'mode':'0500'}
  retained_source=self.retained.parent/'public-source.tar.gz';retained_source.write_bytes(b'original430 source')
  retained_compilation=self.retained.parent/'compilation.json';retained_compilation.write_bytes(b'original430 compilation')
  self.retained.write_text(json.dumps({'binaries':{'webdock-agent':self.entries['webdock-agent']},'operatorTools':tools,'compilationSHA256':sha(retained_compilation),'sourceArchiveSHA256':sha(retained_source)}))
  runtime_source=self.runtime.parent/'public-source.tar.gz';runtime_source.write_bytes(b'original6f1 runtime source')
  runtime_compilation=self.runtime.parent/'compilation.json';runtime_compilation.write_text(json.dumps({'archiveSHA256':sha(runtime_source),'runtimeExecutables':self.entries}))
  self.runtime.write_text(json.dumps({'binaries':self.entries,'sourceArchiveSHA256':sha(runtime_source),'compilationSHA256':sha(runtime_compilation)}))
  standalone_source=self.standalone.parent/'public-source.tar.gz';standalone_source.write_bytes(b'original standalone source')
  standalone_compilation=self.standalone.parent/'compilation.json';standalone_compilation.write_text(json.dumps({'archiveSHA256':sha(standalone_source),'runtimeExecutables':self.entries}))
  self.standalone.write_text(json.dumps({'binaries':self.entries,'sourceArchiveSHA256':sha(standalone_source),'compilationSHA256':sha(standalone_compilation)}))
  self.manifest={'version':'source-ntfy-byte-path-v1','retainedCLIPin':{'path':str(self.standalone),'sha256':sha(self.standalone)},'retainedRuntimePin':{'path':str(self.runtime),'sha256':sha(self.runtime)},'retainedAuthorityPin':{'path':str(self.retained),'sha256':sha(self.retained)},'originalBasePin':{'path':str(self.base),'sha256':sha(self.base)},'binaries':self.entries,'operatorTools':tools,'compilationSHA256':sha(compilation),'sourceArchiveSHA256':sha(source),'rootLockSHA256':'a'*64,'migrations':{}}
  self.overlay=self.root/'.runtime-test/overlay/overlay.json';self.overlay.parent.mkdir(parents=True)
  files={}
  for rel in r.AUTHORITY_OVERLAY_INPUTS:
   raw=(P(__file__).parents[2]/rel).read_bytes() if rel==r.CUSTODY_INPUT else b'finite public fixture'
   for path in (self.root/rel,self.overlay.parent/'files'/rel):path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(raw)
   files[rel]=hashlib.sha256(raw).hexdigest()
  self.overlay_data={'purpose':'recovery-source-component-input-overlay-v1','baseArtifactPinSHA256':'','files':files}
  self.pin.write_text(json.dumps(self.manifest));self.write_overlay()
  self.stack=[]
  for name,value in [('BASE_PIN_SHA256',sha(self.base)),('RETAINED_AUTHORITY_PIN_SHA256',sha(self.retained)),('AUTHORITY_PIN_SHA256',sha(self.pin)),('RETAINED_RUNTIME_PIN_SHA256',sha(self.runtime)),('RETAINED_CLI_PIN_SHA256',sha(self.standalone)),('AUTHORITY_ELFS',{n:e['copiedStrippedSHA256'] for n,e in self.entries.items()})]:
   ctx=patch.object(r,name,value);ctx.start();self.addCleanup(ctx.stop)
 def write_overlay(self):
  self.overlay_data['baseArtifactPinSHA256']=sha(self.pin);self.overlay.write_text(json.dumps(self.overlay_data))
 def select(self,**kw):return r.select_whole_inputs(root=self.root,artifact_pin=self.pin,driver='docker',input_overlay=self.overlay,input_overlay_sha256=sha(self.overlay),**kw)
 def test_exact_five_binaries_and_operator_tool_set(self):
  selected=self.select();self.assertEqual(set(selected['binaries']),{'bootstrap-test','webdock-agent','webdock-storage','webdock-recovery','webdock-recovery-gateway'});self.assertTrue(selected['authorityOnly'])
 def test_wrong_parent_pin_denied(self):
  self.base.write_text('{}')
  with self.assertRaises(ValueError):self.select()
 def test_changed_current_agent_or_mode_denied(self):
  agent=P(self.entries['webdock-agent']['path']);agent.chmod(0o700)
  with self.assertRaises(ValueError):self.select()
 def test_changed_operator_utility_denied(self):
  tool=self.retained.parent/'operator-tools/mc';tool.chmod(0o700);tool.write_bytes(b'wrong utility');tool.chmod(0o500)
  with self.assertRaises(ValueError):self.select()
 def test_missing_operator_source_denied_before_acquisition(self):
  (self.overlay.parent/'files'/r.AUTHORITY_OPERATOR_INPUT).unlink()
  with self.assertRaises(FileNotFoundError):self.select()
 def test_foreign_test_override_denied(self):
  with self.assertRaises(ValueError):self.select(test=self.root/'unapproved-test')
 def test_retained_agent_parent_and_archive_revalidated(self):
  (self.retained.parent/'public-source.tar.gz').write_bytes(b'wrong archive')
  with self.assertRaises(ValueError):self.select()
 def test_foreign_agent_location_cannot_be_substituted(self):
  self.entries['webdock-agent']['path']=str(self.retained.parent/'webdock-agent')
  self.pin.write_text(json.dumps(self.manifest));self.write_overlay()
  with patch.object(r,'AUTHORITY_PIN_SHA256',sha(self.pin)),self.assertRaises(ValueError):self.select()
 def test_retained_pin_itself_is_rehashed(self):
  self.retained.write_text('{}')
  with self.assertRaises(ValueError):self.select()
 def test_old_helper_location_is_not_current_compiler_provenance(self):
  self.entries['webdock-storage']['path']=str(self.base.parent/'webdock-storage')
  self.pin.write_text(json.dumps(self.manifest));self.write_overlay()
  with patch.object(r,'AUTHORITY_PIN_SHA256',sha(self.pin)),self.assertRaises(ValueError):self.select()
 def test_test_compilation_must_bind_exact_current_test(self):
  compilation=self.pin.parent/'compilation.json';record=json.loads(compilation.read_text());record['runtimeExecutables'].pop('bootstrap-test');compilation.write_text(json.dumps(record))
  self.manifest['compilationSHA256']=sha(compilation);self.pin.write_text(json.dumps(self.manifest));self.write_overlay()
  with patch.object(r,'AUTHORITY_PIN_SHA256',sha(self.pin)),self.assertRaises(ValueError):self.select()
 def test_retained_runtime_source_and_pin_are_verified(self):
  (self.runtime.parent/'public-source.tar.gz').write_bytes(b'foreign runtime archive')
  with self.assertRaises(ValueError):self.select()
if __name__=='__main__':unittest.main()

class MatchedMailInputsTests(unittest.TestCase):
 setUp=AuthorityInputsTests.setUp
 write_overlay=AuthorityInputsTests.write_overlay
 def prepare_mail(self):
  self.mail_parent=self.root/r.RETAINED_MATCHED_MAIL_PIN_PATH;self.mail_parent.parent.mkdir(parents=True)
  for name in ('webdock-migrate','webdock-mail'):
   p=self.mail_parent.parent/name;p.write_bytes(('retained '+name).encode());p.chmod(0o500)
   self.entries[name]={'path':str(p),'copiedStrippedSHA256':sha(p),'bytes':p.stat().st_size,'mode':'0500'}
  old_test=self.mail_parent.parent/'bootstrap-test';old_test.write_bytes(b'original e331 test');old_test.chmod(0o500)
  parent_entries={**self.entries,'bootstrap-test':{'path':str(old_test),'copiedStrippedSHA256':sha(old_test),'bytes':old_test.stat().st_size,'mode':'0500'}}
  archive=self.mail_parent.parent/'public-source.tar.gz';archive.write_bytes(b'retained e331 public sources')
  compilation=self.mail_parent.parent/'compilation.json';compilation.write_text(json.dumps({'archiveSHA256':sha(archive),'runtimeExecutables':{n:parent_entries[n] for n in ('bootstrap-test','webdock-migrate','webdock-mail')}}))
  self.parent_manifest={'version':'source-matched-mail-v1','fixtureMode':'matched-mail-v1','binaries':parent_entries,'sourceArchiveSHA256':sha(archive),'compilationSHA256':sha(compilation)}
  self.mail_parent.write_text(json.dumps(self.parent_manifest))
  self.manifest['version']='source-matched-mail-v1';self.manifest['fixtureMode']='matched-mail-v1';self.manifest['retainedMatchedMailPin']={'path':str(self.mail_parent),'sha256':sha(self.mail_parent)}
  probe=self.root/'proto/compiled-test.proto';probe.parent.mkdir(parents=True,exist_ok=True);probe.write_bytes(b'actual fixture public source')
  current=self.pin.parent/'compilation.json';value=json.loads(current.read_text());value['inputs']={'proto/compiled-test.proto':sha(probe)};self.later_live_drift={}
  for name in r.MAIL_ALLOWED_LIVE_DRIFT:
   source=self.root/name;source.parent.mkdir(parents=True,exist_ok=True);source.write_bytes(b'unit compiled '+name.encode());value['inputs'][name]=sha(source);self.later_live_drift[name]={'compiled':sha(source),'live':hashlib.sha256(b'unit later '+name.encode()).hexdigest()}
  self.external=self.root/'external-maylily';self.external.mkdir();self.external_files={}
  for name in r.MAIL_EXTERNAL_FILES:
   path=self.external/name;path.parent.mkdir(exist_ok=True);path.write_bytes(('public '+name).encode());self.external_files[name]={'sha256':sha(path),'bytes':path.stat().st_size}
  for name in ('external-maylily-source.tar.gz','external-maylily-manifest.diff','external-compile-freshness.json'):(self.pin.parent/name).write_bytes(b'finite frozen public receipt')
  receipt=self.pin.parent/'external-maylily-receipt.json';receipt.write_text(json.dumps({'root':str(self.external),'files':self.external_files,'archiveSHA256':sha(self.pin.parent/'external-maylily-source.tar.gz'),'manifestDiffSHA256':sha(self.pin.parent/'external-maylily-manifest.diff')}))
  self.external_record={'root':str(self.external),'files':self.external_files,'archiveSHA256':sha(self.pin.parent/'external-maylily-source.tar.gz'),'receiptSHA256':sha(receipt)}
  self.manifest['externalMaylily']=self.external_record;self.manifest['externalCompileFreshnessSHA256']=sha(self.pin.parent/'external-compile-freshness.json');value['externalMaylily']=self.external_record
  self.manifest['laterLiveDrift']=self.later_live_drift
  self.test_pin=self.root/r.RETAINED_TEST_PIN_PATH;self.test_pin.parent.mkdir(parents=True)
  test=self.test_pin.parent/'bootstrap-test';old_path=P(self.entries['bootstrap-test']['path']);test.write_bytes(old_path.read_bytes());test.chmod(0o500);self.entries['bootstrap-test']={**self.entries['bootstrap-test'],'path':str(test)};value['runtimeExecutables']={'bootstrap-test':self.entries['bootstrap-test']}
  current.write_text(json.dumps(value));self.test_compilation=self.test_pin.parent/'compilation.json';self.test_compilation.write_bytes(current.read_bytes());self.manifest['compilationSHA256']=sha(self.test_compilation)
  for name in ('public-source.tar.gz','external-maylily-source.tar.gz','external-maylily-manifest.diff','external-maylily-receipt.json','external-compile-freshness.json'):(self.test_pin.parent/name).write_bytes((self.pin.parent/name).read_bytes())
  test_record={key:self.manifest[key] for key in ('compilationSHA256','sourceArchiveSHA256','rootLockSHA256','migrations','externalMaylily','externalCompileFreshnessSHA256')};test_record['binaries']={'bootstrap-test':self.entries['bootstrap-test']};self.test_pin.write_text(json.dumps(test_record));self.manifest['retainedTestPin']={'path':str(self.test_pin),'sha256':sha(self.test_pin)}
  worker=self.pin.parent/'webdock-mail';worker.write_bytes(b'current standalone diagnostic worker');worker.chmod(0o500);self.entries['webdock-mail']={'path':str(worker),'copiedStrippedSHA256':sha(worker),'bytes':worker.stat().st_size,'mode':'0500'}
  private_lock=self.root/'plugins/mail/Cargo.lock';private_lock.parent.mkdir(parents=True);private_lock.write_bytes(b'unit private worker lock');self.worker_lock_sha=sha(private_lock);worker_source=self.root/'plugins/mail/src/main.rs';worker_source.parent.mkdir();worker_source.write_bytes(b'unit fixed enum only')
  worker_archive=self.pin.parent/'worker-source.tar.gz';worker_archive.write_bytes(b'unit private worker sources');worker_comp=self.pin.parent/'worker-compilation.json';worker_comp.write_text(json.dumps({'archiveSHA256':sha(worker_archive),'standaloneLockSHA256':self.worker_lock_sha,'runtimeExecutables':{'webdock-mail':self.entries['webdock-mail']},'idsConsumed':False,'maylilyConsumed':False,'externalConsumedFiles':{},'inputs':{'plugins/mail/Cargo.lock':sha(private_lock),'plugins/mail/src/main.rs':sha(worker_source)}}));self.manifest['diagnosticWorker']={'sourceArchiveSHA256':sha(worker_archive),'compilationSHA256':sha(worker_comp),'standaloneLockSHA256':self.worker_lock_sha}
  self.dns=self.root/r.MAIL_DNS_PROBE_DIR;self.dns.mkdir(parents=True)
  tool=self.dns/'dns-constructor-test';tool.write_bytes(b'unit build-only constructor probe');tool.chmod(0o500)
  (self.dns/'public-source.tar.gz').write_bytes(b'unit public test-source archive')
  lib=self.root/'plugins/mail/src/lib.rs';lib.write_bytes(b'unit cfg test only probe')
  (self.dns/'compilation.json').write_text(json.dumps({'privateLockSHA256':self.worker_lock_sha,'inputs':{'plugins/mail/src/lib.rs':sha(lib)},'runtimeWorkerSHA256':self.entries['webdock-mail']['copiedStrippedSHA256']}))
  recipe=("MAIL_DNS_PROBE_SHA256="+repr(sha(tool))+"\n").encode()
  for dest in (self.root/r.GATEWAY_RECIPE,self.overlay.parent/'files'/r.GATEWAY_RECIPE):dest.write_bytes(recipe)
  self.overlay_data['files'][r.GATEWAY_RECIPE]=hashlib.sha256(recipe).hexdigest()
  self.agent_pin=self.root/r.CURRENT_AGENT_PIN_PATH;self.agent_pin.parent.mkdir(parents=True)
  current_agent=self.agent_pin.parent/'webdock-agent';current_agent.write_bytes(b'unit current Agent');current_agent.chmod(0o500)
  self.entries['webdock-agent']={'path':str(current_agent),'copiedStrippedSHA256':sha(current_agent),'bytes':current_agent.stat().st_size,'mode':'0500'}
  agent_archive=self.agent_pin.parent/'agent-source-v2.tar.gz';agent_archive.write_bytes(b'unit actual Agent source')
  agent_comp=self.agent_pin.parent/'agent-compilation-v2.json';agent_comp.write_text(json.dumps({'inputs':{},'rootLockSHA256':'a'*64,'runtimeExecutables':{'webdock-agent':self.entries['webdock-agent']},'archiveSHA256':sha(agent_archive),'actualCargoArtifact':{'target':{'name':'webdock-agent','kind':['bin']},'profile':{'debuginfo':0},'features':['docker','kubernetes','local-recovery-fixture']}}))
  self.agent_pin.write_text(json.dumps({'version':'source-matched-agent-v2','binaries':{'webdock-agent':self.entries['webdock-agent']},'compilationSHA256':sha(agent_comp),'sourceArchiveSHA256':sha(agent_archive),'rootLockSHA256':'a'*64,'compilerInputs':{}}));self.manifest['currentAgentPin']={'path':str(self.agent_pin),'sha256':sha(self.agent_pin)}
  self.gateway_pin=self.root/r.CURRENT_GATEWAY_PIN_PATH;self.gateway_pin.parent.mkdir(parents=True,exist_ok=True)
  gateway=self.gateway_pin.parent/'webdock-recovery-gateway';gateway.write_bytes(b'unit current gateway');gateway.chmod(0o500)
  self.entries['webdock-recovery-gateway']={'path':str(gateway),'copiedStrippedSHA256':sha(gateway),'bytes':gateway.stat().st_size,'mode':'0500'}
  archive=self.gateway_pin.parent/'gateway-source.tar.gz';archive.write_bytes(b'unit gateway source')
  compilation=self.gateway_pin.parent/'gateway-compilation.json';compilation.write_text(json.dumps({'inputs':{},'rootLockSHA256':'a'*64,'runtimeExecutables':{'webdock-recovery-gateway':self.entries['webdock-recovery-gateway']},'archiveSHA256':sha(archive),'actualCargoArtifact':{'target':{'name':'webdock-recovery-gateway','kind':['bin']},'profile':{'debuginfo':0,'test':False},'features':['default','docker','kubernetes','local-recovery-fixture']}}))
  self.gateway_pin.write_text(json.dumps({'version':'source-matched-gateway-v1','binaries':{'webdock-recovery-gateway':self.entries['webdock-recovery-gateway']},'compilationSHA256':sha(compilation),'sourceArchiveSHA256':sha(archive),'rootLockSHA256':'a'*64,'compilerInputs':{}}));self.manifest['currentGatewayPin']={'path':str(self.gateway_pin),'sha256':sha(self.gateway_pin)}
  self.pin.write_text(json.dumps(self.manifest));self.write_overlay()
 def select_mail(self,driver='kubernetes'):
  with patch.object(r,'AUTHORITY_PIN_SHA256',sha(self.pin)),patch.object(r,'AUTHORITY_ELFS',{n:e['copiedStrippedSHA256'] for n,e in self.entries.items()}),patch.object(r,'RETAINED_MATCHED_MAIL_PIN_SHA256',sha(self.mail_parent)),patch.object(r,'MAIL_ALLOWED_LIVE_DRIFT',self.later_live_drift),patch.object(r,'MAIL_EXTERNAL_ROOT',self.external),patch.object(r,'MAIL_EXTERNAL_FILES',self.external_files),patch.object(r,'RETAINED_TEST_PIN_SHA256',sha(self.test_pin)),patch.object(r,'DIAGNOSTIC_MAIL_LOCK_SHA256',self.worker_lock_sha),patch.object(r,'MAIL_DNS_COMPILATION_SHA256',sha(self.dns/'compilation.json')),patch.object(r,'MAIL_DNS_ARCHIVE_SHA256',sha(self.dns/'public-source.tar.gz')),patch.object(r,'MAIL_DNS_PROBE_SHA256',sha(self.dns/'dns-constructor-test')),patch.object(r,'MAIL_DNS_PROBE_BYTES',(self.dns/'dns-constructor-test').stat().st_size),patch.object(r,'MAIL_DNS_TEST_LIB_SHA256',sha(self.root/'plugins/mail/src/lib.rs')),patch.object(r,'CURRENT_AGENT_PIN_SHA256',sha(self.agent_pin)),patch.object(r,'CURRENT_GATEWAY_PIN_SHA256',sha(self.gateway_pin)):
   return r.select_whole_inputs(root=self.root,artifact_pin=self.pin,driver=driver,input_overlay=self.overlay,input_overlay_sha256=sha(self.overlay))
 def test_exact_seven_with_one_current_and_six_retained_compilers_and_kube_driver(self):
  self.prepare_mail();selected=self.select_mail();self.assertEqual(len(selected['binaries']),7);self.assertEqual(selected['driver'],'kubernetes')
  with self.assertRaises(ValueError):self.select_mail('docker')
 def test_build_only_probe_is_separate_and_unknown_source_change_denies(self):
  self.prepare_mail();selected=self.select_mail();self.assertEqual(len(selected['binaries']),7);self.assertNotIn('dns-constructor-test',selected['binaries']);self.assertEqual(selected['mailDnsBuildOnly']['path'],self.dns/'dns-constructor-test')
  (self.root/'plugins/mail/src/lib.rs').write_bytes(b'unknown production or test change')
  with self.assertRaises(ValueError):self.select_mail()

 def test_worker_library_must_match_its_current_compiler_with_no_legacy_cfg_drift_exception(self):
  self.prepare_mail();self.select_mail();comp=self.pin.parent/'worker-compilation.json';value=json.loads(comp.read_text());value['inputs']['plugins/mail/src/lib.rs']=hashlib.sha256(b'unit obsolete production library').hexdigest();comp.write_text(json.dumps(value));self.manifest['diagnosticWorker']['compilationSHA256']=sha(comp);self.pin.write_text(json.dumps(self.manifest));self.write_overlay()
  with self.assertRaises(ValueError):self.select_mail()

 def test_live_target_or_foreign_worker_location_cannot_replace_pinned_current_binary(self):
  self.prepare_mail();self.entries['webdock-mail']['path']=str(self.root/'target/debug/webdock-mail');self.pin.write_text(json.dumps(self.manifest));self.write_overlay()
  with self.assertRaises(ValueError):self.select_mail()
 def test_missing_migrator_compilation_binding_fails_before_acquisition(self):
  self.prepare_mail();compilation=self.mail_parent.parent/'compilation.json';value=json.loads(compilation.read_text());value['runtimeExecutables'].pop('webdock-migrate');compilation.write_text(json.dumps(value));self.parent_manifest['compilationSHA256']=sha(compilation);self.mail_parent.write_text(json.dumps(self.parent_manifest));self.manifest['retainedMatchedMailPin']['sha256']=sha(self.mail_parent);self.pin.write_text(json.dumps(self.manifest));self.write_overlay()
  with self.assertRaises(ValueError):self.select_mail()

 def test_additional_compiled_protocol_drift_denies_before_acquisition(self):
  self.prepare_mail();(self.root/'proto/compiled-test.proto').write_bytes(b'unapproved drift')
  with self.assertRaises(ValueError):self.select_mail()

 def test_only_exact_coordinated_later_hashes_are_allowed(self):
  self.prepare_mail()
  for name in self.later_live_drift:(self.root/name).write_bytes(b'unit later '+name.encode())
  selected=self.select_mail();self.assertEqual(set(selected['liveCompiledSourceDrift']),set(self.later_live_drift))
  (self.root/'crates/ids/src/lib.rs').write_bytes(b'unapproved third identity version')
  with self.assertRaises(ValueError):self.select_mail()
 def test_root_manifest_accepts_only_its_exact_declared_exclusion_pair(self):
  self.prepare_mail();source=self.root/'Cargo.toml';self.select_mail()
  source.write_bytes(b'unit later Cargo.toml');self.select_mail()
  source.write_bytes(b'unknown workspace graph edit')
  with self.assertRaises(ValueError):self.select_mail()
 def test_registered_migration_alias_must_resolve_to_captured_bytes(self):
  self.prepare_mail();target=self.root/'crates/hosting/migrations/owned.sql';target.parent.mkdir(parents=True);target.write_bytes(b'public SQL alias unit')
  alias=self.root/'crates/persistence/migrations/0008_owned.sql';alias.parent.mkdir(parents=True);alias.symlink_to('../../hosting/migrations/owned.sql')
  compilation=self.test_compilation;value=json.loads(compilation.read_text());value['inputs'].update({str(p.relative_to(self.root)):sha(p) for p in (target,alias)});value['migrations']={str(alias.relative_to(self.root)):sha(alias)};compilation.write_text(json.dumps(value));self.manifest['migrations']=value['migrations'];self.manifest['compilationSHA256']=sha(compilation);retained=json.loads(self.test_pin.read_bytes());retained['compilationSHA256']=sha(compilation);retained['migrations']=value['migrations'];self.test_pin.write_text(json.dumps(retained));self.manifest['retainedTestPin']['sha256']=sha(self.test_pin);self.pin.write_text(json.dumps(self.manifest));self.write_overlay();self.select_mail()
  foreign=target.with_name('not-captured.sql');foreign.write_bytes(target.read_bytes());alias.unlink();alias.symlink_to('../../hosting/migrations/not-captured.sql')
  with self.assertRaises(ValueError):self.select_mail()

 def test_external_manifest_and_actual_consumed_source_drift_denied(self):
  self.prepare_mail();self.select_mail()
  for name in self.external_files:
   path=self.external/name;raw=path.read_bytes();path.write_bytes(b'unknown dependency edit')
   with self.assertRaises(ValueError):self.select_mail()
   path.write_bytes(raw)
 def test_external_source_symlink_and_foreign_root_denied(self):
  self.prepare_mail();path=self.external/'src/lib.rs';raw=path.read_bytes();path.unlink();foreign=self.external/'other';foreign.write_bytes(raw);path.symlink_to(foreign)
  with self.assertRaises(ValueError):self.select_mail()
  path.unlink();path.write_bytes(raw);self.manifest['externalMaylily']['root']=str(self.root/'foreign');self.pin.write_text(json.dumps(self.manifest));self.write_overlay()
  with self.assertRaises(ValueError):self.select_mail()
 def test_external_missing_or_foreign_capture_denied_before_effects(self):
  self.prepare_mail();source=self.test_pin.parent/'external-maylily-source.tar.gz';raw=source.read_bytes();source.unlink()
  with self.assertRaises(FileNotFoundError):self.select_mail()
  source.write_bytes(raw);self.manifest['externalMaylily']['files']['unknown.rs']={'sha256':'a'*64,'bytes':1};self.pin.write_text(json.dumps(self.manifest));self.write_overlay()
  with self.assertRaises(ValueError):self.select_mail()

 def test_diagnostic_worker_and_retained_test_cannot_fallback_or_drift(self):
  self.prepare_mail();self.select_mail()
  worker=self.pin.parent/'webdock-mail';worker.chmod(0o700)
  with self.assertRaises(ValueError):self.select_mail()
  worker.chmod(0o500);(self.root/'plugins/mail/src/main.rs').write_bytes(b'unknown source')
  with self.assertRaises(ValueError):self.select_mail()
 def test_missing_worker_archive_or_foreign_retained_test_pin_denied(self):
  self.prepare_mail();archive=self.pin.parent/'worker-source.tar.gz';raw=archive.read_bytes();archive.unlink()
  with self.assertRaises(FileNotFoundError):self.select_mail()
  archive.write_bytes(raw);self.manifest['retainedTestPin']['path']=str(self.root/'foreign/pin.json');self.pin.write_text(json.dumps(self.manifest));self.write_overlay()
  with self.assertRaises(ValueError):self.select_mail()
