"""Exercise deployment decisions against a fake Kubernetes API, never a cluster."""
import base64
import copy
import json
import subprocess
import unittest
from unittest.mock import patch

import plugin_installer as installer
import worker


IMAGE = 'registry.example/plugins@sha256:' + 'a' * 64
MANIFEST = {'name': 'example', 'version': '1.0.0', 'image': IMAGE, 'navigation': []}
JOB = {'id': '123', 'claimToken': '00000000-0000-4000-8000-000000000001',
       'projectID': '456', 'customerID': '789', 'mode': 'build', 'phase': 'deploy',
       'manifestRaw': json.dumps(MANIFEST), 'configuration': {'greeting': 'Hello'}}
CONFIG = {'runtime': {
    'namespace': 'plugins', 'clusterID': 'cluster-1',
    'apiNamespace': 'platform', 'apiPodLabels': {'app': 'webdock-api'},
    'apiAddresses': ['203.0.113.10'], 'hostOrigin': 'https://api.example',
    'configSecret': {'namespace': 'platform', 'name': 'runtime-config', 'key': 'runtime.json'},
}}


def encode(value):
    return base64.b64encode(json.dumps(value).encode()).decode()


class Control:
    def __init__(self):
        self.calls = []

    def request(self, path, body):
        self.calls.append((path, copy.deepcopy(body)))
        if path == '/claim':
            return copy.deepcopy(JOB)
        if path == '/lease':
            if body != {'id': JOB['id'], 'claimToken': JOB['claimToken']}:
                raise AssertionError('Lease must retain the claimed job identity')
            return {'active': True}
        if path == '/complete':
            return {'accepted': True}
        raise AssertionError('Unexpected control request: ' + path)


class Kubernetes:
    def __init__(self, private_key):
        self.settings = {
            'privateKey': private_key, 'hostOrigin': 'https://api.example',
            'runtimes': {'existing-plugin': {
                'generation': 2, 'namespace': 'other-plugins', 'service': 'existing',
                'addresses': ['10.96.0.99'], 'credential': 'existing-runtime-token',
                'ca': 'existing-ca', 'projects': {},
            }},
        }
        self.secret = {
            'apiVersion': 'v1', 'kind': 'Secret', 'type': 'Opaque',
            'metadata': {'name': 'runtime-config', 'namespace': 'platform',
                         'resourceVersion': '41', 'labels': {'operator-owned': 'true'}},
            'data': {'runtime.json': encode(self.settings),
                     'unrelated-key': base64.b64encode(b'preserved-value').decode()},
        }
        self.namespace_policy = 'restricted'
        self.reads = []
        self.created = []
        self.replacements = []
        self.accepted_replace = None
        self.change_during_rollout = False
        self.pod_image = IMAGE
        self.observed_image = 'registry.example/plugins@sha256:' + 'a' * 64
        self.ready = True
        self.pod_count = 1

    def get(self, kind, name, namespace=None):
        self.reads.append((kind, name, namespace))
        if (kind, name, namespace) == ('namespace', 'plugins', None):
            return {'metadata': {'name': 'plugins', 'labels': {
                'pod-security.kubernetes.io/enforce': self.namespace_policy}}}
        if (kind, name, namespace) == ('secret', 'runtime-config', 'platform'):
            return copy.deepcopy(self.secret)
        if (kind, name, namespace) == ('service', 'pi-123', None):
            return {'metadata': {'name': 'pi-123', 'namespace': 'plugins'},
                    'spec': {'clusterIP': '10.96.0.42'}}
        raise AssertionError('Unexpected Kubernetes read: ' + repr((kind, name, namespace)))

    def call(self, args, value=None, timeout=30):
        if args == ['create']:
            self.created.append(copy.deepcopy(value))
            return b'created'
        if args == ['rollout', 'status', 'deployment/pi-123', '-n', 'plugins', '--timeout=300s']:
            if self.change_during_rollout:
                self.secret['metadata']['resourceVersion'] = '42'
                self.secret['data']['new-operator-key'] = base64.b64encode(b'new-value').decode()
            return b'deployment rolled out'
        if args == ['get', 'pods', '-n', 'plugins', '-l', 'webdock.dev/source-import=123', '-o', 'json']:
            pod = {'spec': {'containers': [{'name': 'plugin', 'image': self.pod_image}]},
                   'status': {'containerStatuses': [{'name': 'plugin', 'ready': self.ready,
                                                    'imageID': self.observed_image}]}}
            return json.dumps({'items': [pod] * self.pod_count}).encode()
        if args == ['replace']:
            self.replacements.append(copy.deepcopy(value))
            if value['metadata'].get('resourceVersion') != self.secret['metadata']['resourceVersion']:
                raise worker.Rejected('resource_version_conflict')
            self.accepted_replace = copy.deepcopy(value)
            self.secret = copy.deepcopy(value)
            return b'replaced'
        raise AssertionError('Unexpected Kubernetes write: ' + repr(args))


class PluginDeployTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.private_key = subprocess.run(
            ['/usr/bin/openssl', 'genpkey', '-algorithm', 'ed25519'],
            capture_output=True, timeout=10, check=True,
        ).stdout.decode()
        cls.public_key = subprocess.run(
            ['/usr/bin/openssl', 'pkey', '-pubout'], input=cls.private_key.encode(),
            capture_output=True, timeout=10, check=True,
        ).stdout.decode()

    def setUp(self):
        self.kube = Kubernetes(self.private_key)
        self.control = Control()

    def deploy(self):
        with patch('plugin_installer.Kubernetes', return_value=self.kube):
            return installer.deploy_runtime(copy.deepcopy(CONFIG), self.control, copy.deepcopy(JOB))

    def test_verified_deployment_adds_one_binding_and_preserves_existing_secret(self):
        before = copy.deepcopy(self.kube.secret)
        result = self.deploy()
        self.assertEqual(result['phase'], 'deploy')
        self.assertEqual(result['clusterID'], 'cluster-1')
        self.assertEqual(result['binding']['service'], 'pi-123')
        self.assertEqual(result['binding']['addresses'], ['10.96.0.42'])
        self.assertEqual([item['kind'] for item in self.kube.created],
                         ['NetworkPolicy', 'Secret', 'Secret', 'Service', 'Deployment'])
        self.assertTrue(all(item['metadata']['namespace'] == 'plugins' for item in self.kube.created))
        self.assertTrue(all(item['metadata']['labels'] == {'webdock.dev/source-import': '123'}
                            for item in self.kube.created))
        deployment = self.kube.created[-1]
        self.assertEqual(deployment['spec']['template']['spec']['containers'][0]['image'], IMAGE)
        identity = self.kube.created[1]['stringData']
        self.assertEqual(identity['SPITZLI_PLUGIN_PUBLIC_KEY'], self.public_key)
        self.assertEqual(identity['SPITZLI_PLUGIN_RUNTIME_TOKEN'], result['binding']['credential'])
        self.assertEqual(json.loads(identity['SPITZLI_PLUGIN_CONFIGURATION']), JOB['configuration'])
        self.assertNotIn(self.private_key, json.dumps(self.kube.created))
        certificate = self.kube.created[2]['stringData']['tls.crt']
        self.assertEqual(result['binding']['ca'], certificate)
        extensions = subprocess.run(
            ['/usr/bin/openssl', 'x509', '-noout', '-ext', 'subjectAltName'],
            input=certificate.encode(), capture_output=True, timeout=10, check=True,
        ).stdout.decode()
        self.assertIn('DNS:pi-123.plugins.svc.cluster.local', extensions)
        replacement = self.kube.accepted_replace
        self.assertEqual(replacement['metadata'], before['metadata'])
        self.assertEqual(replacement['data']['unrelated-key'], before['data']['unrelated-key'])
        settings = json.loads(base64.b64decode(replacement['data']['runtime.json']))
        self.assertEqual(settings['privateKey'], self.private_key)
        self.assertEqual(settings['runtimes']['existing-plugin'], self.kube.settings['runtimes']['existing-plugin'])
        self.assertEqual(settings['runtimes']['example'], result['binding'])
        self.assertEqual(len(settings['runtimes']), 2)

    def test_ui_deploy_uses_operator_console_origin_and_standard_public_key(self):
        config=copy.deepcopy(CONFIG)
        config['runtime'].update(consoleOrigin='https://console.example.test',uiBaseDomain='plugins.example.net',
                                 uiCertResolver='webdock',ingressClass='traefik',ingressNamespace='kube-system',
                                 ingressPodLabels={'app.kubernetes.io/name':'traefik'})
        job={**copy.deepcopy(JOB),'manifestRaw':json.dumps({**MANIFEST,'navigation':[{'path':'/ui/cms'}]})}
        with patch('plugin_installer.Kubernetes',return_value=self.kube):
            installer.deploy_runtime(config,self.control,job)
        identity=next(item for item in self.kube.created if item['kind']=='Secret' and item['type']=='Opaque')['stringData']
        self.assertEqual(identity['SPITZLI_PLUGIN_CONSOLE_ORIGIN'],'https://console.example.test')
        self.assertEqual(identity['SPITZLI_PLUGIN_PUBLIC_KEY'],self.public_key)
        self.assertNotIn('SPITZLI_PLUGIN_HOST_PUBLIC_KEY',identity)

    def test_missing_or_invalid_ui_console_origin_creates_no_resource(self):
        config=copy.deepcopy(CONFIG)
        config['runtime'].update(uiBaseDomain='plugins.example.net',uiCertResolver='webdock',ingressClass='traefik',
                                 ingressNamespace='kube-system',ingressPodLabels={'app.kubernetes.io/name':'traefik'})
        job={**copy.deepcopy(JOB),'manifestRaw':json.dumps({**MANIFEST,'navigation':[{'path':'/ui/cms'}]})}
        for origin in [None,'http://console.example.test','https://console.example.test/']:
            with self.subTest(origin=origin),patch('plugin_installer.Kubernetes',return_value=self.kube):
                config['runtime']['consoleOrigin']=origin
                with self.assertRaises(worker.Rejected):
                    installer.deploy_runtime(config,self.control,job)
                self.assertEqual(self.kube.created,[])
                self.assertEqual(self.kube.replacements,[])

    def test_wrong_requested_or_observed_image_never_updates_runtime_binding(self):
        for field in ('pod_image', 'observed_image'):
            with self.subTest(field=field):
                self.kube = Kubernetes(self.private_key)
                setattr(self.kube, field, 'registry.example/plugins@sha256:' + 'b' * 64)
                with self.assertRaisesRegex(worker.Rejected, 'runtime_image_mismatch'):
                    self.deploy()
                self.assertEqual(self.kube.replacements, [])

    def test_unready_or_ambiguous_pods_never_update_runtime_binding(self):
        for field, value in [('ready', False), ('pod_count', 0), ('pod_count', 2)]:
            with self.subTest(field=field, value=value):
                self.kube = Kubernetes(self.private_key)
                setattr(self.kube, field, value)
                with self.assertRaises(worker.Rejected):
                    self.deploy()
                self.assertEqual(self.kube.replacements, [])

    def test_existing_runtime_is_never_replaced(self):
        self.kube.settings['runtimes']['example'] = {'credential': 'keep-existing-runtime'}
        self.kube.secret['data']['runtime.json'] = encode(self.kube.settings)
        before = copy.deepcopy(self.kube.secret)
        with self.assertRaisesRegex(worker.Rejected, 'runtime_already_configured'):
            self.deploy()
        self.assertEqual(self.kube.created, [])
        self.assertEqual(self.kube.secret, before)

    def test_namespace_must_enforce_restricted_before_any_secret_or_resource_access(self):
        self.kube.namespace_policy = 'baseline'
        with self.assertRaisesRegex(worker.Rejected, 'restricted_namespace_required'):
            self.deploy()
        self.assertEqual(self.kube.reads, [('namespace', 'plugins', None)])
        self.assertEqual(self.kube.created, [])

    def test_secret_compare_and_swap_conflict_cannot_report_success_or_overwrite_operator_changes(self):
        self.kube.change_during_rollout = True
        with patch('plugin_installer.Kubernetes', return_value=self.kube):
            self.assertTrue(installer.run_once(copy.deepcopy(CONFIG), self.control))
        self.assertEqual(self.kube.replacements[0]['metadata']['resourceVersion'], '41')
        self.assertIsNone(self.kube.accepted_replace)
        self.assertEqual(self.kube.secret['metadata']['resourceVersion'], '42')
        self.assertIn('new-operator-key', self.kube.secret['data'])
        unchanged = json.loads(base64.b64decode(self.kube.secret['data']['runtime.json']))
        self.assertNotIn('example', unchanged['runtimes'])
        completion = [body for path, body in self.control.calls if path == '/complete']
        self.assertEqual(completion, [{'id': JOB['id'], 'claimToken': JOB['claimToken'],
                                      'result': {'phase': 'failed', 'code': 'INSTALLATION_FAILED'}}])


if __name__ == '__main__':
    unittest.main()
