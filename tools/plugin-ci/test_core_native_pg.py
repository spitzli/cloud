import importlib.util
from pathlib import Path
import json
import unittest

spec=importlib.util.spec_from_file_location('native_pg',Path(__file__).with_name('core_native_pg.py'))
f=importlib.util.module_from_spec(spec);spec.loader.exec_module(f)

class NativePostgresPlan(unittest.TestCase):
    def test_compile_covers_policy_integration_and_all_affected_control_plane_targets(self):
        plan=f.compile_plan()
        self.assertEqual(len(plan),3)
        self.assertTrue(all('--locked' in item['args'] for item in plan))
        self.assertIn(f.POLICY_TEST,plan[0]['args'])
        self.assertIn('--exact',plan[0]['args'])
        self.assertIn('resource_admission',plan[1]['args'])
        self.assertIn('--no-run',plan[1]['args'])
        for flag in ['--lib','--bins','--tests','--no-run']:self.assertIn(flag,plan[2]['args'])
    def test_runtime_executes_only_reviewed_ignored_test(self):
        args=f.runtime_plan()
        self.assertEqual(args[0],'/results/native-compile/retirement-test')
        self.assertIn(f.RETIREMENT_TEST,args)
        for flag in ['--exact','--ignored','--nocapture','--test-threads=1']:self.assertIn(flag,args)
        self.assertFalse(any('include-ignored' in a or a=='cargo' for a in args))
    def test_recovery_support_module_is_verified_through_bootstrap_target(self):
        package='path+file:///src/crates/control-plane#spitzli-control-plane@0.1.0'
        def item(name,kind,source):return {'package_id':package,'manifest_path':'/src/crates/control-plane/Cargo.toml','target':{'name':name,'kind':[kind],'src_path':source},'profile':{'test':True},'executable':'/quality-target/debug/deps/'+name}
        records=[item(name,'bin','/src/crates/control-plane/src/main.rs') for name in ['spitzli-control-plane','spitzli-plugin-controller','spitzli-plugin-control','spitzli-agent-control']]
        bootstrap=item('bootstrap','test','/src/crates/control-plane/tests/bootstrap.rs')
        self.assertEqual(f.control_plane_artifacts([*records,bootstrap],package)['recoveryCompileTarget'],'bootstrap')
        for wrong in [{**bootstrap,'package_id':'path+file:///src/other#other@0.1.0'},{**bootstrap,'target':{**bootstrap['target'],'src_path':'/src/other/bootstrap.rs'}},{**bootstrap,'target':{**bootstrap['target'],'name':'recovery_mail_component'}}]:
            with self.assertRaises(ValueError):f.control_plane_artifacts([*records,wrong],package)

    def test_compiler_message_must_identify_unique_private_target_test_binary(self):
        item={'reason':'compiler-artifact','target':{'name':'resource_admission'},'profile':{'test':True},'executable':'/quality-target/debug/deps/resource_admission-fixture'}
        self.assertEqual(str(f.integration_artifact(json.dumps(item))),item['executable'])
        for raw in ['',json.dumps(item)+'\n'+json.dumps(item),json.dumps({**item,'executable':'/tmp/unreviewed'})]:
            with self.assertRaises(ValueError):f.integration_artifact(raw)

if __name__=='__main__':unittest.main()
