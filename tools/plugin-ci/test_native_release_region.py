"""Evaluate the real allocation conditions for source checks and runtime releases."""
from pathlib import Path
from types import SimpleNamespace
import unittest
import yaml

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = yaml.load((ROOT/'tools/plugin-release/plugin.yml').read_text(),Loader=yaml.BaseLoader)


def scheduled(job,region,event='push',ref='refs/tags/v0.1.4',ready='true',builder='true',fork=False):
    repository = 'spitzli/plugin-fixture'
    context = {
        'vars':SimpleNamespace(SPITZLI_BLACKSMITH_READY=ready,SPITZLI_BLACKSMITH_REGION=region,
                               SPITZLI_BLACKSMITH_BUILDER_READY=builder),
        'github':SimpleNamespace(event_name=event,ref=ref,repository=repository,
                                 event=SimpleNamespace(pull_request=SimpleNamespace(
                                     head=SimpleNamespace(repo=SimpleNamespace(full_name='outside/fork' if fork else repository))))),
        'startsWith':lambda value,prefix:value.startswith(prefix),
    }
    expression = WORKFLOW['jobs'][job]['if'].replace('&&',' and ').replace('||',' or ')
    return eval(expression,{'__builtins__':{}},context)


class NativeReleaseRegion(unittest.TestCase):
    def test_source_checks_keep_the_owners_all_region_ci_exception(self):
        for region in ['us-east','us-west','eu-central','eu-west']:
            with self.subTest(region=region):
                self.assertTrue(scheduled('check',region,ref='refs/heads/feature'))
                self.assertTrue(scheduled('check',region,event='pull_request',ref='refs/pull/1/merge'))

    def test_production_image_releases_require_eu_and_a_version_tag(self):
        for region in ['','us-east','us-west','eu-central','eu-west']:
            with self.subTest(region=region):
                self.assertEqual(scheduled('release',region),region in ['eu-central','eu-west'])
                self.assertFalse(scheduled('release',region,ref='refs/heads/main'))
                self.assertFalse(scheduled('release',region,event='pull_request'))

    def test_unqualified_builders_and_forks_cannot_allocate_jobs(self):
        for job in ['check','release']:
            for values in [{'ready':'false'},{'builder':'false'}]:
                with self.subTest(job=job,values=values):
                    self.assertFalse(scheduled(job,'eu-central',**values))
        self.assertFalse(scheduled('check','eu-central',event='pull_request',fork=True))


if __name__ == '__main__':
    unittest.main()
