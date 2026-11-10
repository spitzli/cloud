import copy
import unittest
from datetime import datetime, timezone, timedelta
from mail_executor import mail_manifests, validate_mail_packet, mail_gateway_policy


class MailExecutorTests(unittest.TestCase):
    def packet(self):
        return {
            'version': 1, 'customerID': '123', 'clusterID': '789', 'operationID': '456',
            'revision': 1, 'generation': 1, 'leaseToken': 'a' * 43,
            'leaseUntil': (datetime.now(timezone.utc) + timedelta(seconds=100)).isoformat(),
            'action': 'ensure', 'hostname': 'mail-123.mail.webdock.dev',
            'image': 'webdock.local/mail-stalwart@sha256:' + 'a' * 64,
            'resources': {'cpuMillicores': 500, 'memoryBytes': 1073741824, 'ephemeralBytes': 268435456, 'volumeBytes': 10737418240},
            'credentials': {'bootstrapPassword': 'b' * 43},
        }

    def test_mail_profile_keeps_restricted_security_storage_and_private_protocol_ports(self):
        packet = self.packet()
        deployment, service = mail_manifests(packet)
        self.assertEqual(deployment['metadata']['namespace'], 'wd-mail-123')
        pod = deployment['spec']['template']['spec']
        container = pod['containers'][0]
        self.assertTrue(pod['securityContext']['runAsNonRoot'])
        self.assertEqual(pod['securityContext']['runAsUser'], 65532)
        self.assertFalse(pod['automountServiceAccountToken'])
        self.assertNotIn('security.webdock.dev/ingress', deployment['spec']['template']['metadata']['labels'])
        self.assertTrue(container['securityContext']['readOnlyRootFilesystem'])
        self.assertFalse(container['securityContext']['allowPrivilegeEscalation'])
        self.assertEqual(container['securityContext']['capabilities'], {'drop': ['ALL']})
        self.assertEqual(container['args'], ['--config', '/data/config.json'])
        self.assertIn({'name': 'data', 'mountPath': '/data'}, container['volumeMounts'])
        self.assertEqual(deployment['spec']['strategy'], {'type': 'Recreate'})
        self.assertEqual(service['spec']['type'], 'ClusterIP')
        self.assertEqual({p['port'] for p in service['spec']['ports']}, {8080, 2525, 2465, 1993})
        self.assertTrue(all(p['containerPort'] >= 1024 for p in container['ports']))
        self.assertTrue(all(v > 0 for v in packet['resources'].values()))
        self.assertEqual(container['startupProbe']['httpGet']['path'], '/healthz/live')
        self.assertNotIn('bootstrapPassword', str(deployment))
        self.assertNotIn('b' * 43, str(deployment))

    def test_mail_gateway_can_reach_only_the_private_http_listener(self):
        policy = mail_gateway_policy(self.packet())
        self.assertEqual(policy['metadata']['namespace'], 'wd-mail-123')
        self.assertEqual(policy['spec']['podSelector'], {'matchLabels': {'webdock.dev/mail-customer-id': '123'}})
        self.assertEqual(policy['spec']['ingress'], [{'from': [{'namespaceSelector': {'matchLabels': {'kubernetes.io/metadata.name': 'spitzli-mail-gateway'}}, 'podSelector': {'matchLabels': {'app': 'mail-gateway'}}}], 'ports': [{'protocol': 'TCP', 'port': 8080}]}])
        self.assertNotIn('egress', policy['spec'])

    def test_wrong_customer_hostname_unsafe_image_expired_lease_and_extra_arguments_are_rejected(self):
        for patch in [{'customerID': '../other'}, {'hostname': 'mail-999.mail.webdock.dev'},
                      {'image': 'stalwartlabs/stalwart:latest'}, {'leaseUntil': '2000-01-01T00:00:00Z'},
                      {'action': 'delete'}, {'args': ['/bin/sh']}]:
            with self.subTest(patch=patch), self.assertRaises(ValueError):
                validate_mail_packet({**self.packet(), **patch})
        for field in ['cpuMillicores', 'memoryBytes', 'ephemeralBytes', 'volumeBytes']:
            packet = self.packet()
            packet['resources'][field] = 0
            with self.subTest(field=field), self.assertRaises(ValueError):
                validate_mail_packet(packet)

    def test_suspension_scales_to_zero_without_deleting_storage(self):
        packet = self.packet()
        packet['action'] = 'suspend'
        deployment, service = mail_manifests(packet)
        self.assertEqual(deployment['spec']['replicas'], 0)
        self.assertIn('persistentVolumeClaim', str(deployment))
        self.assertEqual(service['spec']['selector'], {'webdock.dev/app-id': '123'})


if __name__ == '__main__':
    unittest.main()

class MailCapabilityTests(unittest.TestCase):
    def test_only_cached_pinned_image_on_verified_single_node_is_advertised(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import patch
        from mail_executor import mail_capability
        image = 'webdock.local/mail-stalwart@sha256:' + 'a' * 64
        with tempfile.TemporaryDirectory() as directory:
            config = Path(directory) / 'mail.json'
            config.write_text(json.dumps({'image': image, 'node': 'fixture-node'}))
            with patch('storage.private', return_value=config), patch('subprocess.run') as run:
                run.return_value.stdout = json.dumps({'status': {'repoDigests': [image]}})
                self.assertEqual(mail_capability(['fixture-node']), {'version': 1, 'image': image, 'configurationVersion': 3})
                self.assertIsNone(mail_capability(['other-node']))
                self.assertIsNone(mail_capability(['fixture-node', 'another-node']))
                run.return_value.stdout = json.dumps({'status': {'repoDigests': []}})
                self.assertIsNone(mail_capability(['fixture-node']))

class MailExecutionTests(unittest.TestCase):
    def exercise(self, fail_checkpoint=False, revision=1):
        import tempfile
        import os
        from contextlib import ExitStack
        from unittest.mock import patch, MagicMock
        from mail_executor import execute_mail
        packet = MailExecutorTests().packet()
        packet["revision"] = revision
        objects, events = {}, []
        def get(kind, name, namespace=None):
            kind = {'deployment': 'Deployment', 'service': 'Service', 'secret': 'Secret'}.get(kind, kind)
            return copy.deepcopy(objects.get((kind, name)))
        def apply(obj, owner):
            obj = copy.deepcopy(obj)
            if obj['kind'] == 'Deployment':
                obj['metadata']['generation'] = 1
                obj['status'] = {'observedGeneration': 1, 'updatedReplicas': 1, 'availableReplicas': 1}
                recovery = any(e['name'] == 'STALWART_RECOVERY_ADMIN' for e in obj['spec']['template']['spec']['containers'][0]['env'])
                events.append('recovery' if recovery else 'normal')
            if obj['kind'] == 'Service': obj['spec']['clusterIP'] = '10.43.0.20'
            objects[(obj['kind'], obj['metadata']['name'])] = obj
        def checkpoint(credentials):
            if credentials:
                events.append('credentials')
                if fail_checkpoint: raise ValueError('Control unavailable')
            return {'leaseUntil': packet['leaseUntil']}
        client = MagicMock()
        client.bootstrap.return_value = {'username': 'admin', 'password': 'permanent-fixture'}
        client.submission_certificate.return_value = 'a' * 64
        with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
            stack.enter_context(patch.dict(os.environ, {'WEBDOCK_EXECUTOR_LOCK': directory + '/lock'}))
            prepare = stack.enter_context(patch('executor.prepare_storage'))
            for method in ['prepare_namespace', 'check_owner']:
                stack.enter_context(patch('executor.' + method))
            stack.enter_context(patch('executor.get', side_effect=get))
            stack.enter_context(patch('executor.apply', side_effect=apply))
            stack.enter_context(patch('executor.guarded_delete', side_effect=lambda *args: events.append('secret-removed')))
            stack.enter_context(patch('executor.pods', return_value=[{'metadata': {}, 'status': {'conditions': [{'type': 'Ready', 'status': 'True'}]}}]))
            stack.enter_context(patch('mail_client.MailClient', return_value=client))
            if fail_checkpoint:
                with self.assertRaises(ValueError): execute_mail(packet, checkpoint)
            else:
                proof = execute_mail(packet, checkpoint)
                self.assertTrue(proof['recoveryDisabled'])
                self.assertEqual(proof['submissionCertificateSha256'], 'a' * 64)
                client.verify.assert_called_once()
                client.configure_listeners.assert_called_once()
                if revision > 1:
                    self.assertTrue(prepare.call_args.kwargs.get("initial_allocation"))
        return events

    def test_permanent_credentials_are_checkpointed_before_recovery_is_removed(self):
        events = self.exercise()
        self.assertLess(events.index('credentials'), events.index('normal'))
        self.assertLess(events.index('normal'), events.index('secret-removed'))

    def test_uncertain_credential_checkpoint_keeps_recovery_and_stops(self):
        events = self.exercise(True)
        self.assertIn('credentials', events)
        self.assertNotIn('normal', events)
        self.assertNotIn('secret-removed', events)

    def test_first_allocation_is_explicit_even_after_activation_toggles(self):
        self.exercise(revision=3)

    def test_unsafe_optional_configuration_disables_mail_without_stopping_hosting(self):
        from unittest.mock import patch
        from storage import StorageError
        from mail_executor import mail_capability
        with patch('storage.private', side_effect=StorageError('Unsafe storage path')):
            self.assertIsNone(mail_capability(['fixture-node']))

    def test_first_storage_allocation_keeps_actual_resource_revision(self):
        from unittest.mock import patch
        from executor import prepare_storage
        from mail_executor import application_packet
        packet = MailExecutorTests().packet(); packet['revision'] = 3
        p = application_packet(packet)
        with patch('executor.storage.ensure', return_value=({'node': 'fixture'}, '/private/mail')) as ensure, patch('executor.apply') as apply:
            prepare_storage(p, initial_allocation=True)
            self.assertEqual(ensure.call_args.args[0]['revision'], 1)
            self.assertEqual(apply.call_args_list[0].args[0]['metadata']['annotations']['webdock.dev/revision'], '3')
            prepare_storage(p)
            self.assertEqual(ensure.call_args.args[0]['revision'], 3)

    def test_suspend_preserves_webdock_field_ownership_for_subsequent_resume(self):
        from unittest.mock import patch
        from contextlib import ExitStack
        import tempfile, os
        from mail_executor import execute_mail
        packet = MailExecutorTests().packet(); packet['action'] = 'suspend'
        current = {'metadata': {'generation': 1}, 'spec': {'replicas': 1}, 'status': {'observedGeneration': 1}}
        with tempfile.TemporaryDirectory() as directory, ExitStack() as stack:
            stack.enter_context(patch.dict(os.environ, {'WEBDOCK_EXECUTOR_LOCK': directory + '/lock'}))
            stack.enter_context(patch('executor.get', return_value=current))
            stack.enter_context(patch('executor.check_owner'))
            stack.enter_context(patch('executor.pods', return_value=[]))
            kubectl = stack.enter_context(patch('executor.kubectl'))
            apply = stack.enter_context(patch('executor.apply'))
            execute_mail(packet, lambda credentials: {'leaseUntil': packet['leaseUntil']})
            kubectl.assert_not_called()
            self.assertEqual(apply.call_args.args[0]['spec']['replicas'], 0)
            self.assertNotIn('STALWART_RECOVERY', str(apply.call_args.args[0]))
