import importlib.util
from pathlib import Path
import unittest
import tempfile
from types import SimpleNamespace
from unittest.mock import Mock, call, patch

spec=importlib.util.spec_from_file_location('local_fixture',Path(__file__).with_name('core_local_fixtures.py'))
f=importlib.util.module_from_spec(spec);spec.loader.exec_module(f)

class OwnedLocalFixtures(unittest.TestCase):
    def test_website_package_suite_requires_all_unit_and_http_cases_without_failures(self):
        self.assertEqual(f.website_package_counts('8 pass\n1 skip\n0 fail\n'),{'passed':8,'failed':0,'skipped':1})
        for report in ['0 pass\n8 skip\n0 fail\n','8 pass\n1 fail\n','7 pass\n0 fail\n','no result']:
            with self.subTest(report=report),self.assertRaises(ValueError):f.website_package_counts(report)

    def test_website_schema_is_provisioned_in_its_owned_database_before_payload(self):
        class SeedReached(Exception):pass
        pg=SimpleNamespace(password='owned-synthetic-fixture',port=54329,query=Mock())
        with tempfile.TemporaryDirectory() as td,patch.object(f,'ROOT',Path(__file__).resolve().parents[2]),patch.object(f.subprocess,'run',side_effect=SeedReached) as seed:
            with self.assertRaises(SeedReached):f.website_gate(pg,Path(td))
        database=pg.query.call_args_list[0].args[0].removeprefix('CREATE DATABASE ')
        self.assertRegex(database,r'^owned_website_[a-f0-9]{32}$')
        self.assertEqual(pg.query.call_args_list,[call('CREATE DATABASE '+database),call('CREATE SCHEMA webdock AUTHORIZATION quality;',database)])
        self.assertEqual(seed.call_args.args[0],['bun','--no-env-file','/harness/website_fixture_seed.ts'])
        self.assertTrue(seed.call_args.kwargs['env']['DATABASE_URL'].endswith('/'+database))

    def test_only_new_local_fixture_database_urls_are_accepted(self):
        f.assert_owned_url('postgresql://quality:synthetic@127.0.0.1:54329/owned_website_'+'a'*32)
        for url in ['postgresql://quality@db.example.test/owned_website_'+'a'*32,'postgresql://quality@127.0.0.1/production','postgresql://root@127.0.0.1/owned_website_'+'a'*32]:
            with self.assertRaises(ValueError):f.assert_owned_url(url)

    def test_real_ambient_configuration_is_not_inherited(self):
        with patch.dict(f.os.environ,{'PATH':'/usr/bin','DATABASE_URL':'postgres://production','CMS_URL':'https://live','PAYLOAD_SECRET':'real','GH_TOKEN':'real','HOME':'/home/quality'},clear=True):
            env=f.clean_environment()
        self.assertEqual(env['PATH'],'/usr/bin')
        self.assertFalse(any(k in env for k in ['DATABASE_URL','CMS_URL','PAYLOAD_SECRET','GH_TOKEN']))

    def test_recovery_url_is_generated_only_for_owned_loopback_owner_database(self):
        url=f.recovery_url(54329,'a'*64,'owned_neon_recovery_'+'b'*32+'_test')
        self.assertTrue(url.startswith('postgresql://neondb_owner:'))
        self.assertIn('@127.0.0.1:54329/owned_neon_recovery_',url)
        self.assertTrue(url.endswith('_test'))
        for db in ['production','owned_website_'+'b'*32,'owned_neon_recovery_test']:
            with self.assertRaises(ValueError):f.recovery_url(54329,'a'*64,db)

    def test_recovery_owner_starts_restricted_and_bootstrap_owns_database(self):
        commands=f.recovery_setup('a'*64,'owned_neon_recovery_'+'b'*32+'_test')
        for flag in ['NOSUPERUSER','NOBYPASSRLS','NOINHERIT','NOCREATEDB','NOCREATEROLE']:self.assertIn(flag,commands[0])
        self.assertNotIn('LOGIN SUPERUSER',commands[0])
        self.assertIn('OWNER quality',commands[1])
        self.assertNotIn('GRANT', ''.join(commands))
        owner=f.recovery_url(54329,'a'*64,'owned_neon_recovery_'+'b'*32+'_test')
        bootstrap=f.recovery_bootstrap_url(54329,'c'*64,'owned_neon_recovery_'+'b'*32+'_test')
        self.assertIn('://quality:',bootstrap)
        self.assertEqual(owner.split('@')[1],bootstrap.split('@')[1])

    def test_old_notice_rows_and_permissions_must_remain_byte_equivalent(self):
        before=[{'id':'1','privacy_version':'2026-10-31','version':3},{'id':'2','privacy_version':'legacy','version':1}]
        f.verify_migration(before,before,{'privacy_version':'2026-11-01','version':1},'owned-policy','owned-policy')
        changed=[{**before[0],'privacy_version':'2026-11-01'},before[1]]
        with self.assertRaises(ValueError):f.verify_migration(before,changed,{'privacy_version':'2026-11-01','version':1},'same','same')
        with self.assertRaises(ValueError):f.verify_migration(before,before,{'privacy_version':'2026-10-31','version':1},'same','same')
        with self.assertRaises(ValueError):f.verify_migration(before,before,{'privacy_version':'2026-11-01','version':1},'old','changed')

if __name__=='__main__':unittest.main()
