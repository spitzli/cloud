import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tempfile
import unittest
import rename_environment as migration


class EnvironmentNames(unittest.TestCase):
    def test_dotenv_changes_keys_only_and_preserves_multiline_values(self):
        source = '\ufeff# private\r\n export WEBDOCK_DATABASE_URL = "postgres://WEBDOCK_literal"\r\nWEBDOCK_PEM="first\nWEBDOCK_LITERAL=not-a-key\nlast"\nNEXT_PUBLIC_WEBDOCK_API_ORIGIN=https://example.test\nOTHER=WEBDOCK_TOKEN\n'
        expected = source.replace('export WEBDOCK_DATABASE_URL', 'export SPITZLI_DATABASE_URL').replace('WEBDOCK_PEM=', 'SPITZLI_PEM=').replace('NEXT_PUBLIC_WEBDOCK_API_ORIGIN=', 'NEXT_PUBLIC_SPITZLI_API_ORIGIN=')
        result, changes, count = migration.dotenv(source)
        self.assertEqual(result, expected)
        self.assertEqual((len(changes), count), (3, 4))

    def test_escaped_quotes_backticks_comments_and_last_line(self):
        source = 'WEBDOCK_ONE="a\\"b" # comment\nWEBDOCK_TWO=`two\nWEBDOCK_INSIDE=literal`\nWEBDOCK_THREE=last'
        result, _, count = migration.dotenv(source)
        self.assertIn('SPITZLI_ONE="a\\"b" # comment', result)
        self.assertIn('WEBDOCK_INSIDE=literal', result)
        self.assertTrue(result.endswith('SPITZLI_THREE=last'))
        self.assertEqual(count, 3)

    def test_duplicate_and_conflicting_names_fail(self):
        for source in ['WEBDOCK_ONE=a\nSPITZLI_ONE=b\n', 'X=a\nX=b\n']:
            with self.subTest(source=source), self.assertRaises(migration.InvalidEnvironment):
                migration.dotenv(source)

    def test_malformed_env_is_not_executed_or_partially_parsed(self):
        for source in ['source private.env\n', 'WEBDOCK_ONE="missing', 'WEBDOCK_ONE="v" trailing', 'WEBDOCK_ONE=unquoted\\\n']:
            with self.subTest(source=source), self.assertRaises(migration.InvalidEnvironment):
                migration.dotenv(source)

    def test_dotenv_interpolation_is_explicitly_resolved_without_rewriting_values(self):
        for value in ['${WEBDOCK_ORIGIN}', '$WEBDOCK_ORIGIN', '"line\n${WEBDOCK_ORIGIN}\nend"']:
            with self.subTest(value=value), self.assertRaises(migration.InvalidEnvironment):
                migration.dotenv('WEBDOCK_TARGET=' + value)
        comment = 'WEBDOCK_ONE=value # ${WEBDOCK_DOC_ONLY}\nWEBDOCK_TWO="quoted" # $WEBDOCK_NOTE\n'
        result, _, _ = migration.dotenv(comment)
        self.assertEqual(result, comment.replace('WEBDOCK_ONE=', 'SPITZLI_ONE=').replace('WEBDOCK_TWO=', 'SPITZLI_TWO='))
        source = {'WEBDOCK_SECRET': '${WEBDOCK_LITERAL_PASSWORD}'}
        result, _, _ = migration.json_environment(json.dumps(source))
        self.assertEqual(json.loads(result)['SPITZLI_SECRET'], source['WEBDOCK_SECRET'])

    def test_json_preserves_opaque_values_and_types(self):
        source = {'WEBDOCK_ONE': 'WEBDOCK_VALUE', 'OTHER': 'ä\nline'}
        result, changes, count = migration.json_environment(json.dumps(source))
        self.assertEqual(json.loads(result), {'SPITZLI_ONE': source['WEBDOCK_ONE'], 'OTHER': source['OTHER']})
        self.assertEqual((len(changes), count), (1, 2))

    def test_kubernetes_keeps_metadata_and_base64_bytes(self):
        source = {'apiVersion': 'v1', 'kind': 'Secret', 'metadata': {'name': 'retained-key'}, 'data': {'WEBDOCK_AUTH_SECRET': 'V0VCRE9DS19WQUxVRQ==', 'tls.crt': 'QUJD'}, 'stringData': {'WEBDOCK_OTHER': 'untouched'}}
        result, changes, count = migration.json_environment(json.dumps(source))
        target = json.loads(result)
        self.assertEqual(target['metadata'], source['metadata'])
        self.assertEqual(target['data'], {'SPITZLI_AUTH_SECRET': source['data']['WEBDOCK_AUTH_SECRET'], 'tls.crt': 'QUJD'})
        self.assertEqual(target['stringData'], {'SPITZLI_OTHER': 'untouched'})
        self.assertEqual((len(changes), count), (2, 3))

    def test_json_conflicts_and_duplicate_keys_fail(self):
        for source in ['{"WEBDOCK_ONE":"a","SPITZLI_ONE":"b"}', '{"X":"a","X":"b"}', '{"X":true}', '[]', '{"apiVersion":"v1","kind":"Secret","data":{"WEBDOCK_X":"QQ=="},"stringData":{"SPITZLI_X":"b"}}']:
            with self.subTest(source=source), self.assertRaises(ValueError):
                migration.json_environment(source)

    def test_cli_private_output_dry_run_and_no_secret_logging(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            source, target = root / 'source.env', root / 'target.env'
            source.write_text('WEBDOCK_SECRET="literal-do-not-log-WEBDOCK_VALUE"\n')
            source.chmod(0o600)
            command = [sys.executable, str(Path(migration.__file__)), '--input', str(source)]
            dry = subprocess.run(command + ['--dry-run'], capture_output=True, text=True)
            self.assertEqual(dry.returncode, 0)
            self.assertNotIn('literal-do-not-log', dry.stdout + dry.stderr)
            self.assertFalse(target.exists())
            run = subprocess.run(command + ['--output', str(target)], capture_output=True, text=True)
            self.assertEqual(run.returncode, 0)
            self.assertEqual(target.read_text(), 'SPITZLI_SECRET="literal-do-not-log-WEBDOCK_VALUE"\n')
            self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o600)
            self.assertEqual(source.read_text(), 'WEBDOCK_SECRET="literal-do-not-log-WEBDOCK_VALUE"\n')
            refused = subprocess.run(command + ['--output', str(target)], capture_output=True, text=True)
            self.assertNotEqual(refused.returncode, 0)
            self.assertNotIn('literal-do-not-log', refused.stdout + refused.stderr)
            self.assertEqual(target.read_text(), 'SPITZLI_SECRET="literal-do-not-log-WEBDOCK_VALUE"\n')

    def test_symlinks_and_public_source_are_refused(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            source = root / 'source'; source.write_text('WEBDOCK_X=fixture\n'); source.chmod(0o644)
            with self.assertRaises(migration.InvalidEnvironment):
                migration.read_private(source)
            source.chmod(0o600)
            alias = root / 'alias'; alias.symlink_to(source)
            with self.assertRaises(OSError):
                migration.read_private(alias)
            with self.assertRaises(FileExistsError):
                migration.write_private(alias, 'SPITZLI_X=changed\n')
            self.assertEqual(source.read_text(), 'WEBDOCK_X=fixture\n')

    def test_fifo_and_oversize_source_are_refused(self):
        with tempfile.TemporaryDirectory() as raw:
            root = Path(raw)
            fifo = root / 'fifo'; os.mkfifo(fifo, 0o600)
            with self.assertRaises(migration.InvalidEnvironment):
                migration.read_private(fifo)
            large = root / 'large'; large.write_bytes(b'x' * (migration.LIMIT + 1)); large.chmod(0o600)
            with self.assertRaises(migration.InvalidEnvironment):
                migration.read_private(large)

    def test_output_permissions_are_explicit_under_restrictive_umask(self):
        with tempfile.TemporaryDirectory() as raw:
            target = Path(raw) / 'canonical.env'
            previous = os.umask(0o777)
            try:
                migration.write_private(target, 'SPITZLI_VALUE=fixture\n')
            finally:
                os.umask(previous)
            self.assertEqual(stat.S_IMODE(target.stat().st_mode), 0o600)


if __name__ == '__main__':
    unittest.main()
