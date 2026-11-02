import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
from contextlib import closing
from ntfy_recovery import snapshot, restore, RecoveryError


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='wd-ntfy-unit-')
        self.root = Path(self.temp.name)
        self.source = self.root / 'source'; self.source.mkdir()
        self.target = self.root / 'target'; self.target.mkdir()
        with closing(sqlite3.connect(self.source / 'cache.db')) as db, db:
            db.execute('CREATE TABLE messages(id text PRIMARY KEY, message text)')
            db.execute("INSERT INTO messages VALUES('one','kept')")
        (self.source / 'attachments').mkdir()
        (self.source / 'attachments' / 'one').write_bytes(b'attachment')
        self.identity = {'volumeUUID': 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', 'owner': 'local-operator', 'bytes': 67108864}
        self.target_identity = {**self.identity, 'volumeUUID': 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb'}
        self.config = {'image': 'binwiederhier/ntfy@sha256:' + 'a' * 64, 'profile': 'ntfy-sqlite-offline-v1'}
        self.backup = self.root / 'backup'
        self.fenced = True

    def tearDown(self): self.temp.cleanup()

    def check(self):
        if not self.fenced: raise RecoveryError('source_changed')

    def make(self, **kw):
        return snapshot(self.source, self.backup, self.identity, self.config, self.check, reserved_bytes=134217728, **kw)

    def test_offline_real_sql_and_files_survive_source_unavailability(self):
        receipt = self.make()
        self.source.rename(self.root / 'retained-unavailable')
        restore(self.backup, self.target, self.target_identity, receipt, reserved_bytes=134217728)
        with closing(sqlite3.connect(self.target / 'cache.db')) as db:
            self.assertEqual(db.execute('SELECT message FROM messages').fetchall(), [('kept',)])
        self.assertEqual((self.target / 'attachments' / 'one').read_bytes(), b'attachment')
        self.assertEqual((self.target / 'cache.db').stat().st_mode & 0o777, 0o600)
        marker = json.loads((self.target / '.webdock-restored.json').read_text())
        self.assertEqual(marker['target'], self.target_identity)
        self.assertEqual(marker['dispatch'], 'quarantined')

    def test_running_or_revoked_source_and_insufficient_peak_reservation_fail(self):
        self.fenced = False
        with self.assertRaises(RecoveryError): self.make()
        self.assertFalse(self.backup.exists())
        self.fenced = True
        with self.assertRaises(RecoveryError):
            snapshot(self.source, self.backup, self.identity, self.config, self.check, reserved_bytes=1)
        self.assertFalse(self.backup.exists())

    def test_same_volume_wrong_owner_tamper_and_partial_set_fail_closed(self):
        receipt = self.make()
        for identity in (self.identity, {**self.target_identity, 'owner': 'foreign'}):
            with self.assertRaises(RecoveryError): restore(self.backup, self.target, identity, receipt, reserved_bytes=134217728)
        (self.backup / 'cache.db').write_bytes(b'bad')
        with self.assertRaises(RecoveryError): restore(self.backup, self.target, self.target_identity, receipt, reserved_bytes=134217728)
        self.assertEqual(list(self.target.iterdir()), [])

    def test_symlink_unknown_file_and_external_mutable_dependency_are_rejected(self):
        (self.source / 'attachments' / 'escape').symlink_to('/etc/passwd')
        with self.assertRaises(RecoveryError): self.make()
        (self.source / 'attachments' / 'escape').unlink()
        (self.source / 'unknown.sqlite').write_bytes(b'unknown')
        with self.assertRaises(RecoveryError): self.make()
        (self.source / 'unknown.sqlite').unlink()
        self.config['database-url'] = 'external'
        with self.assertRaises(RecoveryError): self.make()

    def test_crashed_snapshot_has_no_complete_manifest_and_stale_source_callback_fails(self):
        calls = 0
        def revoked():
            nonlocal calls
            calls += 1
            if calls > 1: raise RecoveryError('revoked')
        with self.assertRaises(RecoveryError):
            snapshot(self.source, self.backup, self.identity, self.config, revoked, reserved_bytes=134217728)
        self.assertFalse((self.backup / 'manifest.json').exists())


if __name__ == '__main__': unittest.main()
