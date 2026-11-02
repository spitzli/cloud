"""Execute actual main's entire prelaunch block with real unprivileged files/permissions.
Only external Docker and MinIO calls are replaced; age/SSH and private Unix relay are real.
"""
import ast,hashlib,importlib.util,json,os,pathlib,shutil,stat,subprocess,tempfile,types,unittest,uuid
from unittest.mock import patch
P=pathlib.Path;ROOT=P(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('prelaunch_runner',P(__file__).with_name('recovery-admission-component.py'));r=importlib.util.module_from_spec(spec);spec.loader.exec_module(r)
producer_spec=importlib.util.spec_from_file_location('prelaunch_operator',P(__file__).with_name('recovery_operator_fixture.py'));producer=importlib.util.module_from_spec(producer_spec);producer_spec.loader.exec_module(producer)
custody_spec=importlib.util.spec_from_file_location('prelaunch_custody',P(__file__).with_name('recovery_fixture_custody.py'));custody=importlib.util.module_from_spec(custody_spec);custody_spec.loader.exec_module(custody)
class PrelaunchTests(unittest.TestCase):
 def test_actual_main_prelaunch_with_real_files_transform_permissions_and_all_mount_sources(self):
  for prefix,prelude in ((False,False),(True,False),(False,True)):
   with self.subTest(prefix=prefix,prelude=prelude):self.prelaunch(prefix,prelude)
 def prelaunch(self,prefix,prelude):
  self.assertNotEqual(os.geteuid(),0,'this regression must exercise actual unprivileged permission enforcement')
  tree=ast.parse(P(r.__file__).read_text());main=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='main');body=next(n.body for n in main.body if isinstance(n,ast.Try))
  first=next(i for i,n in enumerate(body) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='preflight' for t in n.targets))
  last=next(i for i,n in enumerate(body[first:],first) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='container' for t in n.targets))
  with tempfile.TemporaryDirectory(prefix='wd-mail-prelaunch-') as temporary:
   root=P(temporary);base=root/'.runtime-test'/('recovery-admission-'+uuid.uuid4().hex[:12]);base.mkdir(parents=True,mode=0o700)
   (root/'.dev-local').mkdir(mode=0o700);env=root/'.dev-local/minio.env';env.write_text('MINIO_ROOT_USER=unit-admin\nMINIO_ROOT_PASSWORD=unit-private-placeholder\n');env.chmod(0o600)
   files=root/'overlay/files/tools/tests';files.mkdir(parents=True);relay=files/'recovery_minio_relay.py';shutil.copyfile(ROOT/'tools/tests/recovery_minio_relay.py',relay);relay.chmod(0o400)
   authority=json.loads((ROOT/'.runtime-test/recovery-app-authority-candidate-430e18a7b65a/pin.json').read_bytes())
   inputs={'authorityOnly':True,'operatorTools':authority['operatorTools'],'overlay':root/'overlay/overlay.json','overlayFiles':{r.RELAY_INPUT:hashlib.sha256(relay.read_bytes()).hexdigest()},'pythonSources':{r.AUTHORITY_OPERATOR_INPUT:(ROOT/r.AUTHORITY_OPERATOR_INPUT).read_bytes()},'driver':'kubernetes'}
   names={k:set() for k in ('bucket','user','policy')}
   def external_mc(argv,environment,input_bytes,timeout,output_limit=262144):
    args=argv[argv.index('--config-dir')+2:];rows=[]
    if args[:2]==['ls','fixture']:
     rows=[{'status':'success','type':'folder','lastModified':'2026-10-27T00:00:00Z','size':0,'key':n+'/','etag':''} for n in names['bucket']]
    elif args[:3] in (['admin','user','list'],['admin','policy','list']):
     kind=args[1];rows=[({'status':'success','accessKey':n,'userStatus':'enabled'} if kind=='user' else {'status':'success','policy':n,'policyInfo':{'PolicyName':'','Policy':None},'isGroup':False}) for n in names[kind]]
    elif args[0]=='mb':names['bucket'].add(args[1].split('/',1)[1])
    elif args[:3]==['admin','user','add']:names['user'].add(input_bytes.decode().splitlines()[0])
    elif args[:3]==['admin','policy','create']:names['policy'].add(args[4])
    elif args[0]=='pipe':pass
    elif args[:3]==['admin','policy','attach']:pass
    else:raise AssertionError('unexpected external provider command')
    return subprocess.CompletedProcess([],0,b'\n'.join(json.dumps(v).encode() for v in rows),b'')
   # The normal verified-byte loader executes the actual producer. Only its external I/O is replaced.
   original_loader=r.load_operator_fixture
   def loader(value,checkout):
    module=original_loader(value,checkout);module.bounded_mc=external_mc;module.OperatorInputs.service_state=staticmethod(lambda port:{'port':port,'metadataSHA256':'a'*64});return module
   gateway_archive=base/'gateway-image.tar';gateway_archive.write_bytes(b'previously-verified-OCI-unit-input')
   mail_archive=base/'mail-image.tar';mail_archive.write_bytes(b'previously-verified-OCI-unit-input')
   ce_archive=base/'ce-source.tar';ce_archive.write_bytes(b'previously-verified-OCI-unit-input')
   # Prelaunch consumes an already verified data-file receipt; no18Gi allocation/physical claim in this filesystem-order test.
   (base/'mail-budget.json').write_text('{}')
   disk=base/'mail-data.ext4';disk.write_bytes(b'prelaunch-mount-path-unit-input');disk.chmod(0o600)
   fixture_spec=importlib.util.spec_from_file_location('prelaunch_legacy',ROOT/'tools/tests/mail-application-recovery.py');legacy=importlib.util.module_from_spec(fixture_spec);fixture_spec.loader.exec_module(legacy)
   fixture=legacy.Fixture(base,(ROOT/r.AUTHORITY_PIN_PATH).parent/'bootstrap-test');created=[]
   def external_docker(*arguments,**kwargs):
    self.assertEqual(arguments[0],'create');self.assertNotIn('/var/run/docker.sock',arguments)
    for i,arg in enumerate(arguments):
     if arg=='--mount':
      fields=dict(x.split('=',1) for x in arguments[i+1].split(',') if '=' in x)
      if fields.get('type')=='bind':self.assertTrue(P(fields['src']).exists())
    created.append((arguments,kwargs));return subprocess.CompletedProcess([],0,('a'*64+'\n').encode(),b'')
   fixture.docker=external_docker
   namespace={**vars(r),'args':types.SimpleNamespace(mail=True,bytes=True,mail_invoke_prefix=prefix,mail_prelude=prelude,preflight_only=False,import_check=False),'base':base,'f':fixture,'inputs':inputs,'chart_sha':'a'*64,'diagnostic_volume':'unit-own-diagnostics','gateway_archive':gateway_archive,'gateway_reference':'localhost/owned/recovery-component@sha256:'+'b'*64,'gateway_bindings':{},'mail_archive':mail_archive,'mail_reference':'localhost/owned/recovery-component@sha256:'+'c'*64,'ce_archive':ce_archive,'mail_data':{'reference':str(disk)},'pause_ref':'rancher/mirrored-pause@sha256:'+'d'*64,'image_id':'sha256:'+'e'*64,'acquisitions':custody.Acquisitions(base),'report':{'importBindings':{r.PG:{'rootDigest':'sha256:'+'f'*64}}},'host_relay':None,'operator_inputs':None}
   namespace['acquisitions'].record('gateway-archive',{'state':'verified','layers':{},'reference':str(gateway_archive)})
   try:
    with patch.object(r,'ROOT',root),patch.object(r,'load_operator_fixture',side_effect=loader):
     namespace['ROOT']=root;namespace['load_operator_fixture']=r.load_operator_fixture
     exec(compile(ast.Module(body=body[first:last+1],type_ignores=[]),str(P(r.__file__)),'exec'),namespace)
    preflight=base/'preflight.sh';self.assertEqual(stat.S_IMODE(preflight.stat().st_mode),0o555);self.assertIn('mail_fixed_clusterip_gate',preflight.read_text());self.assertEqual(len(created),1);self.assertEqual('WEBDOCK_RECOVERY_MAIL_INVOKE_PREFIX' in created[0][0],prefix);self.assertEqual(created[0][1]['env'].get('WEBDOCK_RECOVERY_MAIL_INVOKE_PREFIX'), '1' if prefix else None);self.assertEqual(created[0][1]['env'].get('WEBDOCK_RECOVERY_MAIL_PRELUDE'),'1' if prelude else None)
    server=[line for line in preflight.read_text().splitlines() if 'exec /usr/local/bin/k3s server ' in line]
    self.assertEqual(len(server),1)
    self.assertIn('--snapshotter=overlayfs',server[0])
    self.assertIn('--cluster-dns=172.30.254.1',server[0])
    self.assertIn('mail-offline-dns-ready.json',preflight.read_text())
    self.assertNotIn('--snapshotter=native',server[0])
    self.assertNotIn('local-storage-capacity-isolation=false',preflight.read_text())
    subprocess.run(['bash','-n',str(preflight)],check=True,capture_output=True,timeout=2)
    with self.assertRaises(PermissionError):preflight.write_text('must-not-rewrite-readonly')
    self.assertEqual(set(p.name for p in namespace['private_operator'].iterdir()),{'inputs.json','writer.json','reader.json','identity.agekey','operator.pub','operator.key','byte-inputs.json','native-installation-key'})
   finally:
    relay_state=namespace.get('host_relay')
    if relay_state:
     child=relay_state['child'];child.terminate();child.wait(timeout=5);relay_state['stream'].close();self.assertFalse(relay_state['socket'].exists());r.remove_relay_directory(relay_state)
if __name__=='__main__':unittest.main()
