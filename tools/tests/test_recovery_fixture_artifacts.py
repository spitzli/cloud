"""Pure artifact admission; never invoke Docker or mutate cached images."""
import ast
import hashlib
import io
import tarfile
import shlex
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SPEC=importlib.util.spec_from_file_location("component",Path(__file__).with_name("recovery-admission-component.py"))
runner=importlib.util.module_from_spec(SPEC);SPEC.loader.exec_module(runner)

def digest(path):return hashlib.sha256(Path(path).read_bytes()).hexdigest()

class ArtifactSelectionTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        self.root=Path(self.temp.name);self.base_pin=self.root/'.runtime-test/recovery-current-source-component-6223d3e8baf0/pin.json'
        self.pin=self.root/runner.ARTIFACT_PIN_PATH
        self.base_pin.parent.mkdir(parents=True);self.pin.parent.mkdir(parents=True)
        self.overlay=self.root/'.runtime-test/input-overlay/overlay.json';self.overlay.parent.mkdir(parents=True)
        self.hashes={};base_entries={}
        names=('webdock-control-plane','webdock-agent-control','webdock-migrate','webdock-bootstrap-operator','webdock-enroll-agent','webdock-bootstrap-plugins','webdock-plugin-control','webdock-plugin-controller','webdock-agent','webdock-storage','webdock-recovery','webdock-recovery-gateway','bootstrap-test')
        for name in names:
            path=self.base_pin.parent/name;path.write_bytes(b'ELF original fixture '+name.encode());path.chmod(0o500)
            base_entries[name]={'path':str(path),'copiedStrippedSHA256':digest(path),'bytes':path.stat().st_size,'mode':'0500','originalBuildSHA256':'a'*64,'source':'original fixture compile provenance'}
        path=self.elf_path('bootstrap-test');path.write_bytes(b'ELF new diagnostic fixture');path.chmod(0o500)
        new_agent=self.pin.parent/'webdock-agent';new_agent.write_bytes(b'ELF new convergent Agent');new_agent.chmod(0o500)
        self.hashes={'bootstrap-test':digest(path),'webdock-agent':digest(new_agent),'webdock-storage':base_entries['webdock-storage']['copiedStrippedSHA256']}
        self.archive=self.base_pin.parent/'public-source.tar.gz';self.archive.write_bytes(b'curated original source fixture')
        self.base_pin.write_text(json.dumps({'binaries':base_entries,'publicSourceArchiveSHA256':digest(self.archive),'sourceSHA256':{},'migrations':{},'rootLockSHA256':'a'*64}))
        self.test_archive=self.pin.parent/'diagnostic-test-source.tar.gz';self.test_archive.write_bytes(b'new diagnostic source fixture')
        self.inventory=self.pin.parent/'diagnostic-test-compilation.json';self.inventory.write_text(json.dumps({'inputs':{},'rootLockSHA256':'a'*64,'migrations':{},'archiveSHA256':digest(self.test_archive)}))
        entries={**base_entries,'bootstrap-test':{'path':str(path),'copiedStrippedSHA256':digest(path),'bytes':path.stat().st_size,'mode':'0500','originalBuildSHA256':'b'*64,'source':'diagnostic fixture compile provenance'}}
        entries['webdock-agent']={'path':str(new_agent),'copiedStrippedSHA256':digest(new_agent),'bytes':new_agent.stat().st_size,'mode':'0500','originalBuildSHA256':'c'*64,'source':'new convergent Agent fixture provenance'}
        self.pin.write_text(json.dumps({'version':'source-health-pair-v1','originalBasePin':{'path':str(self.base_pin),'sha256':digest(self.base_pin)},'originalBaseSourceArchive':{'path':str(self.archive),'sha256':digest(self.archive)},'binaries':entries,'unchangedOriginalELFs':sorted(set(names)-{'bootstrap-test','webdock-agent'}),'diagnosticTest':{'name':'bootstrap-test','compilationInventory':{'path':str(self.inventory),'sha256':digest(self.inventory)},'sourceArchive':{'path':str(self.test_archive),'sha256':digest(self.test_archive)},'sourceSHA256':{},'rootLockSHA256':'a'*64,'migrations':{}}}))
        mixed=json.loads(self.pin.read_text());mixed['replacementCompilation']={'names':['bootstrap-test','webdock-agent'],'sourceArchive':mixed['diagnosticTest']['sourceArchive'],'compilationInventory':mixed['diagnosticTest']['compilationInventory']};self.pin.write_text(json.dumps(mixed))
        self.files={}
        paths=('tools/tests/recovery-admission-component.py','tools/tests/recovery_fixture_custody.py','tools/tests/mail-application-recovery.py','agents/kubernetes/ntfy_recovery.py','deploy/helm/webdock-agent/Chart.yaml','deploy/helm/webdock-agent/values.yaml','deploy/helm/webdock-agent/values.schema.json','deploy/helm/webdock-agent/templates/agent.yaml')
        for rel in paths:
            live=self.root/rel;live.parent.mkdir(parents=True,exist_ok=True);live.write_bytes(Path(__file__).with_name('recovery_fixture_custody.py').read_bytes() if rel=='tools/tests/recovery_fixture_custody.py' else ('pinned '+rel).encode())
            copied=self.overlay.parent/'files'/rel;copied.parent.mkdir(parents=True,exist_ok=True);copied.write_bytes(live.read_bytes());self.files[rel]=digest(copied)
        self.write_overlay();self.addCleanup(patch.stopall)
        for name,value in [('ARTIFACT_PIN_SHA256',digest(self.pin)),('BASE_PIN_SHA256',digest(self.base_pin)),('TEST_SOURCE_ARCHIVE_SHA256',digest(self.test_archive)),('TEST_COMPILATION_SHA256',digest(self.inventory)),('ARTIFACT_ELFS',self.hashes),('ARTIFACT_PIN_PATH',str(self.pin.relative_to(self.root)))]:patch.object(runner,name,value,create=True).start()

    def elf_path(self,name):
        return (self.pin.parent if name in ('bootstrap-test','webdock-agent') else self.base_pin.parent)/name

    def write_overlay(self):
        self.overlay.write_text(json.dumps({'purpose':'recovery-source-component-input-overlay-v1','baseArtifactPinSHA256':digest(self.pin),'files':self.files}))

    def select(self,**changes):
        self.assertTrue(callable(getattr(runner,'select_whole_inputs',None)),'whole mode lacks verified finite artifact selection')
        args={'root':self.root,'artifact_pin':self.pin,'driver':'docker','input_overlay':self.overlay,'input_overlay_sha256':digest(self.overlay),'test':None};args.update(changes)
        return runner.select_whole_inputs(**args)

    def test_exact_pinned_selection_and_unmodified_copy(self):
        selected=self.select();target=self.root/'build';target.mkdir()
        runner.copy_whole_inputs(selected,target)
        for name,expected in self.hashes.items():self.assertEqual(digest(target/name),expected)
        self.assertEqual((target/'chart/Chart.yaml').read_bytes(),(self.overlay.parent/'files/deploy/helm/webdock-agent/Chart.yaml').read_bytes())
        self.assertEqual(selected['driver'],'docker')

    def test_missing_manifest_driver_or_overlay_rejected(self):
        for change in ({'artifact_pin':None},{'driver':None},{'driver':'kubernetes'},{'input_overlay':None},{'input_overlay_sha256':None}):
            with self.subTest(change=change),self.assertRaises(ValueError):self.select(**change)

    def test_wrong_manifest_path_hash_or_contradictory_test_rejected(self):
        foreign=self.root/'foreign.json';foreign.write_bytes(self.pin.read_bytes())
        for change in ({'artifact_pin':foreign},{'test':self.root/'historical-test'},{'input_overlay_sha256':'0'*64}):
            with self.subTest(change=change),self.assertRaises(ValueError):self.select(**change)
        self.pin.write_text(self.pin.read_text()+' ')
        with self.assertRaises(ValueError):self.select()

    def test_each_elf_hash_or_symlink_and_foreign_owner_rejected(self):
        for name in self.hashes:
            path=self.elf_path(name);original=path.read_bytes();path.chmod(0o700);path.write_bytes(b'changed');path.chmod(0o500)
            with self.subTest(name=name),self.assertRaises(ValueError):self.select()
            path.chmod(0o700);path.write_bytes(original);path.chmod(0o500)
        path=self.elf_path('webdock-agent');original=path.read_bytes();path.unlink();other=self.root/'other';other.write_bytes(original);path.symlink_to(other)
        with self.assertRaises(ValueError):self.select()
        path.unlink();path.write_bytes(original);path.chmod(0o500)
        with patch.object(runner.os,'getuid',return_value=os.getuid()+1),self.assertRaises(ValueError):self.select()

    def test_overlay_missing_extra_changed_live_and_symlink_denied(self):
        rel='deploy/helm/webdock-agent/Chart.yaml';expected=self.files.pop(rel);self.write_overlay()
        with self.assertRaises(ValueError):self.select()
        self.files[rel]=expected;self.files['extra']='0'*64;self.write_overlay()
        with self.assertRaises(ValueError):self.select()
        del self.files['extra'];self.write_overlay();live=self.root/rel;original=live.read_bytes();live.write_bytes(b'changed live chart')
        with self.assertRaises(ValueError):self.select()
        live.write_bytes(original);copied=self.overlay.parent/'files'/rel;copied.unlink();copied.symlink_to(live)
        with self.assertRaises(ValueError):self.select()

    def test_source_archive_and_precopy_drift_denied(self):
        selected=self.select();path=self.elf_path('webdock-agent');path.chmod(0o700);path.write_bytes(b'changed after selection');path.chmod(0o500)
        target=self.root/'build';target.mkdir()
        with self.assertRaises(ValueError):runner.copy_whole_inputs(selected,target)
        self.archive.write_bytes(b'changed archive')
        with self.assertRaises(ValueError):self.select()

    def test_whole_cli_missing_binding_denied_before_fixture_import_or_acquisition(self):
        with patch('sys.argv',['component','--driver','docker']),patch.object(runner.importlib.util,'spec_from_file_location',side_effect=AssertionError('fixture import before admission')):
            with self.assertRaises(ValueError):runner.main()

    def test_cli_bad_elf_or_contradictory_path_denies_before_import_and_acquisition(self):
        for mutation in ('elf','test'):
            path=self.elf_path('webdock-agent');original=path.read_bytes()
            args=['component','--driver','docker','--artifact-pin',str(self.pin),'--input-overlay',str(self.overlay),'--input-overlay-sha256',digest(self.overlay)]
            if mutation=='elf':path.chmod(0o700);path.write_bytes(b'changed');path.chmod(0o500)
            else:args+=['--test',str(self.root/'historical935e')]
            with self.subTest(mutation=mutation),patch.object(runner,'ROOT',self.root),patch('sys.argv',args),patch.object(runner.importlib.util,'spec_from_file_location',side_effect=AssertionError('fixture import before denial')),patch.object(runner.subprocess,'run',side_effect=AssertionError('command before denial')):
                with self.assertRaises(ValueError):runner.main()
            path.chmod(0o700);path.write_bytes(original);path.chmod(0o500)

    def test_extra_live_chart_file_and_manifest_entry_redirection_denied(self):
        extra=self.root/'deploy/helm/webdock-agent/extra.yaml';extra.write_text('unexpected')
        with self.assertRaises(ValueError):self.select()
        extra.unlink();entry=json.loads(self.pin.read_text());entry['binaries']['webdock-agent']['path']=str(self.root/'foreign-agent')
        self.pin.write_text(json.dumps(entry));self.write_overlay()
        with patch.object(runner,'ARTIFACT_PIN_SHA256',digest(self.pin)),self.assertRaises(ValueError):self.select()

    def test_runner_binding_survives_pause_descriptor_selection_and_sets_driver(self):
        inputs=self.select();tree=ast.parse(Path(runner.__file__).read_text());main=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='main')
        binding=next(n for n in main.body if isinstance(n,ast.If) and 'artifactBinding' in ast.unparse(n))
        namespace={'inputs':inputs,'selected':[{'digest':'pause-descriptor'}],'report':{'rootLock':'b'*64}}
        exec(compile(ast.Module(body=[binding],type_ignores=[]),runner.__file__,'exec'),namespace)
        self.assertEqual(namespace['report']['artifactBinding']['artifactPinSHA256'],digest(self.pin))
        self.assertEqual(namespace['report']['liveRootLockSHA256'],'b'*64)
        self.assertEqual(namespace['report']['rootLock'],inputs['compilation']['rootLockSHA256'])
        create=next(n for n in ast.walk(main) if isinstance(n,ast.Call) and isinstance(n.func,ast.Attribute) and n.func.attr=='create' and any(k.arg=='env' for k in n.keywords))
        env=next(k.value for k in create.keywords if k.arg=='env');driver=next(v for k,v in zip(env.keys,env.values) if isinstance(k,ast.Constant) and k.value=='WEBDOCK_RECOVERY_COMPONENT_DRIVER')
        self.assertEqual(eval(compile(ast.Expression(driver),runner.__file__,'eval'),namespace),'docker')

    def test_nonregular_input_is_rejected_before_open(self):
        fifo=self.root/'fifo';os.mkfifo(fifo)
        with patch.object(runner.os,'open',side_effect=AssertionError('nonregular file must not be opened')):
            with self.assertRaises(ValueError):runner.verified_file(fifo,'0'*64)

    def test_main_executes_selected_fixture_and_dependency_bytes_despite_replacements(self):
        sources={'tools/tests/mail-application-recovery.py':b"import sys\nsys.path.insert(0,'fixture-path-must-not-leak')\nimport ntfy_recovery as profile\nVALUE='verified-fixture'\n",'agents/kubernetes/ntfy_recovery.py':b"VALUE='verified-dependency'\n"}
        for rel,raw in sources.items():
            for path in (self.root/rel,self.overlay.parent/'files'/rel):path.write_bytes(raw)
            self.files[rel]=hashlib.sha256(raw).hexdigest()
        self.write_overlay();inputs=self.select()
        for rel in sources:
            for path in (self.root/rel,self.overlay.parent/'files'/rel):path.write_bytes(b"raise AssertionError('unverified replacement executed')\n")
        tree=ast.parse(Path(runner.__file__).read_text());main=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='main')
        # Exercise the actual main loader block, without creating a fixture or issuing commands.
        start=next(i for i,n in enumerate(main.body) if isinstance(n,ast.Assert))+1
        end=next(i for i,n in enumerate(main.body) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='run' for t in n.targets))
        namespace={**vars(runner),'ROOT':self.root,'inputs':inputs}
        import sys,types
        old_path=list(sys.path)
        with patch.dict('sys.modules',{'ntfy_recovery':types.SimpleNamespace(VALUE='unverified-cached-module')}):
            try:
                exec(compile(ast.Module(body=main.body[start:end],type_ignores=[]),runner.__file__,'exec'),namespace)
                observed_path=list(sys.path)
            finally:sys.path[:]=old_path
        self.assertEqual(observed_path,old_path)
        self.assertEqual(namespace['m'].VALUE,'verified-fixture')
        self.assertEqual(namespace['m'].profile.VALUE,'verified-dependency')
        target=self.root/'build';target.mkdir();runner.copy_whole_inputs(inputs,target)
        self.assertEqual(digest(target/'webdock-agent'),self.hashes['webdock-agent'])

    def test_manifest_descriptor_bytes_survive_postverification_fifo_or_symlink_swap(self):
        original_verified=runner.verified_file;original_read=Path.read_bytes
        for which in ('base','overlay'):
            for replacement in ('fifo','symlink'):
                path=self.pin if which=='base' else self.overlay
                original=path.read_bytes();backing=self.root/'backing';backing.write_bytes(original);swapped=[]
                arguments={'root':self.root,'artifact_pin':self.pin,'driver':'docker','input_overlay':self.overlay,'input_overlay_sha256':digest(self.overlay)}
                def verify(candidate,*args,**kwargs):
                    value=original_verified(candidate,*args,**kwargs)
                    if Path(candidate)==path and not swapped:
                        path.unlink()
                        if replacement=='fifo':os.mkfifo(path)
                        else:path.symlink_to(backing)
                        swapped.append(True)
                    return value
                def read(candidate):
                    if candidate==path and swapped:raise AssertionError('unchecked manifest pathname reopen; FIFO would block')
                    return original_read(candidate)
                try:
                    with self.subTest(which=which,replacement=replacement),patch.object(runner,'verified_file',side_effect=verify),patch.object(Path,'read_bytes',read):
                        selected=runner.select_whole_inputs(**arguments)
                        self.assertEqual(selected['driver'],'docker')
                finally:
                    path.unlink();path.write_bytes(original)

    def test_copy_rejects_mode_drift_before_staging_that_executable(self):
        selected=self.select();agent=self.elf_path('webdock-agent');agent.chmod(0o700)
        target=self.root/'build';target.mkdir()
        with self.assertRaises(ValueError):runner.copy_whole_inputs(selected,target)
        self.assertFalse((target/'webdock-agent').exists())

    def custody_import_probe(self,kind):
        import builtins,py_compile,sys,types
        approved=Path(__file__).with_name('recovery_fixture_custody.py').read_bytes()
        self.assertEqual(hashlib.sha256(approved).hexdigest(),'4313a68353a2964d933047a79e2ff96ed4b690da2945d9a5f0f7c9dd8b8d307c')
        custody_path=self.root/'tools/tests/recovery_fixture_custody.py';custody_path.write_bytes(approved)
        (self.overlay.parent/'files/tools/tests/recovery_fixture_custody.py').write_bytes(approved)
        self.files['tools/tests/recovery_fixture_custody.py']=hashlib.sha256(approved).hexdigest()
        runner_path=self.root/'tools/tests/recovery-admission-component.py';runner_path.write_bytes(Path(runner.__file__).read_bytes())
        (self.overlay.parent/'files/tools/tests/recovery-admission-component.py').write_bytes(runner_path.read_bytes())
        self.files['tools/tests/recovery-admission-component.py']=digest(runner_path);self.write_overlay()
        if kind=='pyc':
            # Timestamp-valid old bytecode beside byte-identical approved source.
            tree=ast.parse(approved);node=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='component_delete')
            lines=approved.decode().splitlines(True);lines[node.lineno-1:node.end_lineno]=["def component_delete(receipt,current,protected_ids):\n    return ('image','rm','binwiederhier/ntfy:latest')\n"]
            malicious=b'import builtins\nbuiltins._recovery_unverified_custody_access=True\n'+''.join(lines).encode();malicious+=b' '*(len(approved)-len(malicious));self.assertEqual(len(malicious),len(approved))
            custody_path.write_bytes(malicious);os.utime(custody_path,(1700000000,1700000000));py_compile.compile(str(custody_path),doraise=True)
            custody_path.write_bytes(approved);os.utime(custody_path,(1700000000,1700000000))
            cached=None
        else:
            import recovery_fixture_custody as original
            class CachedCustody(types.ModuleType):
                def __getattribute__(self,name):
                    if name in ('ComponentImage','component_delete'):builtins._recovery_unverified_custody_access=True
                    return super().__getattribute__(name)
            cached=CachedCustody('recovery_fixture_custody');cached.__dict__.update(vars(original));cached.component_delete=lambda *args:('image','rm','binwiederhier/ntfy:latest')
        before_path=list(sys.path)
        try:
            sys.path.insert(0,str(custody_path.parent))
            with patch.dict(sys.modules,{}),patch.object(builtins,'_recovery_unverified_custody_access',False,create=True),patch.object(runner.subprocess,'run',side_effect=AssertionError('subprocess forbidden')):
                if cached is None:sys.modules.pop('recovery_fixture_custody',None)
                else:sys.modules['recovery_fixture_custody']=cached
                spec=importlib.util.spec_from_file_location('probe_component',runner_path);fresh=importlib.util.module_from_spec(spec);spec.loader.exec_module(fresh)
                self.assertFalse(builtins._recovery_unverified_custody_access,'unverified custody executed or cached module consumed before admission')
                with patch.object(fresh,'ARTIFACT_PIN_SHA256',digest(self.pin)),patch.object(fresh,'ARTIFACT_ELFS',self.hashes),patch.object(fresh,'BASE_PIN_SHA256',digest(self.base_pin),create=True),patch.object(fresh,'TEST_SOURCE_ARCHIVE_SHA256',digest(self.test_archive),create=True),patch.object(fresh,'TEST_COMPILATION_SHA256',digest(self.inventory),create=True):
                    main=next(n for n in ast.parse(runner_path.read_text()).body if isinstance(n,ast.FunctionDef) and n.name=='main')
                    end=next(i for i,n in enumerate(main.body) if isinstance(n,ast.Assert))
                    whole=['--artifact-pin',str(self.pin),'--driver','docker','--input-overlay',str(self.overlay),'--input-overlay-sha256',digest(self.overlay)]
                    for flags in (whole,['--preflight-only','--test',str(self.elf_path('bootstrap-test'))],['--preflight-only','--import-check','--test',str(self.elf_path('bootstrap-test'))]):
                        with self.subTest(kind=kind,flags=flags),patch('sys.argv',['component',*flags]):
                            namespace={**vars(fresh),'ROOT':self.root};exec(compile(ast.Module(body=main.body[:end],type_ignores=[]),str(runner_path),'exec'),namespace)
                            protected='sha256:'+'c'*64;receipt=namespace['ComponentImage']('123456abcdef',protected)
                            current={'Id':protected,'RepoTags':['binwiederhier/ntfy:latest'],'Config':{'Labels':{}},'RepoDigests':[]}
                            with self.assertRaises(ValueError):namespace['component_delete'](receipt,current,{protected})
        finally:sys.path[:]=before_path
        self.assertEqual(custody_path.read_bytes(),approved)

    def test_timestamp_valid_stale_custody_pyc_cannot_control_any_mode(self):
        self.custody_import_probe('pyc')

    def test_cached_custody_module_cannot_control_any_mode(self):
        self.custody_import_probe('cached')

    def test_loaded_fixture_root_uses_admitted_checkout_after_live_symlink(self):
        rel='tools/tests/mail-application-recovery.py';raw=Path(__file__).with_name('mail-application-recovery.py').read_bytes()
        for path in (self.root/rel,self.overlay.parent/'files'/rel):path.write_bytes(raw)
        self.files[rel]=hashlib.sha256(raw).hexdigest()
        dependency='agents/kubernetes/ntfy_recovery.py'
        for path in (self.root/dependency,self.overlay.parent/'files'/dependency):path.write_bytes(b"PROFILE='verified-profile'\n")
        self.files[dependency]=digest(self.root/dependency);self.write_overlay();inputs=self.select()
        live=self.root/rel;live.unlink();live.symlink_to(self.root/'nonexistent-checkout/tools/tests/mail-application-recovery.py')
        with patch.object(runner.subprocess,'run',side_effect=AssertionError('subprocess forbidden')):
            module=runner.load_whole_fixture(inputs,self.root)
            self.assertEqual(module.ROOT,self.root)
            self.assertEqual(module.Fixture.command.__globals__['ROOT'],self.root)
        target=self.root/'build';target.mkdir();runner.copy_whole_inputs(inputs,target)
        self.assertEqual(digest(target/'webdock-agent'),self.hashes['webdock-agent'])

    def reject_before_command(self):
        args=['component','--artifact-pin',str(self.pin),'--driver','docker','--input-overlay',str(self.overlay),'--input-overlay-sha256',digest(self.overlay)]
        with patch.object(runner,'ROOT',self.root),patch('sys.argv',args),patch.object(runner.subprocess,'run',side_effect=AssertionError('command before mixed admission denial')):
            with self.assertRaises((ValueError,OSError)):runner.main()

    def test_mixed_missing_or_foreign_original_entry_denies_before_acquisition(self):
        original=self.pin.read_bytes()
        for change in ('missing','foreign'):
            manifest=json.loads(original)
            if change=='missing':del manifest['binaries']['webdock-recovery-gateway']
            else:manifest['binaries']['webdock-agent']['path']=str(self.pin.parent/'foreign-agent')
            self.pin.write_text(json.dumps(manifest));self.write_overlay()
            with self.subTest(change=change),patch.object(runner,'ARTIFACT_PIN_SHA256',digest(self.pin)):self.reject_before_command()
        self.pin.write_bytes(original)

    def test_parent_pin_missing_changed_or_foreign_reference_denies_before_acquisition(self):
        original=self.base_pin.read_bytes()
        self.base_pin.unlink();self.reject_before_command();self.base_pin.write_bytes(original+b' ');self.reject_before_command();self.base_pin.write_bytes(original)
        manifest=json.loads(self.pin.read_text());manifest['originalBasePin']['path']=str(self.root/'foreign-parent.json');self.pin.write_text(json.dumps(manifest));self.write_overlay()
        with patch.object(runner,'ARTIFACT_PIN_SHA256',digest(self.pin)):self.reject_before_command()

    def test_new_test_archive_or_compilation_mismatch_denies_before_acquisition(self):
        for path in (self.test_archive,self.inventory):
            original=path.read_bytes();path.write_bytes(original+b' ')
            with self.subTest(path=path.name):self.reject_before_command()
            path.write_bytes(original)

    def test_mixed_sources_copy_and_record_separately(self):
        selected=self.select();target=self.root/'provenance';target.mkdir()
        self.assertTrue(callable(getattr(runner,'copy_source_provenance',None)),'mixed source provenance copy is missing')
        runner.copy_source_provenance(selected,target)
        for source,name in ((self.archive,'source-snapshot.tar.gz'),(self.test_archive,'diagnostic-test-source.tar.gz'),(self.inventory,'diagnostic-test-compilation.json')):self.assertEqual(digest(source),digest(target/name))
        self.assertEqual(selected['basePinSHA256'],digest(self.base_pin))

    def test_health_pair_requires_new_agent_instead_of_original_fallback(self):
        selected=self.select()
        self.assertEqual(selected['binaries']['webdock-agent']['path'],self.pin.parent/'webdock-agent')
        self.assertNotEqual(selected['binaries']['webdock-agent']['sha256'],json.loads(self.base_pin.read_text())['binaries']['webdock-agent']['copiedStrippedSHA256'])
        manifest=json.loads(self.pin.read_text());manifest['binaries']['webdock-agent']['path']=str(self.base_pin.parent/'webdock-agent');self.pin.write_text(json.dumps(manifest));self.write_overlay()
        with patch.object(runner,'ARTIFACT_PIN_SHA256',digest(self.pin)):self.reject_before_command()

class CapacitySqlLiteralTests(unittest.TestCase):
    def test_literal_recovery_first_projection_names_status_for_actual_consumer(self):
        sql=(Path(__file__).resolve().parents[2]/'crates/persistence/migrations/0054_source_recovery_fences.sql').read_text()
        body=sql.split('CREATE OR REPLACE FUNCTION webdock_rls.capacity_demand(',1)[1]
        cte=body.split('WITH reservations AS (',1)[1].split('), dimensions AS',1)[0]
        first=cte.split('UNION ALL',1)[0]
        self.assertRegex(first,r"END\s+AS\s+status\s+FROM webdock_auth.hosting_recovery_reservation")
        self.assertIn("r.status='active'",body.split('amounts AS',1)[1])

class ModernPrimaryScriptTests(unittest.TestCase):
    def script(self):
        tree=ast.parse(Path(runner.__file__).read_text());main=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='main')
        assignment=next(n for n in ast.walk(main) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='preflight_source' for t in n.targets) and isinstance(n.value,ast.Call))
        template=assignment.value
        while isinstance(template,ast.Call) and isinstance(template.func,ast.Attribute) and template.func.attr=='replace':
            template=template.func.value
        return ast.literal_eval(template),main

    def test_primary_is_one_modern_store_with_same_bounded_namespace(self):
        script,_=self.script();commands=[shlex.split(line.strip()) for line in script.splitlines() if line.strip().startswith('dockerd ')]
        self.assertEqual(len(commands),1)
        command=commands[0]
        for item in ('--host=unix:///var/run/docker.sock','--data-root=/owned/engine','--storage-driver=native'):self.assertIn(item,command)
        self.assertIn(['--feature','containerd-snapshotter=true'],[command[i:i+2] for i in range(len(command)-1)])
        self.assertIn(['--feature','lazy-pull=false'],[command[i:i+2] for i in range(len(command)-1)])
        self.assertNotIn('--storage-driver=vfs',command)
        self.assertNotIn('/owned/modern',script);self.assertNotIn('modern-engine.sock',script)
        self.assertIn('size=3g',Path(runner.__file__).read_text())

    def test_postgres_execution_selector_is_verified_root_not_classic_config(self):
        _,main=self.script();call=next(n for n in ast.walk(main) if isinstance(n,ast.Call) and isinstance(n.func,ast.Attribute) and n.func.attr=='create' and any(k.arg=='env' for k in n.keywords))
        env=next(k.value for k in call.keywords if k.arg=='env');selector=next(v for k,v in zip(env.keys,env.values) if isinstance(k,ast.Constant) and k.value=='WEBDOCK_RECOVERY_POSTGRES_IMAGE_ID')
        context={'report':{'importBindings':{runner.PG:{'rootDigest':'authenticated-root-id','configurationID':'not-a-runtime-selector'}}},'PG':runner.PG}
        self.assertEqual(eval(compile(ast.Expression(selector),runner.__file__,'eval'),context),'authenticated-root-id')

    def test_oci_transport_retains_root_platform_and_config_identities(self):
        def blob(value):
            raw=json.dumps(value,separators=(',',':')).encode();return 'sha256:'+hashlib.sha256(raw).hexdigest(),raw
        config,c=blob({'os':'linux','architecture':'amd64'});platform,p=blob({'config':{'digest':config}});root,r=blob({'manifests':[{'platform':{'os':'linux','architecture':'amd64'},'digest':platform}]})
        with tempfile.TemporaryDirectory() as directory:
            archive=Path(directory)/'images.tar'
            with tarfile.open(archive,'w') as tar:
                for digest,raw in ((config,c),(platform,p),(root,r)):
                    member=tarfile.TarInfo('blobs/sha256/'+digest.split(':')[1]);member.size=len(raw);tar.addfile(member,io.BytesIO(raw))
            reference='public/fixture@'+root
            self.assertEqual(runner.image_bindings(archive,[reference])[reference],{'rootDigest':root,'platformManifestDigest':platform,'configurationID':config})

class PauseArchiveScriptTests(unittest.TestCase):
    def test_k3s_imports_pause_only_while_docker_keeps_all_images(self):
        script,_=ModernPrimaryScriptTests().script()
        self.assertIn('cp /pause.tar /owned/pause.tar',script)
        imports=[line for line in script.splitlines() if 'ctr --address' in line and ' images import ' in line]
        self.assertEqual(len(imports),1);self.assertIn('/owned/pause.tar',imports[0]);self.assertNotIn('/owned/images.tar',imports[0])
        self.assertIn('docker load -i /owned/images.tar',script)
        self.assertIn('for reference in "$OWNED_PAUSE"; do',script)
        self.assertNotIn('for reference in "$OWNED_NTFY" "$OWNED_PAUSE"',script)

    def test_pause_export_exact_selector_and_partial_failure_custody(self):
        self.assertTrue(callable(getattr(runner,'save_pause_archive',None)),'pause-only exporter missing')
        config_raw=json.dumps({'os':'linux','architecture':'amd64'}).encode();config='sha256:'+hashlib.sha256(config_raw).hexdigest()
        manifest_raw=json.dumps({'config':{'digest':config}}).encode();root='sha256:'+hashlib.sha256(manifest_raw).hexdigest();reference='rancher/mirrored-pause@'+root
        for fail in (False,True):
            with self.subTest(fail=fail),tempfile.TemporaryDirectory() as d:
                directory=Path(d);calls=[];custody=runner.load_verified_custody(None,Path(runner.__file__).resolve().parents[2]);journal=custody.Acquisitions(directory)
                def invoke(*args,**kwargs):
                    calls.append(args)
                    self.assertEqual(journal.entries['pause-archive']['state'],'pending')
                    target=Path(args[2]);self.assertEqual(args,('save','--output',str(directory/'pause.tar'),reference))
                    if fail:
                        target.write_bytes(b'partial');raise TimeoutError('unknown save outcome')
                    with tarfile.open(target,'w') as tar:
                        for digest,raw in ((config,config_raw),(root,manifest_raw)):
                            member=tarfile.TarInfo('blobs/sha256/'+digest.split(':')[1]);member.size=len(raw);tar.addfile(member,io.BytesIO(raw))
                    import subprocess
                    return subprocess.CompletedProcess(args,0,b'',b'')
                commands=custody.CheckedCommands(invoke,directory)
                if fail:
                    with self.assertRaises(TimeoutError):runner.save_pause_archive(commands,journal,directory,reference)
                    self.assertIn('pause-archive',journal.unresolved())
                else:
                    archive,bindings=runner.save_pause_archive(commands,journal,directory,reference)
                    self.assertEqual(archive,directory/'pause.tar');self.assertEqual(set(bindings),{reference});self.assertEqual(bindings[reference],{'rootDigest':root,'platformManifestDigest':root,'configurationID':config})
                    # Execute the actual checked copy expression with a lost response;
                    # no later checkpoint may claim the local archive obligation closed.
                    _,main=ModernPrimaryScriptTests().script()
                    call=next(n for n in ast.walk(main) if isinstance(n,ast.Call) and isinstance(n.func,ast.Name) and n.func.id=='checked_docker' and n.args and isinstance(n.args[0],ast.Constant) and n.args[0].value=='copy-input-pause')
                    def unknown_copy(*args,**kwargs):raise TimeoutError('copy response lost')
                    with self.assertRaises(TimeoutError):eval(compile(ast.Expression(call),runner.__file__,'eval'),{'checked_docker':unknown_copy,'pause_archive':archive,'container':'owned-container'})
                    self.assertIn('pause-archive',journal.unresolved())
                self.assertEqual(len(calls),1)

    def test_finalizer_tracks_both_owned_tar_files(self):
        _,main=ModernPrimaryScriptTests().script();finalizer=next(n.finalbody for n in main.body if isinstance(n,ast.Try));source=ast.unparse(ast.Module(body=finalizer,type_ignores=[]))
        self.assertIn("base / 'images.tar'",source);self.assertIn("base / 'pause.tar'",source);self.assertIn("base / 'build'",source)

if __name__=='__main__':unittest.main()
