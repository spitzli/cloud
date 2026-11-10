import unittest
from render_control import render


class ControlPolicyTests(unittest.TestCase):
    def setUp(self):
        self.args = dict(image='webdock.local/core/backend@sha256:' + 'a9' * 32,
                         namespace='spitzli-core', prefix='webdock',
                         environment_secret='native-runtime', source_secret='native-source',
                         issuer_secret='native-issuer', callback_sources=['10.89.0.1'],
                         runtime_host='10.0.0.2')

    def objects(self, kind):
        return [x for x in render(**self.args)['items'] if x['kind'] == kind]

    def test_disabled_by_default_and_singletons(self):
        self.assertTrue(all(x['spec']['replicas'] == 0 for x in self.objects('Deployment')))
        with self.assertRaises(ValueError):
            render(**self.args, replicas=2)
        self.assertTrue(all(x['spec']['replicas'] == 1 for x in render(**self.args, replicas=1)['items']
                            if x['kind'] == 'Deployment'))

    def test_rejects_mutable_and_placeholder_images(self):
        for image in ['repo/core:latest', 'repo/core@sha256:' + '0' * 64, 'repo/core@sha256:bad']:
            with self.assertRaises(ValueError):
                render(**{**self.args, 'image': image})

    def test_rejects_public_metadata_and_broad_private_sources(self):
        for key in ['runtime_host', 'callback_sources']:
            for value in ['169.254.169.254', '1.1.1.1', '0.0.0.0', '10.0.0.0/8', '::1']:
                with self.assertRaises(ValueError):
                    render(**{**self.args, key: [value] if key == 'callback_sources' else value})

    def test_issuer_secret_cannot_alias_common_or_environment_material(self):
        for name in ['native-source', 'native-runtime']:
            with self.assertRaises(ValueError):
                render(**{**self.args, 'issuer_secret': name})

    def test_kernel_ca_key_is_controller_only(self):
        for deployment in self.objects('Deployment'):
            pod = deployment['spec']['template']['spec']
            issuer = [x for x in pod['volumes'] if x.get('secret', {}).get('secretName') == 'native-issuer']
            self.assertEqual(bool(issuer), deployment['metadata']['name'].endswith('controller'))

    def test_private_service_and_exact_callback_sources(self):
        service, = self.objects('Service')
        self.assertEqual(service['spec']['type'], 'ClusterIP')
        self.assertEqual(service['spec']['ports'], [{'name': 'mtls', 'port': 3132, 'targetPort': 'mtls'}])
        for policy in self.objects('NetworkPolicy'):
            expected = [] if policy['metadata']['name'].endswith('controller') else [
                {'from': [{'ipBlock': {'cidr': '10.89.0.1/32'}}],
                 'ports': [{'protocol': 'TCP', 'port': 3132}]}]
            self.assertEqual(policy['spec']['ingress'], expected)

    def test_no_engine_credentials_or_privileged_runtime(self):
        for deployment in self.objects('Deployment'):
            pod = deployment['spec']['template']['spec']
            self.assertFalse(pod['automountServiceAccountToken'])
            self.assertNotIn('hostNetwork', pod)
            self.assertTrue(all('hostPath' not in x for x in pod['volumes']))
            self.assertEqual(pod['securityContext']['runAsUser'], 65532)
            for c in pod['containers'] + pod['initContainers']:
                self.assertEqual(c['image'], self.args['image'])
                self.assertTrue(c['securityContext']['readOnlyRootFilesystem'])
                self.assertFalse(c['securityContext']['allowPrivilegeEscalation'])
                self.assertEqual(c['securityContext']['capabilities'], {'drop': ['ALL']})
                self.assertEqual(c['resources']['limits']['memory'], '768Mi')
            runtime, = pod['containers']
            self.assertNotIn('env', runtime)
            self.assertEqual(runtime['envFrom'], [{'secretRef': {'name': 'native-runtime'}}])

    def test_policies_precede_each_deployment(self):
        seen = set()
        for obj in render(**self.args)['items']:
            if obj['kind'] == 'NetworkPolicy':
                seen.add(obj['metadata']['name'])
            if obj['kind'] == 'Deployment':
                self.assertIn(obj['metadata']['name'], seen)

    def test_files_staged_without_relaxing_private_reader_contract(self):
        for deployment in self.objects('Deployment'):
            for init in deployment['spec']['template']['spec']['initContainers']:
                script = init['command'][2]
                self.assertIn('umask 077', script)
                self.assertIn('chmod 0700 /private/data', script)
                self.assertIn('chmod 0600', script)
                self.assertNotIn('ln ', script)


if __name__ == '__main__':
    unittest.main()
