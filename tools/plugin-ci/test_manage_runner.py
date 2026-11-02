import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch
spec=importlib.util.spec_from_file_location('ci_manager',Path(__file__).with_name('manage_runner.py'))
module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)

class SourceGates(unittest.TestCase):
    def workflow(self,condition,event='pull_request'):
        return 'on:\n  '+event+':\njobs:\n  check:\n    runs-on: ${{ fromJSON(vars.WEBDOCK_PLUGIN_CI_RUNNER) }}\n    if: '+condition+'\n    steps: []\n'
    def test_trusted_same_repo_guard_and_exact_push_guard_accepted(self):
        self.assertTrue(module.workflow_gate(self.workflow(module.GATE)))
        self.assertTrue(module.workflow_gate(self.workflow("github.event_name == 'push'",'push')))
    def test_fork_bypass_and_untrusted_events_rejected(self):
        for condition in ['always()',"always() || github.event_name == 'push'",'true']:
            with self.assertRaises(ValueError):module.workflow_gate(self.workflow(condition))
        fallback=self.workflow(module.GATE).replace(module.RUNS_ON, "${{ fromJSON(vars.WEBDOCK_PLUGIN_CI_RUNNER || '[\"ubuntu-24.04\"]') }}")
        with self.assertRaises(ValueError):module.workflow_gate(fallback)
        for event in ['pull_request_target','issue_comment','repository_dispatch']:
            with self.assertRaises(ValueError):module.workflow_gate(self.workflow(module.GATE,event))
    def test_only_fixed_native_sdk_and_optional_cms_names_are_eligible(self):
        self.assertEqual(len(module.REPOSITORIES),10)
        self.assertNotIn('webdock-extension-managed-registry',module.REPOSITORIES)
        self.assertNotIn('webdock-extension-job-demo',module.REPOSITORIES)
        self.assertNotIn(module.CMS,module.REPOSITORIES)


class PreserveApprovedCMS(unittest.TestCase):
    def test_default_refresh_preserves_previously_reviewed_cms(self):
        names=module.REPOSITORIES+[module.CMS]
        entries=[{'id':index,'name':name,'full_name':'spitzli/'+name} for index,name in enumerate(names,1)]
        writes=[]
        def api(path,method='GET',body=None):
            if method=='PUT':writes.append(body);return None
            return {'repositories':entries}
        def reviewed(name,already_selected=False):
            self.assertTrue(already_selected)
            row=next(item for item in entries if item['name']==name)
            return {'id':row['id'],'name':row['full_name'],'actionsEnabled':True}
        with patch.object(module,'group',return_value={'id':3}),patch.object(module,'api',side_effect=api),patch.object(module,'verify_repository',side_effect=reviewed):
            result=module.select()
        self.assertEqual(len(result['selectedRepositories']),11)
        self.assertEqual(len(writes[0]['selected_repository_ids']),11)


class OwnerManagedUnion(unittest.TestCase):
    def test_refresh_never_drops_later_owner_managed_repository_ids(self):
        with patch.object(module,'group',return_value={'id':3}),patch.object(module,'api',return_value={'repositories':[{'name':'webdock-extension-managed-analytics'}]}) as api:
            with self.assertRaises(ValueError):module.select()
            self.assertEqual(api.call_count,1)

if __name__=='__main__':unittest.main()
