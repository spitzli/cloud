import ast
import contextlib
import errno
import io
import json
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import recovery_fixture_custody as custody
from pathlib import Path
from recovery_fixture_custody import (
    CheckedCommands,Acquisitions,acquire_image,image_probe,retain_diagnostics,diagnostic_delete,wait_and_capture,cleanup_container,container_probe,
)

REF='wd-owned-recovery-component:123456abcdef'
PAUSE='rancher/mirrored-pause@sha256:'+'a'*64

def result(args,rc=0,out=b'',err=b''):
    return subprocess.CompletedProcess(args,rc,out,err)

def absent(ref):
    return result([],1,b'[]\n',('Error response from daemon: No such image: '+ref+'\n').encode())

class FakeCommands:
    def __init__(self,values):self.values=list(values);self.emitted=[]
    def __call__(self,*args,**kwargs):
        self.emitted.append(args)
        value=self.values.pop(0)
        if isinstance(value,BaseException):raise value
        if callable(value):return value(args)
        return value

def artifact(directory, commands, stage, kind, index=-1):
    attempts=[a for a in commands.attempts if a['stage']==stage]
    return Path(directory)/attempts[index]['files'][kind]

class RunnerStageTests(unittest.TestCase):
    def test_unknown_permission_socket_and_timeout_never_pull_or_delete(self):
        failures=[result([],1,b'[]',b'permission denied'),result([],1,b'',b'No such file or directory: socket'),result([],1,b'[]',b'Error response from daemon: No such image: foreign:latest'),subprocess.TimeoutExpired(['docker','inspect'],1,output=b'partial',stderr=b'timeout')]
        for failure in failures:
            with self.subTest(failure=type(failure).__name__),tempfile.TemporaryDirectory() as d:
                fake=FakeCommands([failure,result([]),result([],0,json.dumps([{'Id':'sha256:'+'b'*64}]).encode())])
                commands=CheckedCommands(fake,d);journal=Acquisitions(d)
                try:acquire_image('pause',PAUSE,('pull',PAUSE),commands,journal,lambda current:None)
                except Exception:pass
                self.assertEqual(fake.emitted,[('image','inspect',PAUSE)])
                self.assertEqual(journal.unresolved(),[])

    def test_exact_expected_image_absence_allows_one_acquisition(self):
        with tempfile.TemporaryDirectory() as d:
            fake=FakeCommands([absent(PAUSE),result([]),result([],0,json.dumps([{'Id':'sha256:'+'b'*64}]).encode())])
            commands=CheckedCommands(fake,d)
            acquire_image('pause',PAUSE,('pull',PAUSE),commands,Acquisitions(d),lambda current:None)
            self.assertEqual(fake.emitted,[('image','inspect',PAUSE),('pull',PAUSE),('image','inspect',PAUSE)])
            self.assertEqual(artifact(d,commands,'pause-before','stderr').read_bytes(),absent(PAUSE).stderr)

    def test_collision_or_unknown_cannot_build_retag_or_delete(self):
        for probe in [result([],0,json.dumps([{'Id':'sha256:'+'c'*64}]).encode()),result([],1,b'',b'daemon unavailable')]:
            with tempfile.TemporaryDirectory() as d:
                fake=FakeCommands([probe,result([]),result([],0,json.dumps([{'Id':'sha256:'+'b'*64}]).encode())])
                try:acquire_image('component',REF,('build','--tag',REF,'.'),CheckedCommands(fake,d),Acquisitions(d),lambda current:None)
                except Exception:pass
                self.assertEqual(fake.emitted,[('image','inspect',REF)])

    def test_successful_build_then_failed_inspect_retains_pending_obligation(self):
        with tempfile.TemporaryDirectory() as d:
            def build(args):
                state=json.loads((Path(d)/'acquisitions.json').read_text())
                self.assertEqual(state['component']['state'],'pending')
                return result(args)
            fake=FakeCommands([absent(REF),build,result([],1,b'',b'connection lost')]);journal=Acquisitions(d)
            try:acquire_image('component',REF,('build','--tag',REF,'.'),CheckedCommands(fake,d),journal,lambda current:None)
            except Exception:pass
            self.assertEqual(journal.unresolved(),['component'])
            self.assertFalse(any(x[:2]==('image','rm') for x in fake.emitted))

    def test_timeout_retains_partial_output_and_original_exception_privately(self):
        with tempfile.TemporaryDirectory() as d:
            error=subprocess.TimeoutExpired(['docker','image','rm',REF],1,output=b'partial stdout',stderr=b'partial stderr')
            fake=FakeCommands([error]);commands=CheckedCommands(fake,d)
            with self.assertRaises(Exception):commands.call('remove-component','image','rm',REF)
            self.assertEqual(artifact(d,commands,'remove-component','stdout').read_bytes(),b'partial stdout')
            self.assertEqual(artifact(d,commands,'remove-component','stderr').read_bytes(),b'partial stderr')
            metadata=json.loads(artifact(d,commands,'remove-component','exception').read_text())
            self.assertIn('TimeoutExpired',metadata['type'])
            self.assertIn('timed out',metadata['message'])

    def test_partial_or_failed_diagnostic_capture_never_removes_volume(self):
        with tempfile.TemporaryDirectory() as d:
            def partial(args):
                target=Path(args[-1]);target.mkdir();(target/'partial').write_bytes(b'partial')
                return result(args,1,b'copied some',b'copy failed')
            fake=FakeCommands([partial]);commands=CheckedCommands(fake,d);capture={'retained':False}
            try:retain_diagnostics(commands,'owned-container',d,capture)
            except Exception:pass
            emitted=[]
            try:emitted.append(diagnostic_delete('owned-volume',capture,{'Labels':{'webdock.recovery-fixture':'owner'}},'owner'))
            except ValueError:pass
            self.assertEqual(emitted,[])
            self.assertTrue((Path(d)/'diagnostics/partial').is_file())

    def test_verified_capture_hash_receipt_allows_exact_owned_volume_cleanup(self):
        with tempfile.TemporaryDirectory() as d:
            def copy(args):
                target=Path(args[-1]);target.mkdir();(target/'preflight.log').write_bytes(b'private log');(target/'stage').write_bytes(b'fixed-stage')
                return result(args)
            capture={'retained':False};commands=CheckedCommands(FakeCommands([copy]),d)
            self.assertTrue(retain_diagnostics(commands,'owned-container',d,capture))
            self.assertIn('sha256',capture['files']['preflight.log'])
            self.assertTrue((Path(d)/'diagnostic-retention.json').is_file())
            self.assertEqual(diagnostic_delete('owned-volume',capture,{'Labels':{'webdock.recovery-fixture':'owner'}},'owner'),('volume','rm','owned-volume'))

    def test_wait_timeout_still_attempts_retention_and_preserves_primary_error(self):
        with tempfile.TemporaryDirectory() as d:
            error=subprocess.TimeoutExpired(['docker','wait','owned'],1,output=b'wait partial',stderr=b'wait timeout')
            def copy(args):
                target=Path(args[-1]);target.mkdir();(target/'preflight.log').write_bytes(b'private');(target/'stage').write_bytes(b'failed-wait')
                return result(args)
            fake=FakeCommands([error,copy]);capture={'retained':False};stopped=[]
            with self.assertRaises(subprocess.TimeoutExpired):
                wait_and_capture(CheckedCommands(fake,d),'owned',d,capture,lambda:stopped.append(True))
            self.assertEqual(stopped,[True]);self.assertTrue(capture['retained'])
            self.assertTrue(list(Path(d).glob('wait-primary--*.exception.json')))

    def test_failed_wait_and_failed_copy_keep_primary_and_volume_obligation(self):
        with tempfile.TemporaryDirectory() as d:
            error=subprocess.TimeoutExpired(['docker','wait','owned'],1,output=b'partial')
            fake=FakeCommands([error,result([],1,b'partial copied',b'copy fail')]);capture={'retained':False}
            with self.assertRaises(subprocess.TimeoutExpired):
                wait_and_capture(CheckedCommands(fake,d),'owned',d,capture,lambda:None)
            self.assertFalse(capture['retained']);self.assertIn('secondaryErrorSHA256',capture)
            self.assertIn('primaryErrorSHA256',capture)
            with self.assertRaises(ValueError):diagnostic_delete('owned-volume',capture,{'Labels':{'webdock.recovery-fixture':'owner'}},'owner')

    def test_pull_timeout_keeps_pre_effect_pending_record(self):
        with tempfile.TemporaryDirectory() as d:
            fake=FakeCommands([absent(PAUSE),subprocess.TimeoutExpired(['docker','pull',PAUSE],1,output=b'partial pull')]);journal=Acquisitions(d)
            commands=CheckedCommands(fake,d)
            with self.assertRaises(subprocess.TimeoutExpired):acquire_image('pause',PAUSE,('pull',PAUSE),commands,journal,lambda value:None)
            self.assertEqual(journal.unresolved(),['pause'])
            self.assertEqual(artifact(d,commands,'pause-acquire','stdout').read_bytes(),b'partial pull')

    def test_owned_container_remove_failure_retains_raw_stderr_and_exception(self):
        with tempfile.TemporaryDirectory() as d:
            entry={'name':'owned-container','id':'a'*64};meta={'Id':entry['id'],'Name':'/owned-container','Config':{'Labels':{'webdock.recovery-fixture':'owner'}},'State':{'Running':False}}
            fake=FakeCommands([result([],0,json.dumps([meta]).encode()),result([],1,b'remove partial',b'provider remove failed')])
            with self.assertRaises(Exception):cleanup_container(entry,'owner',CheckedCommands(fake,d))
            self.assertTrue(any(p.read_bytes()==b'provider remove failed' for p in Path(d).glob('*.stderr')))
            self.assertTrue(any(Path(d).glob('*.exception.json')))
            self.assertEqual(fake.emitted[-1],('rm','--volumes',entry['id']))

    def test_foreign_container_identity_never_emits_stop_or_remove(self):
        with tempfile.TemporaryDirectory() as d:
            entry={'name':'owned-container','id':'a'*64};meta={'Id':entry['id'],'Name':'/owned-container','Config':{'Labels':{'webdock.recovery-fixture':'foreign'}},'State':{'Running':True}}
            fake=FakeCommands([result([],0,json.dumps([meta]).encode())])
            with self.assertRaises(ValueError):cleanup_container(entry,'owner',CheckedCommands(fake,d))
            self.assertEqual(fake.emitted,[('inspect',entry['id'])])

    def test_retry_retains_partial_copy_without_overwriting_it(self):
        with tempfile.TemporaryDirectory() as d:
            partial=Path(d)/'diagnostics';partial.mkdir();(partial/'partial').write_bytes(b'old-partial')
            def copy(args):
                target=Path(args[-1]);target.mkdir();(target/'preflight.log').write_bytes(b'complete');(target/'stage').write_bytes(b'stopped')
                return result(args)
            capture={'retained':False};retain_diagnostics(CheckedCommands(FakeCommands([copy]),d),'owned',d,capture)
            self.assertNotEqual(capture['directory'],'diagnostics')
            self.assertEqual((partial/'partial').read_bytes(),b'old-partial')

    def test_acquisition_cannot_probe_own_reference_then_tag_foreign_reference(self):
        with tempfile.TemporaryDirectory() as d:
            fake=FakeCommands([])
            with self.assertRaises(ValueError):acquire_image('component',REF,('build','--tag','binwiederhier/ntfy:latest','.'),CheckedCommands(fake,d),Acquisitions(d),lambda value:None)
            self.assertEqual(fake.emitted,[])

    def test_failed_copy_then_retry_preserves_each_raw_attempt_and_crossreference(self):
        with tempfile.TemporaryDirectory() as d:
            def partial(args):
                target=Path(args[-1]);target.mkdir();(target/'partial').write_bytes(b'partial')
                return result(args,1,b'FIRST RAW STDOUT',b'FIRST RAW FAILURE')
            def complete(args):
                target=Path(args[-1]);target.mkdir();(target/'stage').write_bytes(b'done');(target/'preflight.log').write_bytes(b'complete')
                return result(args,0,b'SECOND RAW STDOUT',b'SECOND RAW STDERR')
            commands=CheckedCommands(FakeCommands([partial,complete]),d);capture={'retained':False}
            with self.assertRaises(Exception):retain_diagnostics(commands,'owned',d,capture)
            retain_diagnostics(commands,'owned',d,capture)
            all_evidence=[p.read_bytes() for p in Path(d).rglob('*') if p.is_file()]
            for expected in [b'FIRST RAW STDOUT',b'FIRST RAW FAILURE',b'SECOND RAW STDOUT',b'SECOND RAW STDERR']:
                self.assertIn(expected,all_evidence)
            attempts=[a for a in commands.attempts if a['stage']=='copy-diagnostics']
            self.assertEqual(len(attempts),2)
            self.assertEqual(len({a['attemptID'] for a in attempts}),2)
            for a in attempts:
                metadata=json.loads((Path(d)/a['files']['metadata']).read_text())
                self.assertEqual(metadata['attemptID'],a['attemptID'])
                self.assertTrue((Path(d)/metadata['files']['stderr']).is_file())

    def test_empty_or_whitespace_required_diagnostics_hold_volume(self):
        for contents in [b'',b' \n']:
            with self.subTest(contents=contents),tempfile.TemporaryDirectory() as d:
                def empty(args):
                    target=Path(args[-1]);target.mkdir();(target/'stage').write_bytes(contents);(target/'preflight.log').write_bytes(contents)
                    return result(args)
                capture={'retained':False}
                with self.assertRaises(ValueError):retain_diagnostics(CheckedCommands(FakeCommands([empty]),d),'owned',d,capture)
                self.assertFalse(capture['retained'])
                with self.assertRaises(ValueError):diagnostic_delete('volume',capture,{'Labels':{'webdock.recovery-fixture':'owner'}},'owner')

    def test_enospc_recording_cannot_mask_original_command_timeout(self):
        with tempfile.TemporaryDirectory() as d:
            original=subprocess.TimeoutExpired(['docker','wait','owned'],1,output=b'partial',stderr=b'private timeout')
            commands=CheckedCommands(FakeCommands([original]),d)
            with patch.object(custody,'_private_write',side_effect=OSError(errno.ENOSPC,'full fake disk')):
                with self.assertRaises(subprocess.TimeoutExpired) as seen:commands.call('wait','wait','owned')
            self.assertIs(seen.exception,original)
            self.assertEqual(commands.attempts[-1]['status'],'failed')
            self.assertTrue(commands.evidence_failures)

    def test_actual_cleanup_attempt_continues_when_exception_recording_is_full(self):
        source=Path(__file__).with_name('recovery-admission-component.py')
        tree=ast.parse(source.read_text())
        main=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='main')
        attempt=next(n for n in ast.walk(main) if isinstance(n,ast.FunctionDef) and n.name=='attempt')
        with tempfile.TemporaryDirectory() as d:
            failures=[];steps={};evidence=[];reached=[]
            commands=type('Commands',(),{'evidence_failures':evidence,'attempts':[]})()
            def cannot_record(*args,**kwargs):raise OSError(errno.ENOSPC,'private secret must not become fallback')
            namespace={**vars(custody),'failures':failures,'steps':steps,'base':Path(d),'commands':commands,'record_exception':cannot_record}
            exec(compile(ast.Module(body=[attempt],type_ignores=[]),str(source),'exec'),namespace)
            def fail():raise RuntimeError('original cleanup failure')
            try:
                namespace['attempt']('first',fail)
                namespace['attempt']('second',lambda:reached.append('second'))
            except OSError:pass
            self.assertEqual(reached,['second'])
            self.assertIn('first',failures)
            self.assertEqual(steps['first']['status'],'failed')
            self.assertTrue(evidence)

    def test_actual_final_report_failure_marks_memory_failed_with_nonsecret_fallback(self):
        source=Path(__file__).with_name('recovery-admission-component.py');tree=ast.parse(source.read_text())
        main=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='main')
        finalizer=next(n.finalbody for n in main.body if isinstance(n,ast.Try))
        start=next(i for i,n in enumerate(finalizer) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Subscript) and isinstance(t.value,ast.Name) and t.value.id=='report' and isinstance(t.slice,ast.Constant) and t.slice.value=='unresolvedAcquisitions' for t in n.targets))
        with tempfile.TemporaryDirectory() as d:
            report={'functionalPassed':True};evidence=[];output=io.StringIO()
            journal=type('Journal',(),{'unresolved':lambda self:[],'entries':{}})()
            commands=type('Commands',(),{'evidence_failures':evidence,'attempts':[]})()
            namespace={**vars(custody),'report':report,'acquisitions':journal,'capture':{},'primary_error':None,'failures':[],'steps':{},'base':Path(d),'commands':commands}
            acquisition_projection=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='unresolved_acquisitions')
            exec(compile(ast.Module(body=[acquisition_projection],type_ignores=[]),str(source),'exec'),namespace)
            failed=False
            with patch.object(Path,'write_text',side_effect=OSError(errno.ENOSPC,'NEVER PUBLIC SECRET')),patch.object(custody,'_private_write',side_effect=OSError(errno.ENOSPC,'NEVER PUBLIC SECRET')),contextlib.redirect_stdout(output),contextlib.redirect_stderr(output):
                try:exec(compile(ast.Module(body=finalizer[start:],type_ignores=[]),str(source),'exec'),namespace)
                except OSError:failed=True
            self.assertFalse(failed,'report evidence failure must not escape the safe finalizer')
            self.assertFalse(report['passed'])
            self.assertFalse(report['cleanup']['complete'])
            self.assertTrue(evidence)
            self.assertNotIn('NEVER PUBLIC SECRET',output.getvalue())
            self.assertIn('unavailable',output.getvalue())

    def test_all_owned_container_cleanup_commands_continue_when_every_record_write_fails(self):
        with tempfile.TemporaryDirectory() as d:
            entries=[{'name':'first','id':'a'*64},{'name':'second','id':'b'*64}]
            replies=[]
            for entry in entries:
                meta={'Id':entry['id'],'Name':'/'+entry['name'],'Config':{'Labels':{'webdock.recovery-fixture':'owner'}},'State':{'Running':False}}
                replies.extend([result([],0,json.dumps([meta]).encode()),result([]),result([],1,b'[]',('Error: No such object: '+entry['id']).encode())])
            fake=FakeCommands(replies);commands=CheckedCommands(fake,d)
            with patch.object(custody,'_private_write',side_effect=OSError(errno.ENOSPC,'PRIVATE ENOSPC DETAILS')):
                for entry in entries:cleanup_container(entry,'owner',commands)
            self.assertEqual([args for args in fake.emitted if args[0]=='rm'],[('rm','--volumes',entry['id']) for entry in entries])
            self.assertTrue(commands.evidence_failures)
            report={'passed':True,'cleanup':{'complete':True}};output=io.StringIO()
            with contextlib.redirect_stderr(output):
                self.assertFalse(custody.write_final_report(d,report,commands.evidence_failures))
            self.assertFalse(report['passed']);self.assertFalse(report['cleanup']['complete'])
            self.assertNotIn('PRIVATE ENOSPC DETAILS',output.getvalue())

    def test_unrecorded_absence_probe_cannot_start_acquisition(self):
        with tempfile.TemporaryDirectory() as d:
            fake=FakeCommands([absent(PAUSE)]);commands=CheckedCommands(fake,d)
            with patch.object(custody,'_private_write',side_effect=OSError(errno.ENOSPC,'full')):
                with self.assertRaises(Exception):acquire_image('pause',PAUSE,('pull',PAUSE),commands,Acquisitions(d),lambda value:None)
            self.assertEqual(fake.emitted,[('image','inspect',PAUSE)])
            self.assertTrue(commands.evidence_failures)

    def test_recorded_docker29_lowercase_absence_matches_exact_id_or_name(self):
        for reference in ['07755d302282019c20389329f7610641d19f0e44f066e62f76037766dbebe24f','Owned-Resource-A']:
            for prefix in ['error: no such object: ','Error: No such object: ','Error response from daemon: No such container: ']:
                with self.subTest(reference=reference,prefix=prefix):
                    self.assertIsNone(container_probe(reference,result([],1,b'[]\n',(prefix+reference+'\n').encode())))

    def test_container_prefix_compatibility_does_not_normalize_reference_or_unknown_error(self):
        reference='Owned-Resource-A'
        for rc,out,error in [
            (1,b'[]',b'error: no such object: owned-resource-a'),
            (1,b'[]',b'error: no such object: Owned-Resource-B'),
            (1,b'[]',b'error: no such object: Owned-Resource-A\npermission denied'),
            (1,b'[]',b'error: no such file or directory: Owned-Resource-A'),
            (1,b'[]',b'permission denied: Owned-Resource-A'),
            (1,b'[]',b'error: dial unix /var/run/docker.sock: no such file or directory'),
            (125,b'[]',b'error: no such object: Owned-Resource-A'),
            (1,b'incomplete',b'error: no such object: Owned-Resource-A'),
        ]:
            with self.subTest(error=error),self.assertRaises(ValueError):container_probe(reference,result([],rc,out,error))

if __name__=='__main__':unittest.main()
