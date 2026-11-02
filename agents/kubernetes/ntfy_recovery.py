"""DISPOSABLE profile demonstration: offline ntfy SQLite + local attachments.

The trusted native operation executor supplies a live stop/ownership/lease check and
already reserved retained source/fresh target VolumeRecord identities. This module
never grants authority, stops a workload, resumes it, mounts S3, or changes pool state.
The local Docker fixture qualifies data recovery separately from native integration.
"""
from contextlib import closing, contextmanager
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import sqlite3
import stat
import time
import uuid

LIMIT = 64 * 1024**3
PROFILE = 'ntfy-sqlite-offline-v1'
DATABASES = {'cache.db', 'auth.db'}


class RecoveryError(ValueError): pass


def require(ok, message):
    if not ok: raise RecoveryError(message)


def private(path, directory=False):
    st = path.lstat()
    require((stat.S_ISDIR(st.st_mode) if directory else stat.S_ISREG(st.st_mode) and st.st_nlink == 1)
            and not stat.S_ISLNK(st.st_mode) and st.st_uid == os.getuid(), 'untrusted_recovery_path')
    return st


def identity(value):
    # The adapter supplies these from its observed VolumeRecord, never from a client.
    require(set(value) == {'volumeUUID', 'owner', 'bytes'}, 'invalid_volume_identity')
    require(str(uuid.UUID(value['volumeUUID'])) == value['volumeUUID'] and bool(value['owner'])
            and type(value['bytes']) is int and 67108864 <= value['bytes'] <= LIMIT, 'invalid_volume_identity')


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024*1024), b''): h.update(block)
    return h.hexdigest()


def write(path, data):
    with os.fdopen(os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600), 'wb') as stream:
        stream.write(data); stream.flush(); os.fsync(stream.fileno())


def copy(source, target):
    private(source)
    with source.open('rb') as source_stream, os.fdopen(os.open(target, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600), 'wb') as target_stream:
        shutil.copyfileobj(source_stream, target_stream, 1024*1024)
        target_stream.flush(); os.fsync(target_stream.fileno())


def sync_directory(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try: os.fsync(fd)
    finally: os.close(fd)


@contextmanager
def reservation(directory, amount, capacity):
    require(type(amount) is int and amount >= 2 * capacity and amount <= 2 * LIMIT, 'restore_peak_not_reserved')
    require(shutil.disk_usage(directory).free >= amount + capacity, 'insufficient_restore_peak')
    path = directory / ('.recovery-reservation-' + uuid.uuid4().hex)
    fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    try:
        os.posix_fallocate(fd, 0, amount)
        yield
    finally:
        os.close(fd); path.unlink(); sync_directory(directory)


def sql_receipt(path):
    with closing(sqlite3.connect(path.as_uri() + '?mode=ro', uri=True, timeout=2)) as db:
        require(db.execute('PRAGMA integrity_check').fetchall() == [('ok',)], 'sqlite_integrity_failed')
        schema = db.execute("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name").fetchall()
        return {'schemaSHA256': hashlib.sha256(json.dumps(schema, separators=(',', ':')).encode()).hexdigest(),
                'userVersion': db.execute('PRAGMA user_version').fetchone()[0]}


def source_files(source):
    paths = []
    for item in source.iterdir():
        if item.name == 'attachments':
            private(item, True)
            for attachment in item.iterdir():
                require(bool(re.fullmatch(r'[A-Za-z0-9_-]{1,128}', attachment.name)), 'invalid_attachment')
                private(attachment); paths.append(attachment)
        elif item.name in DATABASES: private(item); paths.append(item)
        elif any(item.name == name + suffix for name in DATABASES for suffix in ('-wal', '-shm', '-journal')):
            private(item)  # SQLite backup incorporates WAL after all writers have stopped.
        else: raise RecoveryError('unsupported_ntfy_volume_file')
    require((source / 'cache.db').is_file() and len(paths) <= 10000, 'invalid_ntfy_volume')
    return paths


def snapshot(source, output, source_identity, configuration, check_fence, *, reserved_bytes, timeout=120):
    source = Path(source).absolute(); output = Path(output).absolute()
    identity(source_identity); private(source, True); private(output.parent, True)
    require(set(configuration) == {'image', 'profile'} and configuration['profile'] == PROFILE
            and bool(re.fullmatch(r'binwiederhier/ntfy@sha256:[a-f0-9]{64}', configuration['image'])), 'unsupported_consistency_profile')
    require(0 < timeout <= 120, 'invalid_recovery_deadline')
    deadline = time.monotonic() + timeout
    def check():
        require(time.monotonic() < deadline, 'recovery_deadline'); check_fence()
    check(); paths = source_files(source)
    require(sum(p.stat().st_size for p in paths) <= source_identity['bytes'], 'source_capacity_exceeded')
    with reservation(output.parent, reserved_bytes, source_identity['bytes']):
        output.mkdir(mode=0o700)
        manifest = {'version': 1, 'identity': source_identity, 'configuration': configuration, 'files': {}}
        for path in paths:
            check(); name = path.relative_to(source).as_posix(); target = output / name
            if path.name in DATABASES:
                write(target, b'')
                with closing(sqlite3.connect(path.as_uri() + '?mode=ro', uri=True, timeout=2)) as src, closing(sqlite3.connect(target)) as dst:
                    src.backup(dst, pages=256, progress=lambda *_: check(), sleep=0.01)
                sql = sql_receipt(target)
            else:
                target.parent.mkdir(mode=0o700, exist_ok=True); copy(path, target); sql = None
            manifest['files'][name] = {'sha256': digest(target), 'bytes': target.stat().st_size, 'sql': sql}
        check()
        data = json.dumps(manifest, sort_keys=True, separators=(',', ':')).encode()
        write(output / 'manifest.json', data); sync_directory(output)
        return {'sha256': hashlib.sha256(data).hexdigest(), 'bytes': len(data)}


def restore(backup, target, target_identity, receipt, *, reserved_bytes, check_authority=lambda: None):
    backup = Path(backup).absolute(); target = Path(target).absolute()
    private(backup, True); private(target, True); identity(target_identity)
    require(not list(target.iterdir()), 'fresh_empty_target_required')
    private(backup / 'manifest.json')
    data = (backup / 'manifest.json').read_bytes()
    require(len(data) <= 1024*1024 and receipt == {'sha256': hashlib.sha256(data).hexdigest(), 'bytes': len(data)}, 'recovery_manifest_mismatch')
    manifest = json.loads(data); source_identity = manifest['identity']; identity(source_identity)
    require(manifest['version'] == 1 and manifest['configuration']['profile'] == PROFILE, 'unsupported_consistency_profile')
    require(source_identity['owner'] == target_identity['owner'] and source_identity['volumeUUID'] != target_identity['volumeUUID'], 'fresh_owned_volume_required')
    require(source_identity['bytes'] <= target_identity['bytes'], 'target_capacity_insufficient')
    files = manifest['files']; require(1 <= len(files) <= 10000 and 'cache.db' in files, 'incomplete_recovery_set')
    require(sum(v['bytes'] for v in files.values()) <= target_identity['bytes'], 'target_capacity_exceeded')
    for name, value in files.items():
        parts = PurePosixPath(name).parts
        require(name in DATABASES or len(parts) == 2 and parts[0] == 'attachments' and bool(re.fullmatch(r'[A-Za-z0-9_-]{1,128}', parts[1])), 'invalid_recovery_path')
        path = backup / name; private(path)
        require(path.stat().st_size == value['bytes'] and digest(path) == value['sha256'], 'recovery_file_mismatch')
        if name in DATABASES: require(sql_receipt(path) == value['sql'], 'recovery_schema_mismatch')
    # Revalidate authority before any target bytes and before completion. Failure leaves a
    # partial target closed, never an auto-resumable deployment.
    check_authority()
    with reservation(target.parent, reserved_bytes, target_identity['bytes']):
        write(target / '.webdock-recovery-incomplete', b'quarantined\n')
        for name in files:
            check_authority(); path = target / name
            path.parent.mkdir(mode=0o700, exist_ok=True); copy(backup / name, path)
            require(digest(path) == files[name]['sha256'], 'target_integrity_mismatch')
        check_authority()
        write(target / '.webdock-restored.json', json.dumps({'source': source_identity, 'target': target_identity, 'dispatch': 'quarantined', 'manifest': receipt}).encode())
        (target / '.webdock-recovery-incomplete').unlink(); sync_directory(target)
    return manifest['configuration']
