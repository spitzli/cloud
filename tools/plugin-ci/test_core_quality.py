import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec=importlib.util.spec_from_file_location('core_quality',Path(__file__).with_name('core_quality.py'))
q=importlib.util.module_from_spec(spec);spec.loader.exec_module(q)

class FrozenCoreQuality(unittest.TestCase):
    def archive(self,root,extra=None):
        path=root/'source.tar'
        with tarfile.open(path,'w') as archive:
            for name in sorted(q.REQUIRED):
                data=b"[package]\nname='owned-fixture'\nversion='0.1.0'\n" if name.endswith('Cargo.toml') else b'owned source fixture\n';member=tarfile.TarInfo(name);member.size=len(data);archive.addfile(member,io.BytesIO(data))
            if extra:
                name,data=extra;member=tarfile.TarInfo(name);member.size=len(data);archive.addfile(member,io.BytesIO(data))
        return path,hashlib.sha256(path.read_bytes()).hexdigest()

    def test_exact_archive_includes_full_quality_and_frontend_closure(self):
        with tempfile.TemporaryDirectory() as td:
            root=Path(td);archive,digest=self.archive(root,('packages/owned/src/file.ts',b'export const owned = true;'))
            manifest=q.prepare(archive,digest,root/'context','reviewed-private-source')
            self.assertEqual(manifest['sourceArchiveSHA256'],digest)
            self.assertEqual(manifest['cargoJobs'],3)
            self.assertFalse(manifest['productionActivationPerformed'])
            out=root/'source';q.unpack(archive,digest,out)
            self.assertTrue((out/'packages/owned/src/file.ts').exists())
            self.assertTrue((out/'docs/verification/2026-10-25-native-postgresql-backup-transitions.json').exists())
            self.assertTrue((out/'docs/superpowers/plans/2026-10-26-postgresql-restore-0056.draft.sql').exists())
            with self.assertRaises(ValueError):q.build(root/'context',digest,'focused',root/'context'/'sub'/'..'/'artifact')
            with self.assertRaises(ValueError):q.prepare(archive,digest,root/'context','reviewed-private-source')
            with self.assertRaises(ValueError):q.prepare(archive,'0'*64,root/'bad','reviewed-private-source')

    def test_ambient_configuration_and_runtime_state_are_rejected(self):
        for name in ['apps/console/.env.production','.runtime-test/state.json','config/live.private.json','.npmrc','auth/kubeconfig.yaml','private/auth.json']:
            with self.subTest(path=name),tempfile.TemporaryDirectory() as td:
                root=Path(td);archive,digest=self.archive(root,(name,b'fixture'))
                with self.assertRaises(ValueError):q.prepare(archive,digest,root/'context','reviewed-private-source')

    def test_required_compile_time_fixtures_cannot_be_omitted(self):
        for missing in ['docs/verification/2026-10-25-native-postgresql-backup-transitions.json','docs/superpowers/plans/2026-10-26-postgresql-restore-0056.draft.sql']:
            with self.subTest(path=missing),tempfile.TemporaryDirectory() as td:
                root=Path(td);complete,_=self.archive(root);incomplete=root/'incomplete.tar'
                with tarfile.open(complete) as source,tarfile.open(incomplete,'w') as target:
                    for member in source.getmembers():
                        if member.name!=missing:target.addfile(member,source.extractfile(member))
                digest=hashlib.sha256(incomplete.read_bytes()).hexdigest()
                with self.assertRaisesRegex(ValueError,'required quality'):
                    q.prepare(incomplete,digest,root/'context','reviewed-private-source')

    def test_local_cargo_source_cannot_be_silently_excluded_as_private(self):
        with tempfile.TemporaryDirectory() as td:
            root=Path(td)
            manifest=b"[package]\nname='consumer'\nversion='0.1.0'\n[dependencies]\nspitzli-secrets={path='../secrets'}\n"
            archive,digest=self.archive(root,('crates/consumer/Cargo.toml',manifest))
            with self.assertRaisesRegex(ValueError,'Missing frozen local Cargo dependency'):
                q.prepare(archive,digest,root/'context','reviewed-private-source')

    def test_core_dumps_and_compiled_agent_archives_never_enter_source(self):
        for name in ['crates/build/core','crates/build/core.1869255','legacy/console/public/hosting-agent.zip','apps/console/public/webdock-agent-v1.zip']:
            with self.subTest(path=name),tempfile.TemporaryDirectory() as td:
                root=Path(td);archive,digest=self.archive(root,(name,b'output fixture'))
                with self.assertRaisesRegex(ValueError,'Build output forbidden'):
                    q.prepare(archive,digest,root/'context','reviewed-private-source')

    def test_focused_checks_are_fixed_locked_local_contracts(self):
        plan=q.plan('focused')
        self.assertEqual([x['id'] for x in plan],['waitlist-access','waitlist-http','compute-process','hosting-repository-projection'])
        self.assertTrue(all('--locked' in x['args'] for x in plan))
        self.assertIn('plugins/lumaserv-compute/Cargo.toml',plan[2]['args'])
        self.assertIn('process',plan[2]['args'])
        self.assertFalse(any('include-ignored' in arg for item in plan for arg in item['args']))

    def test_bun_consumer_requires_real_nonzero_pass_and_contract_name(self):
        item=q.plan('consumer')[0]
        with self.assertRaises(ValueError):q.test_counts('0 pass\n0 fail',item)
        with self.assertRaises(ValueError):q.test_counts('1 pass\n0 fail',item)
        raw='(pass) '+item['requiredTests'][0]+'\n1 pass\n0 fail'
        self.assertEqual(q.test_counts(raw,item)['passed'],1)

    def test_root_build_uses_requested_npm_command_without_a_second_equivalent_build(self):
        plan=q.plan('full');builds=[x for x in plan if x['id']=='root-build']
        self.assertEqual(len(builds),1)
        self.assertEqual(builds[0]['args'],['npm','run','build'])
        self.assertFalse(any(x['args']==['bun','run','build'] for x in plan))
        self.assertFalse(any(x['args'][:2]==['npm','install'] for x in plan))

    def test_control_plane_carry_rejects_changed_library_lock_compiler_or_other_fixture(self):
        before={'crates/control-plane/src/main.rs':['0',420,'','same'],'Cargo.lock':['0',420,'','lock'],'rust-toolchain.toml':['0',420,'','compiler'],'crates/plugin-host/tests/support/local_postgres.rs':['0',420,'','old']}
        after={**before,'crates/plugin-host/tests/support/local_postgres.rs':['0',420,'','new']}
        self.assertEqual(q.control_plane_carry_delta(before,after),['crates/plugin-host/tests/support/local_postgres.rs'])
        for path in ['crates/control-plane/src/main.rs','Cargo.lock','rust-toolchain.toml','crates/plugin-runtime/src/lib.rs','crates/plugin-host/tests/support/other.rs','tools/plugin-ci/core-quality.Dockerfile']:
            with self.subTest(path=path),self.assertRaises(ValueError):q.control_plane_carry_delta(before,{**after,path:['0',420,'','changed']})

    def test_control_plane_carry_rejects_unchecked_proof_substitution(self):
        entries={'Cargo.lock':['0',420,'','original']};stdout='actual compile messages';stderr='actual compile stderr'
        record={'id':'control-plane-compile','command':q.CP_COMPILE_COMMAND,'exitCode':0,'stdoutSHA256':hashlib.sha256(stdout.encode()).hexdigest(),'stderrSHA256':hashlib.sha256(stderr.encode()).hexdigest()}
        proof={'sourceArchiveSHA256':q.CP_CARRY_SOURCE,'compileChecksPassed':True,'checks':[record]}
        assessment={'actualCompileProof':proof,'unmappedLocalArtifacts':[],'changedFixtureInControlPlaneArtifactClosure':False,'localCargoArtifactDepInfo':[{}]*66}
        carry={'originalSourceSHA256':q.CP_CARRY_SOURCE,'compileProofRaw':json.dumps(proof),'assessmentRaw':json.dumps(assessment),'originalEntries':entries,'stdoutRaw':stdout,'stderrRaw':stderr}
        with patch.multiple(q,CP_CARRY_PROOF_SHA=hashlib.sha256(carry['compileProofRaw'].encode()).hexdigest(),CP_CARRY_ASSESSMENT_SHA=hashlib.sha256(carry['assessmentRaw'].encode()).hexdigest(),CP_CARRY_MANIFEST_SHA=q.manifest_sha(entries)):
            self.assertEqual(q.validate_control_plane_carry_provenance(carry,q.CP_COMPILE_COMMAND),record)
            for change in [{'originalSourceSHA256':'fake'},{'stdoutRaw':'unchecked substitute'},{'assessmentRaw':'{}'},{'originalEntries':{}},{'compileProofRaw':'{}'}]:
                with self.subTest(change=list(change)),self.assertRaises(ValueError):q.validate_control_plane_carry_provenance({**carry,**change},q.CP_COMPILE_COMMAND)
            with self.assertRaises(ValueError):q.validate_control_plane_carry_provenance(carry,['cargo','test','--different-flags'])

    def test_recovery_dependency_closure_precedes_owned_database_start(self):
        text=Path(q.__file__).with_name('core-quality.Dockerfile').read_text()
        stage=text.split('FROM owned-database AS recovery\n',1)[1].split('FROM scratch AS recovery-artifact',1)[0]
        self.assertIn('bun install --frozen-lockfile --ignore-scripts',stage)
        self.assertNotIn('--filter',stage)
        for specifier in ["import('pg')","import('zod')","import('./tools/migrations/neon-enrollment-recovery.ts')"]:self.assertIn(specifier,stage)
        self.assertLess(stage.index("import('pg')"),stage.index('python3 /harness/core_local_fixtures.py'))

    def test_zero_filtered_tests_cannot_satisfy_focus(self):
        for item in q.plan('focused'):
            with self.assertRaises(ValueError):q.test_counts('test result: ok. 0 passed; 0 failed; 0 ignored; 1 filtered out',item)
            text='\n'.join('test '+name+' ... ok' for name in item['requiredTests'])+'\ntest result: ok. 3 passed; 0 failed; 0 ignored; 0 filtered out'
            self.assertEqual(q.test_counts(text,item)['failed'],0)
            if item['requiredTests']:
                with self.assertRaises(ValueError):q.test_counts('test result: ok. 3 passed; 0 failed; 0 ignored;',item)

if __name__=='__main__':unittest.main()
