import json
import os
from pathlib import Path
import tempfile
import unittest
import io
import subprocess
from types import SimpleNamespace
from unittest.mock import patch
from daemon import load_state, run_once, run_database_once

class DaemonTests(unittest.TestCase):
    def test_database_failure_diagnostic_only_contains_validated_identity_phase_and_class(self):
        from test_databases import packet
        p=packet();p['phase']='configure'
        state={'endpoint':'https://control.invalid','clusterID':p['clusterID'],'generation':p['generation']}
        failures=[RuntimeError('private-message-canary'),subprocess.TimeoutExpired(['private-command-canary'],60,output=b'private-output-canary',stderr=b'private-stderr-canary')]
        for failure in failures:
            stream=io.StringIO();completed=[]
            def request(origin,state,route,body):
                if route=='database-claim':return p
                if route=='database-complete':completed.append(body)
                return {'accepted':True}
            def execute(value,checkpoint):
                checkpoint(phase='configuring',cnpgUID='fixture')
                raise failure
            with self.subTest(exception=type(failure).__name__),patch('daemon.agent_request',side_effect=request),patch('databases.execute_database',side_effect=execute),patch('daemon.sys.stderr',stream):
                result=run_database_once(state,{'capabilities':{'databaseVersion':1}},lambda:None)
            self.assertEqual(result['outcome'],'needs-reconciliation')
            self.assertEqual(json.loads(stream.getvalue()),{'event':'database-execution-failed','operationID':p['operationID'],'phase':'configuring','exception':type(failure).__name__})
            self.assertNotIn('private-',stream.getvalue())
            self.assertEqual(completed[0]['outcome'],'needs-reconciliation')
            self.assertNotIn('error',completed[0])

    def test_slow_database_stage_refreshes_real_heartbeat_before_guard_and_completion(self):
        from test_databases import packet
        for slow in [False,True]:
            with self.subTest(slow=slow),tempfile.TemporaryDirectory() as directory:
                path=self.state(directory);p=packet();p['clusterID']='123';p['phase']='configure'
                clock=SimpleNamespace(value=0);beats=[];calls=[];collections=0
                def collect():
                    nonlocal collections
                    collections+=1
                    if collections>1:clock.value+=7
                    return {'nodes':[],'capabilities':{'databaseVersion':1}}
                def request(origin,state,route,body):
                    calls.append((route,clock.value))
                    if route=='heartbeat':
                        beats.append(body['sequence'])
                        self.assertEqual(body['generation'],1)
                        self.assertEqual(load_state(path)['sequence'],body['sequence'])
                    if route=='database-claim':return p
                    if route in ['database-guard','database-complete']:
                        last=max(value for name,value in calls if name=='heartbeat')
                        self.assertLess(clock.value-last,20)
                    if route=='claim':return None
                    return {'valid':True}
                def execute(value,checkpoint):
                    checkpoint(phase='configuring',cnpgUID='fixture')
                    clock.value=20 if slow else 5
                    checkpoint()
                    # A seven-second heartbeat must reset freshness after success, not before it.
                    clock.value=40 if slow else 10
                    checkpoint()
                    clock.value=48 if slow else 15
                    return {'state':'ready'}
                with patch('daemon.time.monotonic',side_effect=lambda:clock.value),patch('daemon.collect_observation',side_effect=collect),patch('daemon.agent_request',side_effect=request),patch('databases.execute_database',side_effect=execute),patch('daemon.run_template_once',return_value=None):
                    result=run_once(path)
                self.assertEqual(result['outcome'],'succeeded')
                self.assertEqual(beats,[1,2,3] if slow else [1])
                self.assertEqual([value for route,value in calls if route=='database-guard'],[27,40] if slow else [5,10])
                self.assertEqual(load_state(path)['generation'],1)
                self.assertEqual(load_state(path)['credential'],'a'*43)

    def test_failed_observation_refresh_blocks_next_effect_and_completion(self):
        from test_databases import packet
        with tempfile.TemporaryDirectory() as directory:
            path=self.state(directory);p=packet();p['clusterID']='123';p['phase']='configure'
            clock=SimpleNamespace(value=0);calls=[];effects=[];stream=io.StringIO()
            def request(origin,state,route,body):
                calls.append(route)
                if route=='database-claim':return p
                return {'valid':True}
            def execute(value,checkpoint):
                clock.value=21;checkpoint();effects.append('sql')
            observed={'nodes':[],'capabilities':{'databaseVersion':1}}
            with patch('daemon.time.monotonic',side_effect=lambda:clock.value),patch('daemon.collect_observation',side_effect=[observed,RuntimeError('private-NotReady-detail')]) as collect,patch('daemon.agent_request',side_effect=request),patch('databases.execute_database',side_effect=execute),patch('daemon.sys.stderr',stream):
                with self.assertRaises(RuntimeError):run_once(path)
            self.assertEqual(collect.call_count,2)
            self.assertEqual(effects,[])
            self.assertNotIn('database-guard',calls);self.assertNotIn('database-complete',calls)
            self.assertEqual(load_state(path)['sequence'],1)
            self.assertNotIn('private-',stream.getvalue())

    def state(self,directory):
        path=Path(directory)/'connection.json'
        path.write_text(json.dumps({'endpoint':'https://auth.webdock.dev','clusterID':'123','credential':'a'*43,'generation':1,'sequence':0}))
        path.chmod(0o600)
        return path
    def test_private_https_credentials_and_sequence_survive_idle_cycle(self):
        with tempfile.TemporaryDirectory() as directory:
            path=self.state(directory)
            def request(origin,state,route,body):
                self.assertEqual(json.loads(path.read_text())['sequence'],1)
                if route=='heartbeat':self.assertEqual(body['sequence'],1)
                return None
            with patch('daemon.collect_observation',return_value={'version':'fixture','nodes':[]}),patch('daemon.agent_request',side_effect=request):
                run_once(path)
            self.assertEqual(load_state(path)['sequence'],1)
            path.chmod(0o644)
            with self.assertRaises(ValueError):load_state(path)
    def test_pending_template_cycles_do_not_starve_ordinary_or_mail_lanes(self):
        with tempfile.TemporaryDirectory() as directory:
            path=self.state(directory);calls=[]
            def request(origin,state,route,body):calls.append(route);return None
            with patch('daemon.collect_observation',return_value={'nodes':[]}),patch('daemon.agent_request',side_effect=request),patch('daemon.run_template_once',return_value={'event':'template-operation','id':'77','outcome':'pending'}) as template,patch('daemon.run_mail_once',return_value=None) as mail,patch('daemon.run_mail_configuration',return_value=None):
                for _ in range(3):run_once(path)
            self.assertEqual(calls.count('claim'),3);self.assertEqual(template.call_count,3);self.assertEqual(mail.call_count,3)

    def test_template_claim_failure_does_not_block_ordinary_operation(self):
        with tempfile.TemporaryDirectory() as directory:
            path=self.state(directory);approval=Path(directory)/'templates.json';approval.write_text('{}');calls=[]
            def request(origin,state,route,body):
                calls.append(route)
                if route=='template-claim':raise PermissionError('revoked template actor')
                if route=='claim':return {'id':'456','generation':1,'desired':{'clusterID':'123','operationID':'456','generation':1}}
            with patch.dict(os.environ,{'SPITZLI_APP_TEMPLATES':str(approval)}),patch('daemon.collect_observation',return_value={'nodes':[]}),patch('daemon.agent_request',side_effect=request),patch('daemon.execute_local',return_value={'outcome':'succeeded','proof':{}}):
                result=run_once(path)
            self.assertEqual(result['event'],'operation');self.assertIn('complete',calls)

    def test_foreign_cluster_job_never_executes(self):
        with tempfile.TemporaryDirectory() as directory:
            path=self.state(directory);results=[]
            def request(origin,state,route,body):
                if route=='claim':return {'id':'456','generation':1,'desired':{'clusterID':'999','operationID':'456','generation':1}}
                if route=='complete':results.append(body)
            with patch('daemon.collect_observation',return_value={'version':'fixture','nodes':[]}),patch('daemon.agent_request',side_effect=request),patch('daemon.execute_local') as execute:
                run_once(path)
            execute.assert_not_called()
            self.assertEqual(results[0]['outcome'],'failed')
            self.assertEqual(results[0]['id'],'456')
    def test_http_and_credential_in_url_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path=self.state(directory)
            for endpoint in ['http://auth.webdock.dev','https://user:secret@auth.webdock.dev','https://auth.webdock.dev/wrong']:
                state=json.loads(path.read_text());state['endpoint']=endpoint;path.write_text(json.dumps(state))
                with self.assertRaises(ValueError):load_state(path)
    def test_only_explicit_webdock_credential_rejection_stops_the_agent(self):
        from io import BytesIO
        from urllib.error import HTTPError
        from daemon import authentication_revoked
        for code,body,expected in [(403,b'<html>Temporary provider protection</html>',False),(403,b'{"error":"Own infrastructure is disabled."}',False),(401,b'{"error":"Cluster authentication failed."}',True),(503,b'{"error":"Unavailable"}',False)]:
            error=HTTPError('https://auth.webdock.dev',code,'failure',{},BytesIO(body))
            self.assertEqual(authentication_revoked(error),expected)
            error.close()

    def test_mail_job_checkpoints_before_reporting_completion(self):
        from test_mail_executor import MailExecutorTests
        packet = MailExecutorTests().packet(); packet['clusterID'] = '123'
        with tempfile.TemporaryDirectory() as directory:
            path = self.state(directory); calls = []
            def request(origin, state, route, body):
                calls.append((route, body))
                if route == 'mail-claim': return packet
                if route == 'mail-checkpoint': return {'leaseUntil': packet['leaseUntil']}
            def execute(value, checkpoint):
                checkpoint({'username': 'admin', 'password': 'fixture'})
                return {'namespace': 'wd-mail-123', 'revision': 1, 'running': True, 'edition': 'community', 'recoveryDisabled': True}
            observed = {'nodes': [], 'capabilities': {'nativeMail': {'version': 1, 'image': packet['image']}}}
            with patch('daemon.collect_observation', return_value=observed), patch('daemon.agent_request', side_effect=request), patch('mail_executor.execute_mail', side_effect=execute) as executor:
                result = run_once(path)
            executor.assert_called_once()
            self.assertEqual(result['event'], 'mail-operation')
            routes = [route for route, _ in calls]
            self.assertLess(routes.index('mail-checkpoint'), routes.index('mail-complete'))
            complete = next(body for route, body in calls if route == 'mail-complete')
            self.assertEqual(complete['outcome'], 'succeeded')
            self.assertNotIn('fixture', str(complete))

    def test_foreign_mail_packet_never_executes(self):
        from test_mail_executor import MailExecutorTests
        packet = MailExecutorTests().packet()
        with tempfile.TemporaryDirectory() as directory:
            path = self.state(directory)
            observed = {'nodes': [], 'capabilities': {'nativeMail': {'version': 1, 'image': packet['image']}}}
            def request(origin, state, route, body):
                if route == 'mail-claim': return packet
            with patch('daemon.collect_observation', return_value=observed), patch('daemon.agent_request', side_effect=request), patch('mail_executor.execute_mail') as executor:
                with self.assertRaises(ValueError): run_once(path)
            executor.assert_not_called()
