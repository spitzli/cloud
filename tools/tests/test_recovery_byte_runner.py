"""Pure byte-fixture input/OCI/cleanup checks; no Docker, services or provider calls."""
import hashlib,importlib.util,io,json,pathlib,shutil,subprocess,tarfile,tempfile,unittest
from unittest.mock import patch
P=pathlib.Path
spec=importlib.util.spec_from_file_location('byte_runner',P(__file__).with_name('recovery-admission-component.py'));r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
spec=importlib.util.spec_from_file_location('byte_producer',P(__file__).with_name('recovery_operator_fixture.py'));producer=importlib.util.module_from_spec(spec);spec.loader.exec_module(producer)
class Journal:
 def __init__(self):self.entries={}
 def record(self,k,v):self.entries[k]=v
class ByteRunnerTests(unittest.TestCase):
 def test_own_oci_root_is_actual_manifest_not_configuration_id(self):
  with tempfile.TemporaryDirectory() as tmp:
   base=P(tmp);source=base/'source.tar';cfg=json.dumps({'os':'linux','architecture':'amd64'}).encode();cd='sha256:'+hashlib.sha256(cfg).hexdigest();layer=b'owned public gateway';ld='sha256:'+hashlib.sha256(layer).hexdigest();manifest=json.dumps({'schemaVersion':2,'mediaType':'application/vnd.oci.image.manifest.v1+json','config':{'digest':cd},'layers':[{'digest':ld,'size':len(layer)}]}).encode();root='sha256:'+hashlib.sha256(manifest).hexdigest()
   with tarfile.open(source,'w') as archive:
    for digest,raw in [(cd,cfg),(ld,layer),(root,manifest)]:
     info=tarfile.TarInfo('blobs/sha256/'+digest[7:]);info.size=len(raw);archive.addfile(info,io.BytesIO(raw))
   emitted=[]
   class Commands:
    def call(self,*a,**kw):emitted.append(a);shutil.copyfile(source,a[a.index('--output')+1])
   journal=Journal();path,reference,bindings=r.save_owned_gateway(Commands(),journal,base,{'Id':cd,'Descriptor':{'digest':root,'mediaType':'application/vnd.oci.image.manifest.v1+json','size':len(manifest)}})
   self.assertEqual(reference,'localhost/owned/recovery-component@'+root);self.assertEqual(bindings[reference]['configurationID'],cd);self.assertEqual(journal.entries['gateway-archive']['state'],'verified');self.assertEqual(emitted[0][-1],cd)
   with tarfile.open(path) as archive:self.assertEqual(json.load(archive.extractfile('index.json'))['manifests'][0]['annotations']['io.containerd.image.name'],reference)
 def test_gateway_recipe_rejects_nonlibrary_and_bad_elf(self):
  spec=importlib.util.spec_from_file_location('gateway_recipe',P(__file__).with_name('recovery_gateway_rootfs.py'));recipe=importlib.util.module_from_spec(spec);spec.loader.exec_module(recipe)
  self.assertFalse(recipe.library(P('/etc/private-key')));self.assertFalse(recipe.library(P('/usr/lib/../../root/key')))
  with tempfile.TemporaryDirectory() as tmp:
   binary=P(tmp)/'bad';binary.write_bytes(b'not ELF'*20)
   with self.assertRaises(ValueError):recipe.interpreter(binary)
 def test_owned_object_cleanup_requires_exact_hash_before_delete(self):
  with tempfile.TemporaryDirectory() as tmp:
   value=producer.OperatorInputs.__new__(producer.OperatorInputs);value.base=P(tmp);value.directory=P(tmp);value.tools={'mc':{'path':'unused'}};value.environment={};value.bucket='owned-bucket';value.receipt={'byteCanary':{'objectKey':'authority/permission-delete-owned','sizeBytes':0,'sha256':hashlib.sha256(b'').hexdigest()}};value.persist=lambda:None;deleted=[];value.mc=lambda *a,**kw:deleted.append(a)
   with patch.object(producer,'bounded_mc',return_value=subprocess.CompletedProcess([],0,b'WRONG',b'')):
    with self.assertRaises(RuntimeError):value.cleanup_byte_objects()
   self.assertEqual(deleted,[])
   with patch.object(producer,'bounded_mc',return_value=subprocess.CompletedProcess([],0,b'',b'')):value.cleanup_byte_objects()
   self.assertEqual(deleted,[('rm','fixture/owned-bucket/authority/permission-delete-owned')])
 def test_byte_data_contract_keeps_native_limits_and_private_inputs(self):
  source=P(r.__file__).read_text();self.assertIn('dst=/gateway-image.tar,readonly',source);self.assertIn("'--tmpfs','/owned:rw,nodev,nosuid,exec,size=3g",source);self.assertTrue("limit=536870912 if kind=='gateway' else 1073741824" in source);self.assertIn('gateway_current[\'Size\']>268435456',source);self.assertIn('names+=(operator.key byte-inputs.json)',r.OPERATOR_INPUT_INSTALL)
 def test_precore_tool_contract_uses_controlled_python_and_product_cli_paths(self):
  source=P(r.__file__).read_text();self.assertIn('RUN python3 /tmp/rootfs.py',source);self.assertNotIn('RUN /usr/bin/python3 /tmp/rootfs.py',source)
  for name in ['python3','nsenter','timeout','sqlite3','openssl','age','age-keygen','ldd','install','mkfs.ext4','blkid','findmnt','losetup']:self.assertIn(repr(name),r.TOOL_AUDIT)
  self.assertIn("import ssl,sqlite3",r.TOOL_AUDIT);self.assertIn("'/usr/bin/sqlite3'",r.TOOL_AUDIT)
  recipe=P(__file__).with_name('recovery_gateway_rootfs.py').read_text();self.assertIn("shutil.which('ldd',path=fixed_path)",recipe);self.assertNotIn("['/usr/bin/ldd'",recipe)
 def test_owned_main_installs_measured_tools_and_gateway_uses_verified_main(self):
  source=P(r.__file__).read_text();self.assertIn('age openssl python3 sqlite3',source);self.assertIn('FROM {component_tag} AS closure',source);self.assertIn("verify_component(main_before)",source);self.assertIn("main_before['Id']!=component_receipt.image_id",source);self.assertNotIn('FROM webdock-rust-storage-test@{BASE} AS closure',source)
  self.assertIn('recovery-tool-packages.tsv',source);self.assertIn('executableSHA256',r.TOOL_AUDIT)
  self.assertNotIn('apt-get',P(__file__).with_name('recovery_gateway_rootfs.py').read_text())
 def producer_instance(self,base):
  value=producer.OperatorInputs.__new__(producer.OperatorInputs);value.base=base;value.directory=base;value.run='a'*32;value.bucket='wd-app-authority-'+'b'*32;value.byte_mode=True;value.byte_canary='authority/permission-delete-'+value.run;value.tools={'mc':{'path':'verified-mc'}};value.environment={};value.deadline=__import__('time').monotonic()+30;value.persist=lambda:None
  value.receipt={'byteCanary':{'objectKey':value.byte_canary,'sizeBytes':0,'sha256':hashlib.sha256(b'').hexdigest(),'state':'pending'}};return value
 def test_actual_mc_dispatches_only_exact_zero_canary_and_hash_verified_remove(self):
  with tempfile.TemporaryDirectory() as tmp:
   value=self.producer_instance(P(tmp));emitted=[]
   def command(argv,env,body,timeout,**kw):
    emitted.append((argv,body));return subprocess.CompletedProcess(argv,0,b'',b'')
   with patch.object(producer,'bounded_mc',side_effect=command):
    target='fixture/'+value.bucket+'/'+value.byte_canary;value.mc('pipe',target,input_bytes=b'')
    self.assertEqual(emitted[-1][0][-2:],['pipe',target]);self.assertEqual(emitted[-1][1],b'')
    before=len(emitted)
    for args,body in [(('pipe','fixture/foreign/'+value.byte_canary),b''),(('pipe',target),b'not-empty'),(('pipe',target,'--force'),b''),(('rm',target),None),(('unknown',target),None)]:
     with self.assertRaises(producer.InventoryError):value.mc(*args,input_bytes=body)
    self.assertEqual(len(emitted),before)
    value.cleanup_byte_objects();self.assertEqual(emitted[-1][0][-2:],['rm',target]);self.assertEqual(value.receipt['byteCanary']['state'],'closed')
 def test_measured_bucket_version_ordinal_metadata_is_strict(self):
  row={'status':'success','type':'folder','lastModified':'2026-10-27T00:00:00Z','size':0,'key':'wd-owned-bucket/','etag':'','url':'redacted-fixture-location','versionOrdinal':1}
  self.assertEqual(producer.parse_inventory('bucket',0,json.dumps(row).encode(),b''),{'wd-owned-bucket'})
  for poison in [True,-1,9223372036854775808,1.0,'1',None]:
   bad={**row,'versionOrdinal':poison}
   with self.assertRaises(producer.InventoryError):producer.parse_inventory('bucket',0,json.dumps(bad).encode(),b'')
  with self.assertRaises(producer.InventoryError):producer.parse_inventory('bucket',0,json.dumps({**row,'unknown':True}).encode(),b'')
 def test_short_relay_directory_is_pending_before_spawn_and_cleaned_on_early_exit(self):
  with tempfile.TemporaryDirectory() as tmp:
   base=P(tmp)/('deep-'+'x'*80);base.mkdir();overlay=P(tmp)/'overlay/overlay.json';source=overlay.parent/'files'/r.RELAY_INPUT;source.parent.mkdir(parents=True);source.write_bytes(b'pure fake relay not executed')
   inputs={'overlay':overlay,'overlayFiles':{r.RELAY_INPUT:hashlib.sha256(source.read_bytes()).hexdigest()}};journal=Journal();pending=[]
   original=journal.record
   def record(kind,value):
    if value['state']=='pending':
     directory=P(value['directory']);self.assertEqual(directory.parent,P('/tmp'));self.assertLessEqual(len(__import__('os').fsencode(value['socket'])),107);self.assertEqual(directory.stat().st_mode&0o777,0o700);pending.append(directory)
    original(kind,value)
   journal.record=record
   class Child:
    pid=__import__('os').getpid()
    def poll(self):return 1
    def wait(self,timeout):self.reaped=True;return 1
   child=Child()
   with patch.object(r.subprocess,'Popen',return_value=child):
    with self.assertRaises(ValueError):r.start_host_relay(inputs,base,journal)
   self.assertTrue(child.reaped);self.assertEqual(len(pending),1);self.assertFalse(pending[0].exists());self.assertEqual(journal.entries['host-relay']['state'],'closed')
 def test_relay_directory_cleanup_denies_changed_inode_mode_unknown_file_and_symlink(self):
  import os
  for fault in ['inode','mode','file','symlink']:
   directory=P(tempfile.mkdtemp(prefix='wd-relay-',dir='/tmp'));m=directory.stat();value={'directory':directory,'socket':directory/'minio.sock','directoryIdentity':(m.st_dev,m.st_ino)}
   try:
    if fault=='inode':value['directoryIdentity']=(m.st_dev,m.st_ino+1)
    elif fault=='mode':directory.chmod(0o750)
    elif fault=='file':(directory/'unknown').write_bytes(b'owned unit fixture')
    else:(directory/'minio.sock').symlink_to('/foreign-never-touched')
    with self.assertRaises(ValueError):r.remove_relay_directory(value)
    self.assertTrue(directory.exists())
   finally:
    directory.chmod(0o700)
    for p in directory.iterdir():p.unlink()
    directory.rmdir()
if __name__=='__main__':unittest.main()
