import ast,importlib.util,pathlib,re,stat,subprocess,types,unittest
from unittest.mock import patch
p=pathlib.Path(__file__).with_name('recovery-admission-component.py');spec=importlib.util.spec_from_file_location('mail_runner',p);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
class MailRunnerTests(unittest.TestCase):
 def receipt(self):return {'reference':str(m.ROOT/'.runtime-test/recovery-admission-aaaaaaaaaaaa/mail-data.ext4'),'dev':1,'inode':2,'uid':1000,'state':'allocated'}
 def metadata(self,**changes):
  value={'st_mode':stat.S_IFREG|0o600,'st_nlink':1,'st_uid':1000,'st_dev':1,'st_ino':2,'st_size':m.MAIL_DATA_BYTES};value.update(changes);return types.SimpleNamespace(**value)
 def test_exact_non_sparse_disk_custody_and_both_runtime_boundaries_required(self):
  r=self.receipt();self.assertEqual(m.mail_data_delete(r,self.metadata(),True,True),pathlib.Path(r['reference']))
  for container,loops in [(False,True),(True,False),(False,False)]:
   with self.assertRaises(ValueError):m.mail_data_delete(r,self.metadata(),container,loops)
 def test_foreign_disk_link_inode_owner_mode_and_namespace_deny(self):
  for changes in [{'st_ino':3},{'st_uid':0},{'st_nlink':2},{'st_mode':stat.S_IFLNK|0o600},{'st_mode':stat.S_IFREG|0o644},{'st_size':1}]:
   with self.assertRaises(ValueError):m.mail_data_delete(self.receipt(),self.metadata(**changes),True,True)
  for path in ['/tmp/foreign.ext4',str(m.ROOT/'.runtime-test/foreign/mail-data.ext4')]:
   with self.assertRaises(ValueError):m.mail_data_delete({**self.receipt(),'reference':path},self.metadata(),True,True)
 def test_mail_only_finite_store_and_proxy_transform(self):
  original=next(n.value for n in ast.walk(ast.parse(p.read_text())) if isinstance(n,ast.Constant) and isinstance(n.value,str) and n.value.startswith('#!/bin/bash\n') and '__OWNED_NESTING__' in n.value)
  result=m.mail_preflight(original);self.assertIn('--data-root=/owned/mail-data/engine',result);self.assertIn('--data-dir=/owned/mail-data/k3s',result);self.assertIn('conntrack-max-per-core=0',result);self.assertIn('conntrack-min=0',result);self.assertIn('--disable=traefik,servicelb,metrics-server,coredns,local-storage',result)
  self.assertLess(result.index('MAIL_FS_PROBE'),result.index('dockerd --host='))
  self.assertIn('mail_exact_required_crds',result);self.assertIn('mail_fixed_clusterip_gate',result);self.assertIn('/ce-source.tar|$OWNED_CE',result)
  with self.assertRaises(ValueError):m.mail_preflight(original+'--disable-kube-proxy')
 def test_actual_generated_cri_case_keeps_exact_rust_ce_and_owned_mail_names(self):
  original=next(n.value for n in ast.walk(ast.parse(p.read_text())) if isinstance(n,ast.Constant) and isinstance(n.value,str) and n.value.startswith('#!/bin/bash\n') and '__OWNED_NESTING__' in n.value)
  generated=m.mail_preflight(original);block=re.search(r'case "\$reference".*?esac',generated,re.S)[0]
  rust=(m.ROOT/'crates/control-plane/tests/support/recovery_mail_component.rs').read_text();ce=re.search(r'const CE_IMAGE: &str = "([^"]+)";',rust)[1];self.assertEqual(ce,m.CE)
  mail='localhost/owned/recovery-component@sha256:'+'a'*64
  for reference in (ce,mail):
   result=subprocess.run(['bash','-c','set -eu;reference=$1;OWNED_CE=$2;WEBDOCK_RECOVERY_MAIL_IMAGE=$3;'+block+';printf "%s" "$wanted"','owned-unit',reference,ce,mail],capture_output=True,timeout=2)
   self.assertEqual(result.returncode,0);self.assertEqual(result.stdout.decode(),reference)
  denied=subprocess.run(['bash','-c','set -eu;reference=$1;OWNED_CE=$2;WEBDOCK_RECOVERY_MAIL_IMAGE=$3;'+block,'owned-unit','foreign.invalid/image@sha256:'+'b'*64,ce,mail],capture_output=True,timeout=2)
  self.assertEqual(denied.returncode,64)
 def test_actual_generated_mail_dockerfile_is_finite_scratch_closure(self):
  recipe=m.mail_image_recipe('a'*12,'webdock.recovery-component-run','webdock.recovery-component-kind','native-recovery-component','wd-owned-recovery-component:'+'b'*12)
  self.assertEqual(recipe.splitlines()[0],'FROM webdock-rust-storage-test:owned AS mailbase')
  self.assertIn('FROM scratch',recipe);self.assertIn('--mount=from=mailbase,source=/,target=/cached-base,ro',recipe)
  self.assertIn('/tmp/rootfs.py /tmp/mail /mail-rootfs mail',recipe);self.assertIn('USER 65532:65532',recipe)
  self.assertIn('ENTRYPOINT ["/usr/local/bin/webdock-mail"]',recipe);self.assertNotIn('apt-get',recipe)
  self.assertIn('COPY mail-dns-probe /tmp/mail-dns-probe',recipe);self.assertIn('/tmp/mail-stage.sh /tmp/mail-dns-probe',recipe)
  self.assertNotIn('mail-dns-probe',recipe.split('FROM scratch\n',1)[1])
  self.assertFalse(any(line.startswith('FROM sha256:') for line in recipe.splitlines()))
  with self.assertRaises(ValueError):m.mail_image_recipe('a'*12,'r','k','v','foreign:image')
 def test_dns_probe_is_build_only_exact_pinned_tool_and_unknown_lib_change_denies(self):
  descriptor=m.verify_mail_dns_build_probe(m.ROOT)
  self.assertEqual(descriptor['sha256'],m.MAIL_DNS_PROBE_SHA256);self.assertEqual(descriptor['bytes'],m.MAIL_DNS_PROBE_BYTES)
  self.assertIn('BUILD-ONLY',descriptor['scope'])
  # The captured source hash remains mandatory; the current BUILD-only source/worker pair admits no unknown edit.
  with patch.object(m,'MAIL_DNS_TEST_LIB_SHA256','0'*64),self.assertRaises(ValueError):m.verify_mail_dns_build_probe(m.ROOT)

 def test_actual_pending_before_build_and_unknown_acquisitions_remain_unresolved(self):
  spec=importlib.util.spec_from_file_location('approved_custody',p.with_name('recovery_fixture_custody.py'));c=importlib.util.module_from_spec(spec)
  import sys,tempfile
  sys.modules[spec.name]=c;spec.loader.exec_module(c)
  with tempfile.TemporaryDirectory() as directory:
   acquisitions=c.Acquisitions(directory)
   acquisitions.record('mail-image',{'reference':'owned-test','state':'pending-before-build'})
   self.assertEqual(m.unresolved_acquisitions(acquisitions),['mail-image'])
   for state in ('pending-before-create','allocated','unknown-new-state'):
    acquisitions.record('mail-data',{'reference':'owned-file','state':state});self.assertIn('mail-data',m.unresolved_acquisitions(acquisitions))
   acquisitions.record('mail-image',{'state':'closed'});self.assertNotIn('mail-image',m.unresolved_acquisitions(acquisitions))
 def test_source_contract_no_sparse_disk_no_hidden_data_exec(self):
  source=p.read_text();self.assertIn('os.posix_fallocate(fd,0,MAIL_DATA_BYTES)',source);self.assertIn('m.st_blocks*512<MAIL_DATA_BYTES',source)
  self.assertIn('rw,noexec,nodev,nosuid',m.MAIL_FILESYSTEM_SETUP);self.assertIn('17*1024**3',m.MAIL_FILESYSTEM_SETUP);self.assertIn('freeInodes',m.MAIL_FILESYSTEM_SETUP)
 def test_whole_loop_inventory_accepts_fresh_zero_and_new_unbound_nodes_without_equality(self):
  before={'sysfs':{'fsType':'sysfs','complete':True},'control':{'fileType':'char','major':10,'minor':237,'uid':0},'block':{},'devBlock':{},'attachments':{},'module':{'present':False}}
  self.assertEqual(m.validate_mail_loop_inventory(before),before)
  import copy
  after=copy.deepcopy(before);row={'major':7,'minor':0,'target':'/sys/devices/virtual/block/loop0','targetIdentity':{'dev':1,'ino':2}}
  after['block']['loop0']=row;after['devBlock']['7:0']=copy.deepcopy(row);after['attachments']['loop0']={'sizeSectors':0,'readOnly':0,'loopDirectoryPresent':False,'backingFile':None,'autoclear':None,'offset':None,'sizeLimit':None};after['module']={'present':True}
  self.assertEqual(m.validate_mail_loop_inventory(after),after);self.assertNotEqual(before,after)
  after['attachments']['loop0']['sizeSectors']=1
  with self.assertRaises(ValueError):m.validate_mail_loop_inventory(after)
  before['sysfs']['complete']=False
  with self.assertRaises(ValueError):m.validate_mail_loop_inventory(before)
 def test_loop_receipts_follow_autoclear_and_hold_exact_identity_before_unpin(self):
  source=m.MAIL_FILESYSTEM_SETUP
  self.assertLess(source.index('losetup --detach "$maildev"'),source.index('owned_mail_loop_receipt mail-data autoclear-held'))
  self.assertLess(source.index('owned_mail_loop_receipt mail-data mounted-autoclear'),source.index('exec {mail_loop_pin}<&-'))
  self.assertLess(source.index('losetup --detach "$imagedev"'),source.index('owned_mail_loop_receipt image-store autoclear-held'))
  self.assertLess(source.index('owned_mail_loop_receipt image-store mounted-autoclear'),source.index('exec {image_loop_pin}<&-'))
  for guard in ("row['back-ino']==meta.st_ino","row['back-maj:min']==number(meta.st_dev)","row['maj:min']==number(held.st_rdev)","row['autoclear'] is True","row['ro'] is False","os.O_EXCL|os.O_NOFOLLOW"):
   self.assertIn(guard,source)
  self.assertIn('loopModuleStateChangedDuringRun',p.read_text());self.assertIn('newUnboundKernelLoopNodes',p.read_text());self.assertNotIn('modprobe',source)
  script=source.split("<<'OWNED_MAIL_LOOP_RECEIPT'\n",1)[1].split('\nOWNED_MAIL_LOOP_RECEIPT',1)[0];compile(script,'owned-loop-receipt','exec')

 def private_loop_fixture(self):
  module=types.ModuleType('owned_private_loop_nodes_test');exec(compile(m.MAIL_PRIVATE_LOOP_NODE_SOURCE,'verified-private-loop-nodes','exec'),module.__dict__)
  host={'mountNS':11,'dev':7,'ino':1}
  actual={'uid':0,'mountNS':22,'dev':{'dev':99,'ino':1,'uid':0,'gid':0,'mode':0o755,'kind':'directory'},'devFS':'tmpfs','devWritable':True,'mountPropagationPrivate':True,'completeKernelEnumeration':True,'control':{'kind':'char','uid':0,'nlink':1,'rdev':__import__('os').makedev(10,237),'dev':99},'kernel':{0:{'bytes':0,'readOnly':0,'attachment':False}}}
  def node(n,**kw):
   row={'dev':99,'ino':100+n,'uid':0,'gid':0,'mode':0o600,'nlink':1,'kind':'block','rdev':__import__('os').makedev(7,n)};row.update(kw);return row
  return module,host,actual,node
 def test_private_loop_nodes_preserve_existing_0660_and_create_exact_missing_range(self):
  module,host,actual,node=self.private_loop_fixture();existing={0:node(0,gid=993,mode=0o660),5:node(5)};calls=[]
  def create(n):calls.append(n);return node(n)
  result=module.prepare(host,actual,existing,create)
  self.assertEqual(calls,[n for n in range(32) if n not in existing]);self.assertEqual(result['count'],32);self.assertEqual(result['createdCount'],30);self.assertEqual(result['blockDataBytes'],0);self.assertEqual(result['nodes'][0]['metadata'],existing[0]);self.assertEqual(result['nodes'][5]['metadata'],existing[5])
  self.assertNotIn('os.chmod',m.MAIL_PRIVATE_LOOP_NODE_SOURCE);self.assertNotIn('ioctl',m.MAIL_PRIVATE_LOOP_NODE_SOURCE);self.assertNotIn("os.open('/dev/",m.MAIL_PRIVATE_LOOP_NODE_SOURCE)
 def test_private_loop_nodes_deny_hostlike_namespace_device_propagation_or_unknown_kernel_before_creation(self):
  import copy
  module,host,actual,node=self.private_loop_fixture()
  poisons=[('mountNS',11),('devFS','devtmpfs'),('devWritable',False),('mountPropagationPrivate',False),('completeKernelEnumeration',False),('uid',1000)]
  for key,value in poisons:
   v=copy.deepcopy(actual);v[key]=value
   with self.subTest(key=key),self.assertRaises(AssertionError):module.prepare(host,v,{},lambda n:self.fail('creation before boundary proof'))
  v=copy.deepcopy(actual);v['dev']['dev']=7
  with self.assertRaises(AssertionError):module.prepare(host,v,{},lambda n:self.fail('hostlike /dev creation'))
  for n,row in [(0,{'bytes':512,'readOnly':0,'attachment':True}),(0,{'bytes':0,'readOnly':0,'attachment':True}),(32,{'bytes':0,'readOnly':0,'attachment':False})]:
   v=copy.deepcopy(actual);v['kernel']={n:row}
   with self.subTest(kernel=n,row=row),self.assertRaises(AssertionError):module.prepare(host,v,{},lambda n:self.fail('unknown/bound kernel creation'))
 def test_private_loop_nodes_validate_all_existing_before_any_mutation(self):
  module,host,actual,node=self.private_loop_fixture()
  for n,row in [(31,node(31,kind='symlink')),(31,node(31,rdev=__import__('os').makedev(7,30))),(31,node(31,uid=1000)),(31,node(31,nlink=2)),(31,node(31,dev=7)),(32,node(32)),(-1,node(0)),(True,node(1))]:
   with self.subTest(minor=n,row=row),self.assertRaises(AssertionError):module.prepare(host,actual,{n:row},lambda n:self.fail('must validate high existing minor before creating missing0'))
  module.COUNT=33
  with self.assertRaises(AssertionError):module.prepare(host,actual,{},lambda n:self.fail('range expansion'))
 def test_private_loop_nodes_new_identity_mode_and_EXCL_failure_deny(self):
  module,host,actual,node=self.private_loop_fixture()
  for change in ({'gid':993},{'mode':0o660},{'kind':'symlink'},{'rdev':0},{'nlink':2}):
   with self.subTest(change=change),self.assertRaises(AssertionError):module.prepare(host,actual,{},lambda n:node(n,**change))
  def occupied(n):raise FileExistsError('public occupied node')
  with self.assertRaises(FileExistsError):module.prepare(host,actual,{},occupied)
 def test_private_loop_setup_precedes_first_loop_and_keeps_original_whole_guards(self):
  self.assertTrue(m.MAIL_FILESYSTEM_SETUP.startswith('set -euo pipefail'))
  self.assertLess(m.MAIL_FILESYSTEM_SETUP.index('OWNED_PRIVATE_LOOP_NODES'),m.MAIL_FILESYSTEM_SETUP.index('maildev=$(losetup'))
  for field in ('OWNED_HOST_MOUNT_NS','OWNED_HOST_DEV_DEVICE','OWNED_HOST_DEV_INODE'):self.assertIn(field,p.read_text());self.assertIn(field,m.MAIL_PRIVATE_LOOP_NODE_SOURCE)
  self.assertIn('os.O_EXCL|os.O_NOFOLLOW',m.MAIL_PRIVATE_LOOP_NODE_SOURCE);self.assertIn('os.fsync',m.MAIL_PRIVATE_LOOP_NODE_SOURCE);self.assertIn('kernel_unbound()==actual',m.MAIL_PRIVATE_LOOP_NODE_SOURCE)
  self.assertIn("'--privileged'",p.read_text());self.assertIn('newUnboundKernelLoopNodes',p.read_text());self.assertIn('loopModuleStateChangedDuringRun',p.read_text())


 def test_private_kernel_attachment_unknown_is_not_absence(self):
  module,*_=self.private_loop_fixture()
  class Missing:
   def lstat(self):raise FileNotFoundError()
  class Denied:
   def lstat(self):raise PermissionError(13,'public sysfs deny')
  class Present:
   def lstat(self):return object()
  self.assertFalse(module.attachment_present(Missing()));self.assertTrue(module.attachment_present(Present()))
  with self.assertRaises(PermissionError):module.attachment_present(Denied())

if __name__=='__main__':unittest.main()
