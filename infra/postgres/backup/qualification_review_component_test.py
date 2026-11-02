#!/usr/bin/env python3
"""Runner-only fake regressions. No Docker, PostgreSQL, listener or private input."""
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
sys.dont_write_bytecode = True
spec=importlib.util.spec_from_file_location('pg_review_runner',Path(__file__).with_name('qualification_review_component.py'))
runner=importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)
CID='a'*64

class Fake:
    def __init__(self,name,mode='success'):
        self.name=name;self.mode=mode;self.started=False;self.removed=False;self.migration=False;self.calls=[];self.create_attempted=False;self.host_attempts=0;self.migration_host_attempts=None;self.owner_probe_failed=False
    def __call__(self,args,**kwargs):
        self.calls.append(args)
        out=b'';err=b'';code=0
        if args[:2]==['docker','run']:
            self.create_attempted=True
            if self.mode=='create_unknown' and self.create_attempted:return subprocess.CompletedProcess(args,125,b'',b'Cannot connect to the Docker daemon')
            self.started=True;out=((CID+'\n') if self.mode!='invalid_cid' else 'not-one-CID64\n').encode()
        elif args[:2]==['docker','inspect']:
            ref=args[-1]
            if self.mode=='no_health' and '--format' in args and '.State.Health' in args[args.index('--format')+1]:return subprocess.CompletedProcess(args,1,b'',b'template parsing error: map has no entry for key Health')
            if self.mode=='owner_probe_once' and self.started and not self.owner_probe_failed:
                self.owner_probe_failed=True;return subprocess.CompletedProcess(args,1,b'',b'Cannot connect to the Docker daemon')
            if self.mode=='create_unknown' and self.create_attempted:return subprocess.CompletedProcess(args,1,b'',b'Cannot connect to the Docker daemon')
            if self.mode=='cleanup_timeout' and self.migration:raise subprocess.TimeoutExpired(args,10,output=b'partial cleanup stdout',stderr=b'socket timeout')
            if self.removed and self.mode=='absence_unknown':return subprocess.CompletedProcess(args,1,b'',b'Cannot connect to the Docker daemon')
            if not self.started or self.removed:
                return subprocess.CompletedProcess(args,1,b'',f'Error response from daemon: No such container: {ref}\n'.encode())
            meta={'Id':CID,'Image':runner.IMAGE,'Owner':self.name,'Mounts':[], 'Config':{'Labels':{'webdock.pg-review-owner':self.name}}}
            if '--format' in args and 'StartedAt' in args[args.index('--format')+1]:meta={'Id':CID,'Status':'running','Running':True,'StartedAt':'fixture-start','FinishedAt':'','HealthStatus':None}
            if self.mode=='foreign_owner' and 'Config' in meta:meta['Owner']='foreign';meta['Config']['Labels']['webdock.pg-review-owner']='foreign'
            if self.mode=='volume_unknown' and 'Mounts' in meta:meta['Mounts']=[{'Type':'volume','Name':'b'*64}]
            out=json.dumps(meta if '--format' in args else [meta]).encode()
        elif args[:2]==['docker','rm']:
            if self.mode=='cleanup_timeout':raise subprocess.TimeoutExpired(args,15,output=b'partial removal stdout',stderr=b'socket timeout')
            self.removed=True;out=(CID+'\n').encode()
        elif args[:3]==['docker','volume','inspect']:
            return subprocess.CompletedProcess(args,1,b'',b'Cannot connect to the Docker daemon')
        elif args[:2]==['docker','logs']:
            if self.mode=='tcp_timeout_logs_fail':raise subprocess.TimeoutExpired(args,10,output=b'partial server log',stderr=b'log timeout')
            out=b'owned temporary init server then final server startup'
        elif args[:2]==['docker','exec']:
            sql=kwargs.get('input',b'').decode()
            if 'rust_journal' in sql:out=json.dumps({'1':{'checksum':'fixture-checksum','success':True}}).encode()
        elif Path(args[0]).name=='webdock-migrate':
            self.migration=True;self.migration_host_attempts=self.host_attempts
            if self.mode=='interrupted':raise KeyboardInterrupt('fake controlled interruption')
            if self.mode in ['migration_fail','cleanup_timeout']:return subprocess.CompletedProcess(args,9,b'',b'primary migration failure')
        elif Path(args[0]).name=='host-psql':
            self.host_attempts+=1
            if self.mode in ['tcp_timeout','tcp_timeout_logs_fail'] or (self.mode=='socket_only' and self.host_attempts<3):return subprocess.CompletedProcess(args,2,b'',b'connection refused')
            if self.mode=='bad_select' and self.host_attempts<3:return subprocess.CompletedProcess(args,0,b'0\n',b'')
            out=b'1\n'
        elif Path(args[0]).name=='database-primitives':
            config=json.loads(Path(kwargs['env']['WEBDOCK_PG_REVIEW_COMPONENT_INPUT']).read_text())
            code=101 if config['baseline'] else 0
            out=('test result: FAILED. 0 passed; 5 failed' if config['baseline'] else 'test result: ok. 5 passed; 0 failed').encode()
        return subprocess.CompletedProcess(args,code,out,err)

class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.addCleanup(self.temp.cleanup)
        parent=Path(self.temp.name)/'.runtime-test';parent.mkdir(mode=0o700)
        self.directory=parent/'pg-restore-review1-abcdef123456';self.directory.mkdir(mode=0o700)
        candidate=self.directory/'candidate';candidate.mkdir(mode=0o700)
        sources=self.directory/'sources';sources.mkdir(mode=0o700)
        binaries={}
        for name in ['database-primitives','webdock-migrate','host-psql']:
            path=candidate/name;path.write_text('fake '+name);path.chmod(0o700);binaries[name]={'sha256':runner.sha(path),'bytes':path.stat().st_size}
        for name in ['baseline-0056.draft.sql','fixed-0056.draft.sql']:
            path=self.directory/name;path.write_text('SELECT '+runner.BILLING_CASE+';\n');path.chmod(0o600)
        self.pin={'owner':self.directory.name,'image':runner.IMAGE,'port':0,'filter':runner.FILTER,'scope':'fake runner only',
                  'sourceSHA256':{},'nativeMigrations':{'crates/persistence/migrations/0001_ids.sql':{'sha384':'fixture-checksum'}},
                  'binaries':binaries,'fixedDraftSHA256':runner.sha(self.directory/'fixed-0056.draft.sql'),
                  'baselineDraftSHA256':runner.sha(self.directory/'baseline-0056.draft.sql')}
        self.pin['baselineSyntaxOverlay']=runner.write_baseline_syntax(self.directory)
        runner.write(self.directory/'pin.json',self.pin)
    def execute(self,mode='success',write_failure=False):
        fake=Fake(self.directory.name,mode)
        original_write=runner.write
        def writing(path,value):
            if write_failure and path.name=='result.json':raise OSError('simulated result evidence write failure')
            return original_write(path,value)
        clock=[0.0]
        from contextlib import ExitStack
        with ExitStack() as stack:
            stack.enter_context(patch.object(runner.subprocess,'run',fake));stack.enter_context(patch.object(runner,'check_port',lambda port:None,create=True));stack.enter_context(patch.object(runner,'write',writing))
            if mode.startswith('tcp_timeout'):
                stack.enter_context(patch.object(runner.time,'monotonic',lambda:clock[0]));stack.enter_context(patch.object(runner.time,'sleep',lambda seconds:clock.__setitem__(0,clock[0]+15)))
            runner.execute(self.directory)
        return fake
    def result(self):return json.loads((self.directory/'result.json').read_text())
    def test_journal_dictionary_survives_test_exit_loop_and_credentials_are_removed(self):
        self.execute()
        self.assertEqual(self.result()['status'],'component-only-green-after-five-baseline-reds')
        self.assertFalse((self.directory/'private').exists())
    def test_cleanup_uses_verified_returned_cid_not_name(self):
        fake=Fake(self.directory.name)
        with patch.object(runner.subprocess,'run',fake),patch.object(runner,'check_port',lambda port:None,create=True):
            try:runner.execute(self.directory)
            except BaseException:pass
        removals=[x for x in fake.calls if x[:2]==['docker','rm']]
        self.assertTrue(removals)
        self.assertEqual(removals[0][-1],CID)
    def test_primary_failure_survives_cleanup_timeout_and_records_unknown(self):
        with self.assertRaises(AssertionError):self.execute('cleanup_timeout')
        result=self.result()
        self.assertIsNone(result['cleanup']['ownedAbsent'])
        self.assertTrue(result['cleanup']['obligations'])
        self.assertFalse((self.directory/'private').exists())
        self.assertIn('socket timeout',''.join(p.read_text() for p in self.directory.glob('*.log')))
    def test_unknown_daemon_after_remove_is_not_absence(self):
        with self.assertRaises(Exception):self.execute('absence_unknown')
        self.assertIsNone(self.result()['cleanup']['ownedAbsent'])
        self.assertFalse(self.result()['cleanup']['complete'])
        self.assertFalse((self.directory/'private').exists())
    def test_failed_create_unknown_is_not_removed_by_guessed_name(self):
        fake=Fake(self.directory.name,'create_unknown')
        with patch.object(runner.subprocess,'run',fake),patch.object(runner,'check_port',lambda port:None,create=True):
            with self.assertRaises(Exception):runner.execute(self.directory)
        self.assertFalse(any(x[:2]==['docker','rm'] for x in fake.calls))
        self.assertIsNone(self.result()['cleanup']['ownedAbsent'])
        self.assertTrue(self.result()['cleanup']['obligations'])
        self.assertFalse((self.directory/'private').exists())
    def test_evidence_write_failure_does_not_mask_primary_and_erases_credentials(self):
        with self.assertRaises(AssertionError):self.execute('migration_fail',write_failure=True)
        self.assertFalse((self.directory/'private').exists())
    def test_bad_pin_mode_prevents_any_process_or_secret_creation(self):
        (self.directory/'pin.json').chmod(0o644)
        fake=Fake(self.directory.name)
        with patch.object(runner.subprocess,'run',fake):
            with self.assertRaises(PermissionError):runner.execute(self.directory)
        self.assertEqual(fake.calls,[]);self.assertFalse((self.directory/'private').exists())
    def test_pin_symlink_prevents_any_process_or_secret_creation(self):
        pin=self.directory/'pin.json';original=self.directory/'actual-pin.json';pin.rename(original);pin.symlink_to(original)
        fake=Fake(self.directory.name)
        with patch.object(runner.subprocess,'run',fake):
            with self.assertRaises(PermissionError):runner.execute(self.directory)
        self.assertEqual(fake.calls,[]);self.assertFalse((self.directory/'private').exists())
    def test_candidate_uid_mismatch_prevents_processes_and_secrets(self):
        fake=Fake(self.directory.name)
        with patch.object(runner.os,'getuid',return_value=os.getuid()+1),patch.object(runner.subprocess,'run',fake):
            with self.assertRaises(PermissionError):runner.execute(self.directory)
        self.assertEqual(fake.calls,[]);self.assertFalse((self.directory/'private').exists())
    def test_malformed_create_receipt_never_removes_a_name(self):
        fake=Fake(self.directory.name,'invalid_cid')
        with patch.object(runner.subprocess,'run',fake),patch.object(runner,'check_port',lambda port:None,create=True):
            with self.assertRaises(AssertionError):runner.execute(self.directory)
        self.assertFalse(any(x[:2]==['docker','rm'] for x in fake.calls))
        self.assertTrue(self.result()['cleanup']['obligations']);self.assertFalse((self.directory/'private').exists())
    def test_foreign_owner_never_gets_removed(self):
        fake=Fake(self.directory.name,'foreign_owner')
        with patch.object(runner.subprocess,'run',fake),patch.object(runner,'check_port',lambda port:None,create=True):
            with self.assertRaises(AssertionError):runner.execute(self.directory)
        self.assertFalse(any(x[:2]==['docker','rm'] for x in fake.calls))
        self.assertTrue(self.result()['cleanup']['obligations']);self.assertFalse((self.directory/'private').exists())
    def test_volume_daemon_error_is_not_volume_absence(self):
        with self.assertRaises(Exception):self.execute('volume_unknown')
        self.assertTrue(self.result()['cleanup']['ownedAbsent'])
        self.assertIsNone(self.result()['cleanup']['volumeAbsence']['b'*64])
        self.assertFalse(self.result()['cleanup']['complete']);self.assertFalse((self.directory/'private').exists())
    def test_interrupt_cleans_credentials_and_preserves_interrupt(self):
        with self.assertRaises(KeyboardInterrupt):self.execute('interrupted')
        self.assertTrue(self.result()['cleanup']['ownedAbsent']);self.assertFalse((self.directory/'private').exists())
    def test_candidate_directory_mode_prevents_processes_and_secrets(self):
        self.directory.chmod(0o755);fake=Fake(self.directory.name)
        with patch.object(runner.subprocess,'run',fake):
            with self.assertRaises(PermissionError):runner.execute(self.directory)
        self.assertEqual(fake.calls,[]);self.assertFalse((self.directory/'private').exists())
    def test_candidate_directory_symlink_prevents_processes_and_secrets(self):
        actual=self.directory.with_name('saved-candidate');self.directory.rename(actual);self.directory.symlink_to(actual,target_is_directory=True)
        fake=Fake(self.directory.name)
        with patch.object(runner.subprocess,'run',fake):
            with self.assertRaises(PermissionError):runner.execute(self.directory)
        self.assertEqual(fake.calls,[]);self.assertFalse((actual/'private').exists())
    def test_exact_absence_supports_docker29_case_without_folding_reference(self):
        for prefix in ['Error: No such object: ','error: no such object: ','Error response from daemon: No such container: ']:
            reply=subprocess.CompletedProcess([],1,'[]',prefix+'CaseSensitiveRef')
            self.assertTrue(runner.absent_reply(reply,'CaseSensitiveRef'))
            self.assertFalse(runner.absent_reply(reply,'casesensitiveref'))
        for stderr in ['Cannot connect to Docker daemon','error: no such object: CaseSensitiveRef extra','socket error: no such object: CaseSensitiveRef']:
            self.assertFalse(runner.absent_reply(subprocess.CompletedProcess([],1,'[]',stderr),'CaseSensitiveRef'))
        self.assertFalse(runner.absent_reply(subprocess.CompletedProcess([],2,'[]','error: no such object: CaseSensitiveRef'),'CaseSensitiveRef'))
        self.assertTrue(runner.absent_reply(subprocess.CompletedProcess([],1,'[]','error: no such volume: ExactVolume'),'ExactVolume',volume=True))
        self.assertTrue(runner.absent_reply(subprocess.CompletedProcess([],1,'[]','error response from daemon: get ExactVolume: no such volume'),'ExactVolume',volume=True))
        self.assertFalse(runner.absent_reply(subprocess.CompletedProcess([],1,'[]','Cannot connect to Docker daemon: no such volume'),'ExactVolume',volume=True))
    def test_unix_socket_ready_does_not_skip_final_host_tcp_login(self):
        fake=self.execute('socket_only')
        self.assertEqual(fake.migration_host_attempts,3)
        probes=[args for args in fake.calls if Path(args[0]).name=='host-psql']
        self.assertTrue(probes)
        self.assertTrue(all('-X' in args and '-w' in args and '127.0.0.1' in args and 'SELECT 1' in args for args in probes))
    def test_zero_exit_without_select_one_does_not_open_migration_gate(self):
        fake=self.execute('bad_select');self.assertEqual(fake.migration_host_attempts,3)
    def test_gate_timeout_captures_server_logs_before_cid_removal(self):
        fake=Fake(self.directory.name,'tcp_timeout');clock=[0.0]
        with patch.object(runner.subprocess,'run',fake),patch.object(runner,'check_port',lambda port:None,create=True),patch.object(runner.time,'monotonic',lambda:clock[0]),patch.object(runner.time,'sleep',lambda seconds:clock.__setitem__(0,clock[0]+15)):
            with self.assertRaises(TimeoutError):runner.execute(self.directory)
        self.assertFalse(any(Path(args[0]).name=='webdock-migrate' for args in fake.calls))
        logs=next(n for n,args in enumerate(fake.calls) if args[:2]==['docker','logs'])
        removal=next(n for n,args in enumerate(fake.calls) if args[:2]==['docker','rm'])
        self.assertLess(logs,removal);self.assertEqual(fake.calls[removal][-1],CID)
        self.assertFalse((self.directory/'private').exists())
        self.assertIn('server-diagnostics',self.result())
    def test_log_capture_failure_preserves_gate_timeout_and_cleanup(self):
        with self.assertRaises(TimeoutError):self.execute('tcp_timeout_logs_fail')
        self.assertTrue(self.result()['cleanup']['ownedAbsent']);self.assertFalse((self.directory/'private').exists())
    def test_diagnostic_collector_failure_preserves_primary_and_cleanup(self):
        with patch.object(runner,'capture_server_diagnostics',side_effect=RuntimeError('collector failure')):
            with self.assertRaises(AssertionError):self.execute('migration_fail')
        self.assertTrue(self.result()['cleanup']['ownedAbsent']);self.assertFalse((self.directory/'private').exists())
    def test_missing_pinned_host_client_is_rejected_before_secrets(self):
        self.pin['binaries'].pop('host-psql');(self.directory/'pin.json').unlink();runner.write(self.directory/'pin.json',self.pin)
        fake=Fake(self.directory.name)
        with patch.object(runner.subprocess,'run',fake):
            with self.assertRaises(PermissionError):runner.execute(self.directory)
        self.assertEqual(fake.calls,[]);self.assertFalse((self.directory/'private').exists())
    def test_recovered_ownership_captures_server_logs_before_removal(self):
        fake=Fake(self.directory.name,'owner_probe_once')
        with patch.object(runner.subprocess,'run',fake),patch.object(runner,'check_port',lambda port:None,create=True):
            with self.assertRaises(AssertionError):runner.execute(self.directory)
        logs=next(n for n,args in enumerate(fake.calls) if args[:2]==['docker','logs'])
        removal=next(n for n,args in enumerate(fake.calls) if args[:2]==['docker','rm'])
        self.assertLess(logs,removal);self.assertTrue(self.result()['cleanup']['ownedAbsent'])
        self.assertFalse((self.directory/'private').exists())
    def test_missing_optional_health_still_captures_whitelisted_state(self):
        fake=self.execute('no_health')
        self.assertTrue(self.result()['server-diagnostics']['complete'])
        state_args=next(args for args in fake.calls if args[:2]==['docker','inspect'] and '--format' in args and 'StartedAt' in args[args.index('--format')+1])
        template=state_args[state_args.index('--format')+1]
        self.assertNotIn('.Config.Env',template);self.assertNotIn('{{json .State}}',template)
    def test_syntax_normalization_is_one_parentheses_only_overlay(self):
        original=('SELECT '+runner.BILLING_CASE+';\n').encode();normalized,diff=runner.baseline_syntax(original)
        self.assertEqual(normalized,('SELECT ('+runner.BILLING_CASE+');\n').encode())
        self.assertEqual(len(normalized),len(original)+2);self.assertTrue(diff)
        for invalid in [b'SELECT 1;',original+original,normalized]:
            with self.assertRaises(ValueError):runner.baseline_syntax(invalid)
    def test_overlay_tampering_rejected_before_process_or_credentials(self):
        overlay=self.directory/'baseline-0056.syntax-normalized.draft.sql';overlay.write_text(overlay.read_text()+'-- extra change')
        fake=Fake(self.directory.name)
        with patch.object(runner.subprocess,'run',fake):
            with self.assertRaises(PermissionError):runner.execute(self.directory)
        self.assertEqual(fake.calls,[]);self.assertFalse((self.directory/'private').exists())

if __name__=='__main__':unittest.main()
