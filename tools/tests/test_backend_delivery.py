"""Delivery boundaries: public check-only CI, explicit bootstrap and no frontend in backend."""
from pathlib import Path
import re
import unittest
import yaml

ROOT=Path(__file__).resolve().parents[2]

def workflow(name):
    # BaseLoader preserves GitHub's `on` key rather than YAML1.1's boolean coercion.
    return yaml.load((ROOT/".github/workflows"/name).read_text(),Loader=yaml.BaseLoader)

class BackendDeliveryTests(unittest.TestCase):
    def test_quality_is_read_only_and_does_not_dispatch_fork_code_on_private_runners(self):
        job=workflow("quality.yml")
        self.assertEqual(set(job['on']),{'push','workflow_dispatch'})
        self.assertEqual(job['permissions'],{'contents':'read'})
        self.assertIn('ubuntu-24.04',job['jobs']['quality']['runs-on'])
        self.assertIn('WEBDOCK_CI_RUNNER',job['jobs']['quality']['runs-on'])
        self.assertNotIn('secrets.',str(job))
        steps=job['jobs']['quality']['steps']
        self.assertTrue(any('bun run quality' in s.get('run','') for s in steps))
        self.assertTrue(any('sha256sum --check' in s.get('run','') for s in steps))

    def test_bootstrap_is_manual_main_only_and_cannot_publish_or_deploy(self):
        job=workflow('backend-bootstrap.yml')
        self.assertEqual(set(job['on']),{'workflow_dispatch'})
        self.assertEqual(job['permissions'],{'contents':'read'})
        package=job['jobs']['package']
        self.assertIn("github.repository == 'spitzli/webdock'",package['if'])
        self.assertIn("github.ref == 'refs/heads/main'",package['if'])
        self.assertNotIn('secrets.',str(job))
        self.assertNotIn('kubectl',str(job));self.assertNotIn('helm upgrade',str(job))
        setup=next(s for s in package['steps'] if s.get('uses','').startswith('docker/setup-buildx-action@'))
        self.assertEqual(setup['with']['version'],'v0.34.1')
        self.assertRegex(setup['with']['driver-opts'],r'^image=docker.io/moby/buildkit:v[0-9.]+@sha256:[a-f0-9]{64}$')
        build=next(s for s in package['steps'] if s.get('uses','').startswith('docker/build-push-action@'))
        self.assertEqual(build['with']['file'],'deploy/docker/backend.Dockerfile')
        self.assertEqual(build['with']['target'],'backend')
        self.assertEqual(build['with']['push'],'false')
        upload=next(s for s in package['steps'] if s.get('uses','').startswith('actions/upload-artifact@'))
        self.assertEqual(upload['with']['retention-days'],'1')
        self.assertIn('480*1024*1024',str(package['steps']))

    def test_every_external_action_is_immutable_and_backend_has_no_ui_stage(self):
        for name in ['quality.yml','backend-bootstrap.yml']:
            for job in workflow(name)['jobs'].values():
                for step in job['steps']:
                    if 'uses' in step:self.assertRegex(step['uses'],r'^[\w./-]+@[a-f0-9]{40}$')
        source=(ROOT/'deploy/docker/backend.Dockerfile').read_text()
        self.assertNotIn('bun install',source);self.assertNotIn('console-build',source)
        self.assertNotIn('WEBDOCK_UI_DIST',source)
        bases=re.findall(r'^FROM (\S+)',source,re.M)
        for base in bases:
            if base not in ['runtime','source-context','scratch','build-base']:self.assertRegex(base,r'@sha256:[a-f0-9]{64}$')
        self.assertIn('USER 65532:65532',source)
        self.assertIn('webdock-kubernetes-build-worker',source)

    def test_runtime_package_bootstrap_uses_explicit_verified_ca_before_compilation(self):
        source=(ROOT/'deploy/docker/backend.Dockerfile').read_text()
        self.assertIn('COPY --from=build-base /etc/ssl/certs/ca-certificates.crt',source)
        self.assertEqual(source.count('Acquire::https::CAInfo=/etc/ssl/certs/ca-certificates.crt'),4)
        self.assertEqual(source.count('APT::Update::Error-Mode=any'),2)
        self.assertNotRegex(source,r'Verify-(?:Peer|Host)=false|allow-unauthenticated')

if __name__=='__main__':unittest.main()
