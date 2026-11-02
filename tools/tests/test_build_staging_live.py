"""Offline trust/budget/ownership checks for the temporary source-GET fixture."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]


class StagingFixture(unittest.TestCase):
    def module(self):
        path = ROOT / 'tools/tests/build-staging-live.py'
        self.assertTrue(path.is_file(), 'repeatable fresh source-GET fixture is missing')
        spec = importlib.util.spec_from_file_location('build_staging_live', path)
        module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
        return module

    def test_state_root_is_explicit_private_and_never_follows_a_symlink(self):
        m = self.module()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory) / 'private'
            self.assertEqual(m.private_root(root), root)
            self.assertEqual(root.stat().st_mode & 0o777, 0o700)
            linked = Path(directory) / 'linked'; linked.symlink_to(root, target_is_directory=True)
            with self.assertRaises(ValueError): m.private_root(linked)
            root.chmod(0o755)
            with self.assertRaises(ValueError): m.private_root(root)
        with self.assertRaises(ValueError): m.private_root(Path('relative'))

    def test_only_infrastructure_minio_receives_hostnetwork_and_real_tmpfs_bound(self):
        m = self.module()
        record = {'nonce': 'a'*20, 'namespace': 'wd-stage-fixture-'+'a'*20, 'image': 'docker.io/library/fixture@sha256:'+'b'*64}
        docs = m.objects(record, {'accessKeyId': 'testadmin', 'secretAccessKey': 'testsecret'}, b'cert', b'key', b'ca')
        pods = [item for item in docs if item['kind'] == 'Pod']
        self.assertEqual(len(pods), 1)
        pod = pods[0]['spec']
        self.assertTrue(pod['hostNetwork'])
        self.assertFalse(pod['automountServiceAccountToken'])
        self.assertEqual(pod['containers'][0]['imagePullPolicy'], 'Never')
        self.assertIn('@sha256:', pod['containers'][0]['image'])
        self.assertFalse(pod['containers'][0]['securityContext']['privileged'])
        self.assertFalse(pod['containers'][0]['securityContext']['allowPrivilegeEscalation'])
        self.assertEqual(pod['containers'][0]['securityContext']['capabilities']['drop'], ['ALL'])
        data = next(item for item in pod['volumes'] if item['name']=='data')['emptyDir']
        self.assertEqual(data, {'medium': 'Memory', 'sizeLimit': '1Gi'})
        self.assertEqual(pod['containers'][0]['resources']['limits']['memory'], '1536Mi')
        self.assertNotIn('hostPath', str(docs))

    def test_source_credentials_and_policy_cannot_write_or_administer_minio(self):
        m = self.module()
        operator, source = m.iam_policies('wd-stage-'+'a'*20)
        actions = {action for statement in source['Statement'] for action in statement['Action']}
        self.assertEqual(actions, {'s3:GetObject', 's3:GetBucketLocation'})
        self.assertIn('worker-staging/*', str(source))
        self.assertNotIn('s3:*', str(operator))
        self.assertNotIn('s3:ListAllMyBuckets', str(operator))
        policy = m.source_egress('a'*20)
        self.assertEqual(policy['metadata']['namespace'], 'webdock-build-qualification')
        self.assertEqual(policy['spec']['podSelector'], {'matchLabels': {'webdock.dev/component': 'build-job'}})
        self.assertEqual(policy['spec']['egress'], [{'to': [{'ipBlock': {'cidr': '167.86.75.144/32'}}], 'ports': [{'protocol': 'TCP', 'port': 19443}]}])

    def test_upload_policy_is_separate_and_only_the_exact_upload_port(self):
        m = self.module()
        self.assertTrue(hasattr(m, 'upload_egress'))
        policy = m.upload_egress('a'*20)
        self.assertEqual(policy['metadata']['name'], 'webdock-upload-qualification')
        self.assertEqual(policy['spec']['podSelector'], {'matchLabels': {'webdock.dev/component': 'build-job'}})
        self.assertEqual(policy['spec']['egress'], [{'to': [{'ipBlock': {'cidr': '167.86.75.144/32'}}], 'ports': [{'protocol': 'TCP', 'port': 19444}]}])
        config = m.dispatcher_config(b'public-test-ca', 'test-scoped-token')
        self.assertEqual(config['current-context'], 'owned-build-live')
        self.assertEqual(config['clusters'][0]['cluster']['server'], 'https://127.0.0.1:16443')
        self.assertNotIn('client-key-data', str(config))
        self.assertNotIn('insecure-skip-tls-verify', str(config))

    def test_transfer_budget_and_origins_match_the_bounded_upload_gateway(self):
        m = self.module()
        self.assertTrue(hasattr(m, 'transfer_policy'))
        policy = m.transfer_policy('test-ca')
        self.assertEqual(policy['maxResultBytes'], 537059328)
        self.assertEqual(policy['allowedOrigins'], ['https://167.86.75.144:19443', 'https://167.86.75.144:19444'])
        self.assertEqual(policy['certificateAuthorityPEM'], 'test-ca')

    def test_cleanup_requires_exact_recorded_uid_and_nonce(self):
        m = self.module()
        record = {'nonce': 'a'*20, 'namespace': 'wd-stage-fixture-'+'a'*20, 'namespaceUID': 'original'}
        observed = {'metadata': {'uid': 'original', 'name': record['namespace'], 'labels': {'webdock.dev/fixture': record['nonce']}}}
        self.assertEqual(m.namespace_delete(record, observed)['preconditions'], {'uid': 'original'})
        observed['metadata']['uid'] = 'replacement'
        with self.assertRaises(ValueError): m.namespace_delete(record, observed)


if __name__ == '__main__': unittest.main()
