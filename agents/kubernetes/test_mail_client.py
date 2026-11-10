import json
import smtplib
import ssl
from urllib.parse import urlsplit
import os
import subprocess
import time
import unittest
import uuid
from mail_client import MailClient
from urllib.request import Request


@unittest.skipUnless(os.environ.get('SPITZLI_MAIL_DOCKER_TEST') == '1', 'Explicit local Docker integration test')
class MailClientIntegrationTests(unittest.TestCase):
    def test_restricted_bootstrap_high_ports_and_permanent_credentials(self):
        endpoint = os.environ.get('DOCKER_HOST')
        if not endpoint or not endpoint.startswith('unix:///'):
            self.fail('An explicit local Docker socket is required')
        name = 'webdock-mail-restricted-test-' + uuid.uuid4().hex[:12]
        image = 'webdock.local/mail-stalwart:0.16.25-restricted'
        env = dict(os.environ); env.pop('DOCKER_CONTEXT', None)
        def docker(*args):
            result = subprocess.run(['docker', '--host', endpoint, *args], env=env, capture_output=True, text=True, timeout=40)
            if result.returncode:
                raise AssertionError('Local Docker test operation failed: ' + args[0])
            return result.stdout
        def start(recovery):
            docker('run', '-d', '--name', name, '--network', name, '--user', '65532:65532', '--cap-drop', 'ALL',
                   '--security-opt', 'no-new-privileges', '--read-only', '--memory', '1g', '--cpus', '1',
                   '--tmpfs', '/tmp', '--mount', 'type=volume,src=' + name + ',dst=/data',
                   '--env', 'STALWART_PUBLIC_URL=https://mail-123.example.com',
                   *(['--env', 'STALWART_RECOVERY_MODE=1', '--env', 'STALWART_RECOVERY_ADMIN=admin:local-fixture-only'] if recovery else []), image)
            address = json.loads(docker('inspect', name))[0]['NetworkSettings']['Networks'][name]['IPAddress']
            return 'http://' + address + ':8080'
        def eventually(call):
            deadline = time.monotonic() + 25
            while True:
                try: return call()
                except Exception:
                    if time.monotonic() >= deadline: raise
                    time.sleep(.3)
        try:
            docker('network', 'create', '--internal', name)
            docker('volume', 'create', name)
            docker('run', '--rm', '--network', 'none', '--user', '0', '--mount', 'type=volume,src=' + name + ',dst=/data',
                   '--entrypoint', 'chown', image, '65532:65532', '/data')
            client = MailClient(start(True), 'admin', 'local-fixture-only')
            eventually(lambda: client.call('x:Bootstrap/get', {'ids': ['singleton']}))
            with client.opener.open(Request(client.url + '/healthz/live'), timeout=5) as response:
                self.assertEqual(response.status, 200)
            credentials = client.bootstrap('mail-123.example.com')
            docker('restart', name)
            eventually(lambda: client.call('x:NetworkListener/get', {'ids': None}))
            client.configure_listeners()
            docker('rm', '-f', name)
            permanent = MailClient(start(False), **credentials)
            eventually(permanent.verify)
            self.assertRegex(permanent.submission_certificate(), r'^[a-f0-9]{64}$')
            with permanent.opener.open(Request(permanent.url + '/healthz/live'), timeout=5) as response:
                self.assertEqual(response.status, 200)
            rows = permanent.call('x:NetworkListener/get', {'ids': None})['list']
            self.assertEqual({int(bind.rsplit(':', 1)[1]) for row in rows for bind in row['bind']}, {8080, 2525, 2465, 1993})
            with smtplib.SMTP_SSL(urlsplit(permanent.url).hostname, 2465, timeout=5, context=ssl._create_unverified_context()) as smtp:
                self.assertEqual(smtp.ehlo()[0], 250)
            self.assertFalse(any(v.startswith('STALWART_RECOVERY') for v in json.loads(docker('inspect', name))[0]['Config']['Env']))
            with self.assertRaises(ValueError):
                MailClient(permanent.url, 'admin', 'local-fixture-only').verify()
            from mail_configuration import apply_configuration
            from test_mail_configuration import packet
            config = packet()
            proof = apply_configuration(permanent, config, lambda: None)
            self.assertTrue(proof['dkim']['11'][0]['selector'].startswith('webdock-rsa-'))
            self.assertRegex(proof['dkim']['11'][0]['publicKey'], r'^[A-Za-z0-9+/]+={0,2}$')
            self.assertEqual(apply_configuration(permanent, config, lambda: None), proof)
            identity = config['identities'][0]
            member = MailClient(permanent.url, identity['username'], identity['password'])
            def jmap(method, account_id, capabilities):
                return member.request('/jmap', {'using': ['urn:ietf:params:jmap:core', *capabilities],
                    'methodCalls': [[method, {'accountId': account_id, 'ids': None}, 'fixture']]})['methodResponses'][0][0]
            self.assertEqual(jmap('Mailbox/get', proof['mailboxes']['22'], ['urn:ietf:params:jmap:mail']), 'Mailbox/get')
            folders=member.request('/jmap',{'using':['urn:ietf:params:jmap:core','urn:ietf:params:jmap:mail'],'methodCalls':[['Mailbox/get',{'accountId':proof['mailboxes']['22'],'ids':None},'folders']]})['methodResponses'][0][1]['list']
            self.assertTrue({'inbox','drafts','sent'}.issubset({folder.get('role') for folder in folders}))
            self.assertEqual(jmap('Calendar/get', proof['identities']['33'], ['urn:ietf:params:jmap:calendars']), 'error')
            denied = member.request('/jmap', {'using': ['urn:ietf:params:jmap:core', 'urn:ietf:params:jmap:submission'],
                'methodCalls': [['EmailSubmission/set', {'accountId': proof['mailboxes']['22'], 'create': {'forged': {'emailId': 'invalid', 'identityId': 'invalid'}}}, 'blocked']]})['methodResponses'][0]
            self.assertEqual(denied[0], 'error')
            self.assertEqual(denied[1]['type'], 'forbidden')
            with smtplib.SMTP_SSL(urlsplit(permanent.url).hostname, 2465, timeout=5, context=ssl._create_unverified_context()) as smtp:
                with self.assertRaises(smtplib.SMTPAuthenticationError):
                    smtp.login(identity['username'], identity['password'])
            identity['enabled'] = False; identity['mailboxIds'] = []
            apply_configuration(permanent, config, lambda: None)
            with self.assertRaises(ValueError):
                jmap('Mailbox/get', proof['mailboxes']['22'], ['urn:ietf:params:jmap:mail'])
            from mail_submission import configure_submission
            submission={'customerID':'123','hostname':'mail-123.example.com','username':'wd-submit@mail-123.example.com','password':uuid.uuid4().hex,'hookToken':'wd_hook_123_'+'t'*43,'relayHost':'pro.eu.turbo-smtp.com','relayUsername':'fixture-key','relayPassword':'fixture-secret','transportRevision':'1'}
            submitted=configure_submission(permanent,submission,lambda:None)
            self.assertEqual(configure_submission(permanent,submission,lambda:None),submitted)
            submission['resourceIDs']=submitted
            self.assertEqual(configure_submission(permanent,submission,lambda:None),submitted)
            service=permanent.call('x:Account/get',{'ids':[submitted['accountID']]})['list'][0]
            self.assertEqual(service['permissions']['enabledPermissions'],{'authenticate':True,'emailSend':True})
            hook=permanent.call('x:MtaHook/get',{'ids':[submitted['hookID']]})['list'][0]
            self.assertTrue(hook['tempFailOnError'])
            route=permanent.call('x:MtaRoute/get',{'ids':[submitted['routeID']]})['list'][0]
            self.assertTrue(route['implicitTls']);self.assertFalse(route['allowInvalidCerts'])
            docker('restart',name);eventually(permanent.verify)
            fingerprint=permanent.submission_certificate()
            docker('restart',name);eventually(permanent.verify)
            self.assertEqual(permanent.submission_certificate(),fingerprint)


        finally:
            for args in [('rm', '-f', name), ('volume', 'rm', name), ('network', 'rm', name)]:
                subprocess.run(['docker', '--host', endpoint, *args], env=env, capture_output=True, timeout=30)


if __name__ == '__main__':
    unittest.main()
