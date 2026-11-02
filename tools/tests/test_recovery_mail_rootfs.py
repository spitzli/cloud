"""Actual finite Mail closure construction using public local ELF/tool bytes; no Docker."""
import hashlib,importlib.util,io,json,os,pathlib,shutil,subprocess,tarfile,tempfile,unittest
from unittest.mock import patch
P=pathlib.Path
spec=importlib.util.spec_from_file_location('rootfs_recipe',P(__file__).with_name('recovery_gateway_rootfs.py'));r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
class MailRootfsTests(unittest.TestCase):
 def test_current_build_probe_matches_recipe_and_runner_before_build(self):
  spec=importlib.util.spec_from_file_location('mail_probe_contract',P(__file__).with_name('recovery-admission-component.py'));runner=importlib.util.module_from_spec(spec);spec.loader.exec_module(runner)
  probe=P(runner.MAIL_DNS_PROBE_DIR)/'dns-constructor-test'
  self.assertEqual(hashlib.sha256(probe.read_bytes()).hexdigest(),runner.MAIL_DNS_PROBE_SHA256)
  self.assertEqual(r.MAIL_DNS_PROBE_SHA256,runner.MAIL_DNS_PROBE_SHA256)
  runner.verify_mail_dns_recipe(P(r.__file__).read_bytes())
  for source in ("", "MAIL_DNS_PROBE_SHA256='old'", "MAIL_DNS_PROBE_SHA256=unknown", "MAIL_DNS_PROBE_SHA256='"+runner.MAIL_DNS_PROBE_SHA256+"'\nMAIL_DNS_PROBE_SHA256='old'"):
   with self.subTest(source=source),self.assertRaisesRegex(ValueError,'binding mismatch'):runner.verify_mail_dns_recipe(source)

 def test_actual_public_tool_closure_stages_files_and_tar_without_whole_base(self):
  with tempfile.TemporaryDirectory() as tmp:
   base=P(tmp)/'base';base.mkdir();paths={P('/bin/sh'),P('/bin/cp'),P('/bin/mkdir'),P('/bin/chmod'),P('/bin/tar'),P('/bin/sleep'),P('/bin/true')}
   for path in list(paths):
    i=r.interpreter(path)
    if i:paths.add(i)
    raw=subprocess.run(['ldd',str(path)],capture_output=True,check=True,timeout=5).stdout.decode()
    for line in raw.splitlines():
     part=line.split('=>')[-1].strip().split(' ',1)[0]
     if part.startswith('/'):paths.add(P(part))
   for path in paths:
    destination=base/str(path).lstrip('/');destination.parent.mkdir(parents=True,exist_ok=True);shutil.copyfile(path,destination);destination.chmod(0o555)
   # Public symlink layout exercises containment; no host-root symlink resolution is allowed.
   link=base/'bin/sh';link.unlink();shutil.copyfile('/bin/sh',base/'bin/dash');(base/'bin/dash').chmod(0o555);link.symlink_to('/bin/dash')
   (base/'unselected-secret').write_text('not-in-output')
   root=P(tmp)/'output'
   with patch.object(r,'MAIL_BASE_ROOT',base),patch.object(r,'MAIL_SOURCE_UID',os.getuid()):r.build_mail(base/'bin/true',root)
   manifest=json.loads((root/'mail-files.json').read_bytes());self.assertEqual(manifest['directories']['/etc']['mode'],'0o755');self.assertFalse((root/'etc/resolv.conf').exists());self.assertEqual(manifest['directories']['/run']['mode'],'0o755');self.assertEqual(manifest['directories']['/run/webdock']['mode'],'0o755');self.assertLess(manifest['bytes'],192*1024**2);self.assertLessEqual(manifest['fileCount'],128);self.assertLessEqual(manifest['inodeCount'],512);self.assertFalse((root/'unselected-secret').exists())
   for name,item in manifest['files'].items():self.assertEqual(hashlib.sha256((root/name.lstrip('/')).read_bytes()).hexdigest(),item['sha256'])
   work=P(tmp)/'work';work.mkdir();source=work/'source';source.mkdir();(source/'value').write_text('actual-copy')
   shell=root/'bin/dash'
   # Absolute generated aliases are intended for scratch root; invoke the copied canonical ELF here.
   script='mkdir -p "$1/private"; cp "$1/source/value" "$1/private/value"; chmod 600 "$1/private/value"; tar -C "$1/private" -cf "$1/result.tar" .; sleep 0'
   out=subprocess.run([str(shell),'-ec',script,'owned-unit',str(work)],env={'PATH':str(root/'bin')+':'+str(root/'usr/bin')},capture_output=True,timeout=5)
   self.assertEqual(out.returncode,0);self.assertEqual((work/'private/value').read_text(),'actual-copy');self.assertTrue((work/'result.tar').is_file())
   runner_spec=importlib.util.spec_from_file_location('mail_oci_reader',P(__file__).with_name('recovery-admission-component.py'));runner=importlib.util.module_from_spec(runner_spec);runner_spec.loader.exec_module(runner)
   # Structural OCI receipt test; these static fixture fields do not claim a scratch/kernel smoke.
   manifest['scratchBuildSmoke']={'uid':65532,'gid':65532,'commands':list(r.MAIL_TOOLS),'workerUnconfiguredExit':1,'workerStaticError':'mail_plugin_start_failed'}
   # OCI-format fixture metadata only; actual UID65532/bind proof is the separately retained kernel micro-repro.
   for entry in manifest['directories'].values():entry['uid']=0;entry['gid']=0
   manifest['filePlumbingSmoke']={'positive':{'uid':65532,'gid':65532,'reachedConfigDecode':True,'noReadMetadata':True,'intentionalInvalidPublicJSON':True,'mapping':'same-filesystem rename relocation, NOT bind','sameDevInodeMode':True},'wrongModeDenied':True,'symlinkDenied':True,'dummyFilesRemoved':True,'stageScriptSHA256':'3a2473c468628a2f45b98dd148810adabb1ccee604b5921af2d827ce76ad5099'}
   manifest['systemDnsSmoke']={'uid':65532,'gid':65532,'probeSHA256':r.MAIL_DNS_PROBE_SHA256,'constructorOnlyNoQuery':True,'cases':{'valid':{'constructorOk':True,'readable':True,'readErrno':None},'malformed':{'constructorOk':False,'readable':True,'readErrno':None},'missing':{'constructorOk':False,'readable':False,'readErrno':2},'inaccessible':{'constructorOk':False,'readable':False,'readErrno':13}},'dummyFilesAndProbeRemoved':True}
   (root/'mail-files.json').write_text(json.dumps(manifest))
   layer=io.BytesIO()
   with tarfile.open(fileobj=layer,mode='w') as archive:
    for path in sorted(root.rglob('*')):
     entry=archive.gettarinfo(str(path),arcname=str(path.relative_to(root)))
     if entry.isdir():entry.uid=0;entry.gid=0
     if entry.isfile():
      with path.open('rb') as stream:archive.addfile(entry,stream)
     else:archive.addfile(entry)
   payload=layer.getvalue();digest=hashlib.sha256(payload).hexdigest();layout={'layers':[{'digest':'sha256:'+digest}]};raw=json.dumps(layout).encode();md=hashlib.sha256(raw).hexdigest();oci=P(tmp)/'oci.tar'
   with tarfile.open(oci,'w') as archive:
    for name,data in [(digest,payload),(md,raw)]:
     entry=tarfile.TarInfo('blobs/sha256/'+name);entry.size=len(data);archive.addfile(entry,io.BytesIO(data))
   binding={'owned':{'platformManifestDigest':'sha256:'+md}};worker=manifest['files']['/usr/local/bin/webdock-mail']['sha256'];receipt=runner.mail_image_layout(oci,binding,'owned',worker)
   self.assertEqual(receipt['closure']['files'],manifest['files']);self.assertGreater(receipt['inodeEntriesUpper'],len(manifest['files']))
   with self.assertRaises(ValueError):runner.mail_image_layout(oci,binding,'owned','0'*64)

 def test_measured_base_gnucp_and_usrmerged_uutils_paths_preserve_applet_names(self):
  with tempfile.TemporaryDirectory() as tmp:
   base=P(tmp);(base/'usr/bin').mkdir(parents=True);(base/'usr/lib/cargo/bin/coreutils').mkdir(parents=True)
   (base/'bin').symlink_to('usr/bin');(base/'lib').symlink_to('usr/lib')
   shutil.copyfile('/bin/cp',base/'usr/bin/gnucp');(base/'usr/bin/gnucp').chmod(0o555);(base/'usr/bin/cp').symlink_to('gnucp')
   body=P('/bin/true').read_bytes()
   for tool in ('mkdir','chmod','sleep'):
    target=base/'usr/lib/cargo/bin/coreutils'/tool;target.write_bytes(body);target.chmod(0o555);(base/'usr/bin'/tool).symlink_to('../lib/cargo/bin/coreutils/'+tool)
   with patch.object(r,'MAIL_BASE_ROOT',base),patch.object(r,'MAIL_SOURCE_UID',os.getuid()):
    # Actual measured symlink layout, not a claim these unit applet bytes are BASE execution proof.
    canonical,source=r.mail_source(P('/bin/cp'),{},tool='cp');self.assertEqual(str(canonical),'/usr/bin/gnucp');self.assertEqual(source.read_bytes(),P('/bin/cp').read_bytes())
    with self.assertRaises(ValueError):r.mail_source(P('/usr/bin/gnucp'),{},tool='mkdir')
    for tool in ('mkdir','chmod','sleep'):
     links={};canonical,source=r.mail_source(P('/bin')/tool,links,tool=tool);self.assertEqual(str(canonical),'/usr/lib/cargo/bin/coreutils/'+tool);self.assertEqual(source.read_bytes(),body);self.assertEqual(links[P('/bin')],'usr/bin');self.assertEqual(links[P('/usr/bin')/tool],'../lib/cargo/bin/coreutils/'+tool)
    (base/'usr/lib/cargo/bin/coreutils/foreign').write_bytes(body);(base/'usr/bin/cp').unlink();(base/'usr/bin/cp').symlink_to('../lib/cargo/bin/coreutils/foreign')
    with self.assertRaises(ValueError):r.mail_source(P('/bin/cp'),{},tool='cp')
 def test_exact_nonmerged_lib_variant_and_owner_checks(self):
  with tempfile.TemporaryDirectory() as tmp:
   base=P(tmp);parent=base/'lib/cargo/bin/coreutils';parent.mkdir(parents=True);path=parent/'mkdir';path.write_bytes(P('/bin/true').read_bytes());path.chmod(0o555)
   with patch.object(r,'MAIL_BASE_ROOT',base),patch.object(r,'MAIL_SOURCE_UID',os.getuid()):
    canonical,_=r.mail_source(P('/lib/cargo/bin/coreutils/mkdir'),{},tool='mkdir');self.assertEqual(str(canonical),'/lib/cargo/bin/coreutils/mkdir')
    with self.assertRaises(ValueError):r.mail_source(P('/lib/cargo/bin/coreutils/mkdir'),{},tool='sleep')
    with self.assertRaises(ValueError):r.mail_source(P('/lib/cargo/bin/coreutils/mkdir'),{})
    with patch.object(r,'MAIL_SOURCE_UID',os.getuid()+1),self.assertRaises(ValueError):r.mail_source(P('/lib/cargo/bin/coreutils/mkdir'),{},tool='mkdir')
 def test_source_resolution_rejects_foreign_tool_symlink(self):
  with tempfile.TemporaryDirectory() as tmp:
   base=P(tmp);(base/'bin').mkdir();(base/'bin/sh').symlink_to('/etc/passwd')
   with patch.object(r,'MAIL_BASE_ROOT',base),self.assertRaises(ValueError):r.mail_source(P('/bin/sh'),{})

 def test_dns_build_probe_rejects_wrong_elf_before_creating_public_files(self):
  with tempfile.TemporaryDirectory() as tmp:
   root=P(tmp)/'root';root.mkdir();(root/'etc').mkdir();probe=P(tmp)/'probe';probe.write_bytes(b'foreign tool')
   with self.assertRaisesRegex(ValueError,'test-tool identity'):r.smoke_system_dns(root,probe,P('/unused'),'/usr/bin:/bin')
   self.assertEqual(list((root/'etc').iterdir()),[]);self.assertFalse((root/'mail-dns-probe').exists())

 def test_image_mount_parents_ignore_builder_umask_without_changing_host_paths(self):
  with tempfile.TemporaryDirectory() as tmp:
   target=P(tmp)/'image';old=os.umask(0o077)
   try:
    target.mkdir(mode=0o755);target.chmod(0o755)
    for path in (target/'run',target/'run/webdock'):path.mkdir(mode=0o755);path.chmod(0o755)
   finally:os.umask(old)
   self.assertEqual(r.mail_directories(target)['/run']['mode'],'0o755');self.assertEqual(r.mail_directories(target)['/run/webdock']['mode'],'0o755')

if __name__=='__main__':unittest.main()
