"""Check provider-only allocation and execute region guards without launching CI."""
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import unittest
import yaml

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location('readiness', Path(__file__).with_name('check_blacksmith_readiness.py'))
READINESS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(READINESS)


def workflows():
    paths = [*ROOT.glob('.github/workflows/*.yml'), ROOT/'tools/plugin-release/plugin.yml', ROOT/'tools/plugin-release/legacy.yml']
    evidence = json.loads(READINESS.EVIDENCE.read_text())
    for name in evidence['repositories']:
        sibling = ROOT.parent/name.split('/')[1]
        if sibling != ROOT and sibling.exists():
            paths.extend(sibling.glob('.github/workflows/*.yml'))
    return [(path, yaml.load(path.read_text(), Loader=yaml.BaseLoader)) for path in paths]


class BlacksmithWorkflows(unittest.TestCase):
    def test_all_jobs_allocate_only_blacksmith_after_provider_qualification(self):
        for path, workflow in workflows():
            with self.subTest(path=path):
                self.assertEqual(workflow['permissions'], {'contents': 'read'})
                self.assertLessEqual(set(workflow['on']), {'push', 'pull_request', 'workflow_dispatch'})
                for job in workflow['jobs'].values():
                    self.assertEqual(job['runs-on'], 'blacksmith-4vcpu-ubuntu-2404')
                    condition = job['if']
                    probe = path.name == 'blacksmith-qualification.yml'
                    if probe:
                        self.assertEqual(workflow['on']['push']['branches'], ['ci/blacksmith-qualification'])
                        self.assertNotIn('pull_request', workflow['on'])
                        self.assertIn("github.repository == 'spitzli/cloud'", condition)
                        self.assertIn("github.ref == 'refs/heads/ci/blacksmith-qualification'", condition)
                        self.assertNotIn('secrets.', path.read_text())
                        self.assertNotIn('WEBDOCK_BLACKSMITH_READY', condition)
                    else:
                        self.assertIn("vars.WEBDOCK_BLACKSMITH_READY == 'true'", condition)
                    self.assertIn("vars.WEBDOCK_BLACKSMITH_REGION != ''", condition)
                    if 'pull_request' in workflow['on']:
                        self.assertTrue("(github.event_name != 'pull_request' || github.event.pull_request.head.repo.full_name == github.repository)" in condition
                            or "github.event_name == 'push'" in condition
                            or "github.event_name == 'workflow_dispatch'" in condition)
                    for step in job['steps']:
                        if 'uses' in step:
                            self.assertRegex(step['uses'], r'^[a-zA-Z0-9_./-]+@[0-9a-f]{40}$')
                            if step['uses'].startswith('actions/checkout@'):
                                self.assertEqual(step['with']['persist-credentials'], 'false')
                        if 'build_oci.py' in step.get('run', '') and not probe:
                            self.assertIn("vars.WEBDOCK_BLACKSMITH_BUILDER_READY == 'true'", condition)
                self.assertNotIn('docker/setup-buildx-action', path.read_text())
                self.assertNotIn('WEBDOCK_PLUGIN_CI_RUNNER', path.read_text())
                self.assertNotIn('WEBDOCK_CI_RUNNER', path.read_text())

    def test_runtime_guard_accepts_documented_matching_regions_and_rejects_missing_or_mismatched(self):
        for path, workflow in workflows():
            for job in workflow['jobs'].values():
                with self.subTest(path=path):
                    guard = job['steps'][0]
                    self.assertNotIn('uses', guard)
                    for actual, expected, success in [(None,'eu-central',False),('us-east','us-east',True),('us-west','us-west',True),('unknown','unknown',False),('eu-west','eu-central',False),('eu-central','eu-central',True),('eu-west','eu-west',True)]:
                        environment = dict(os.environ, QUALIFIED_BLACKSMITH_REGION=expected)
                        environment.pop('BLACKSMITH_REGION', None)
                        if actual is not None:
                            environment['BLACKSMITH_REGION'] = actual
                        result = subprocess.run(['bash','-c',guard['run']], env=environment, capture_output=True, timeout=5)
                        self.assertEqual(result.returncode == 0, success, (path, actual, expected))

    def test_shared_helper_copies_stay_identical(self):
        helper = (ROOT/'tools/plugin-release/build_oci.py').read_bytes()
        evidence = json.loads(READINESS.EVIDENCE.read_text())
        for repository in evidence['repositories'][1:]:
            sibling = ROOT.parent/repository.split('/')[1]
            if sibling.exists():
                copies = list((sibling/'scripts').rglob('build_oci.py'))
                self.assertEqual(len(copies), 1, sibling)
                self.assertEqual(copies[0].read_bytes(), helper, sibling)

    def test_managed_builder_setup_has_no_fallback_and_is_guarded_before_and_after(self):
        for path, workflow in workflows():
            for job in workflow['jobs'].values():
                if job.get('env', {}).get('WEBDOCK_OCI_BACKEND') != 'blacksmith-buildx':
                    continue
                with self.subTest(path=path):
                    steps=job['steps']
                    before=[index for index,step in enumerate(steps) if '--before-blacksmith-setup' in step.get('run','')]
                    after=[index for index,step in enumerate(steps) if '--assert-blacksmith-builder' in step.get('run','')]
                    setup=[index for index,step in enumerate(steps) if step.get('uses','').startswith('useblacksmith/setup-docker-builder@')]
                    self.assertEqual(len(before),1);self.assertEqual(len(after),1);self.assertEqual(len(setup),1)
                    self.assertLess(before[0],setup[0]);self.assertLess(setup[0],after[0])
                    action=steps[setup[0]]
                    self.assertEqual(action['uses'],'useblacksmith/setup-docker-builder@19215110ab936351210feebdfa5b440b4493e184')
                    self.assertEqual(action['with']['nofallback'],'true')
                    self.assertEqual(action['with']['platforms'],'linux/amd64')
                    self.assertEqual(action['with']['buildx-version'],'v0.23.0')
                    self.assertTrue(action['with']['cache-key'])
                    for forbidden in ['--allow','docker.sock','--oci-worker-no-process-sandbox','unconfined','docker:dind']:
                        self.assertNotIn(forbidden,path.read_text())

    def test_release_readiness_rejects_skipped_or_incomplete_checks(self):
        fixture = {'status':'qualified','blacksmithInstalled':True,'qualifiedRegion':'us-east','regionProof':'reviewed-provider-evidence',
            'builder':dict(qualified=True,backend='blacksmith-managed',fallbackDisabled=True,proof='reviewed-builder-evidence'),
            'repositories':['spitzli/example'],'successfulChecks':[dict(repository='spitzli/example',conclusion='success',region='us-east',runnerLabel='blacksmith-4vcpu-ubuntu-2404',sourceSha='a'*40,jobUrl='https://github.com/spitzli/example/actions/runs/123/job/456')]}
        self.assertEqual(READINESS.blockers(fixture), [])
        for field, value in [('conclusion','skipped'),('region','us-west'),('runnerLabel','ubuntu-24.04'),('sourceSha','main'),('jobUrl','')]:
            changed = json.loads(json.dumps(fixture)); changed['successfulChecks'][0][field] = value
            self.assertTrue(READINESS.blockers(changed), field)
        self.assertTrue(READINESS.blockers(json.loads(READINESS.EVIDENCE.read_text())))


if __name__ == '__main__':
    unittest.main()
