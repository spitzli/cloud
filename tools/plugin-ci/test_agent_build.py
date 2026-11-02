import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

root=Path(__file__).parent
spec=importlib.util.spec_from_file_location('agent_build',root/'agent_build.py');build=importlib.util.module_from_spec(spec);spec.loader.exec_module(build)
spec=importlib.util.spec_from_file_location('agent_elf',root/'agent_elf.py');elf=importlib.util.module_from_spec(spec);spec.loader.exec_module(elf)

class FrozenSource(unittest.TestCase):
    def archive(self,directory,extra=None):
        target=directory/'source.tar'
        with tarfile.open(target,'w') as tar:
            for name in sorted(build.SOURCE_REQUIRED):
                raw=b'fixture source\n';entry=tarfile.TarInfo(name);entry.size=len(raw);tar.addfile(entry,io.BytesIO(raw))
            if extra:
                name,raw,kind=extra;entry=tarfile.TarInfo(name)
                if kind=='symlink':entry.type=tarfile.SYMTYPE;entry.linkname='/etc/passwd';tar.addfile(entry)
                else:entry.size=len(raw);tar.addfile(entry,io.BytesIO(raw))
        return target,hashlib.sha256(target.read_bytes()).hexdigest()

    def test_packages_exact_frozen_archive_without_git_or_replacement(self):
        with tempfile.TemporaryDirectory() as td:
            directory=Path(td);archive,digest=self.archive(directory);target=directory/'candidate'
            proof=build.prepare(archive,digest,target,'parent-reviewed-checkpoint')
            self.assertEqual(proof['sourceRef'],'sha256:'+digest)
            self.assertFalse(proof['productionActivationPerformed'])
            self.assertEqual(proof['features'],['kubernetes'])
            self.assertEqual(hashlib.sha256((target/'source.tar').read_bytes()).hexdigest(),digest)
            with self.assertRaises(ValueError):build.prepare(archive,digest,target,'parent-reviewed-checkpoint')
            with self.assertRaises(ValueError):build.prepare(archive,'0'*64,directory/'wrong','parent-reviewed-checkpoint')
            with self.assertRaises(ValueError):build.prepare(archive,digest,directory/'unfrozen','HEAD')

    def test_refuses_path_escape_links_and_runtime_private_inputs(self):
        for entry in [('bad/../../escape',b'x','file'),('link',b'','symlink'),('.runtime-test/secret.json',b'{}','file'),('private/config.json',b'{}','file'),('crates/agent/.env.production',b'TOKEN=secret','file'),('crates/agent/credential.key',b'key','file')]:
            with self.subTest(name=entry[0]),tempfile.TemporaryDirectory() as td:
                directory=Path(td);archive,digest=self.archive(directory,entry)
                with self.assertRaises(ValueError):build.prepare(archive,digest,directory/'candidate','parent-reviewed-checkpoint')
                self.assertFalse((directory/'candidate').exists())

class ABI(unittest.TestCase):
    def test_glibc_cap_rejects_trixie_and_private_abi(self):
        self.assertEqual(elf.glibc_requirements('GLIBC_2.2.5 GLIBC_2.34 GLIBC_2.36'),['2.2.5','2.34','2.36'])
        for value in ['GLIBC_2.41','GLIBC_PRIVATE GLIBC_2.36','no ABI records']:
            with self.assertRaises(ValueError):elf.glibc_requirements(value)
    def test_needed_shared_libraries_cannot_reference_paths(self):
        self.assertEqual(elf.needed_libraries('Shared library: [libc.so.6]'),['libc.so.6'])
        with self.assertRaises(ValueError):elf.needed_libraries('Shared library: [/private/lib.so]')


class RegressionEvidence(unittest.TestCase):
    def test_zero_filtered_or_missing_guard_tests_do_not_count_as_passed(self):
        with tempfile.TemporaryDirectory() as td:
            output=Path(td)
            for name,tests in build.REQUIRED_TESTS.items():
                lines=['test '+test+' ... ok' for test in tests]
                (output/name).write_text('\n'.join(lines)+'\ntest result: ok. 20 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n')
            self.assertEqual(build.validate_transcripts(output)['management-tests.log']['passed'],20)
            (output/'management-tests.log').write_text('test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 20 filtered out\n')
            with self.assertRaises(ValueError):build.validate_transcripts(output)


class InternalSourceLinks(unittest.TestCase):
    def test_internal_git_archive_sql_link_is_materialized_inside_source(self):
        with tempfile.TemporaryDirectory() as td:
            directory=Path(td);archive=directory/'source.tar'
            with tarfile.open(archive,'w') as tar:
                for name in sorted(build.SOURCE_REQUIRED|{'migrations/fixture.sql'}):
                    raw=b'fixture source\n';entry=tarfile.TarInfo(name);entry.size=len(raw);tar.addfile(entry,io.BytesIO(raw))
                link=tarfile.TarInfo('crates/persistence/migrations/fixture.sql');link.type=tarfile.SYMTYPE;link.linkname='../../../migrations/fixture.sql';tar.addfile(link)
            digest=hashlib.sha256(archive.read_bytes()).hexdigest();output=directory/'unpacked'
            build.unpack(archive,digest,output)
            target=output/'crates/persistence/migrations/fixture.sql'
            self.assertFalse(target.is_symlink());self.assertEqual(target.read_bytes(),b'fixture source\n')


class FixtureRefreshIdentity(unittest.TestCase):
    def test_only_fixture_change_can_reuse_software_proof(self):
        with tempfile.TemporaryDirectory() as td:
            root=Path(td)
            def archive(name,runtime,fixture):
                path=root/name
                with tarfile.open(path,'w') as data:
                    for filename,raw in [('crates/agent/src/legacy.rs',runtime),('crates/agent/tests/legacy_attachment.rs',fixture)]:
                        entry=tarfile.TarInfo(filename);entry.size=len(raw);data.addfile(entry,io.BytesIO(raw))
                return path
            previous=archive('previous.tar',b'runtime',b'before')
            current=archive('current.tar',b'runtime',b'after')
            self.assertTrue(build.fixture_only_change(previous,current)['allOtherSourceFilesUnchanged'])
            changed=archive('changed.tar',b'changed runtime',b'after')
            with self.assertRaises(ValueError):build.fixture_only_change(previous,changed)

class FixtureOnlyEvidence(unittest.TestCase):
    def test_fixture_only_reuses_proof_without_a_release_binary(self):
        with tempfile.TemporaryDirectory() as td:
            output=Path(td)
            reused={'sourceRef':'sha256:accepted','testTranscripts':{'full-agent-tests.log':{'passed':137,'ignored':25}},'sourceComparison':{'allOtherSourceFilesUnchanged':True}}
            (output/'checkpoint.json').write_text(json.dumps({'reusedSoftwareProof':reused}))
            (output/'legacy-attachment-test').write_bytes(b'fixture')
            self.assertEqual(build.validate_transcripts(output,'fixture-only')['full-agent-tests.log']['passed'],137)
            (output/'webdock-agent').write_bytes(b'unrequested release')
            with self.assertRaises(ValueError):build.validate_transcripts(output,'fixture-only')

if __name__=='__main__':unittest.main()
