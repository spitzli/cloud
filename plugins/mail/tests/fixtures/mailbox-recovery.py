#!/usr/bin/env python3
"""Bounded, uniquely owned TLS PG + controlled CE regression; no Core/cluster/S3.

Each case owns a fresh database. The cancellation case kills its own test process
AFTER accepted CE mutation, restarts only this runner's PG, then uses a new process.
Only redacted assertion codes and custody metadata survive temporary-key cleanup.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[4]
spec = importlib.util.spec_from_file_location('mail_fixture_support', ROOT/'tools/tests/mail-application-recovery.py')
support = importlib.util.module_from_spec(spec)
spec.loader.exec_module(support)
CASES = [
    ('mailbox_unknown_response_retains_custody', 'lost'),
    ('mailbox_unknown_response_retains_custody', 'timeout'),
    ('mailbox_unknown_response_retains_custody', 'http400'),
    ('reconciliation_partial_completion_retains_custody', 'provision'),
    ('reconciliation_partial_completion_retains_custody', 'suspend'),
    ('mailbox_success_without_metadata_persistence_retains_custody', ''),
    ('mailbox_success_without_metadata_persistence_retains_custody', 'skip'),
    ('reconciliation_success_without_metadata_persistence_retains_custody', ''),
    ('reconciliation_success_without_metadata_persistence_retains_custody', 'skip'),
    ('disconnected_metadata_writer_cannot_release_accepted_effect', ''),
    ('definite_success_and_never_started_release_only_their_own_custody', ''),
    ('prevalidation_failure_never_submits_a_provider_request', ''),
    ('wrong_token_and_stale_epoch_cannot_release_custody', ''),
    ('missing_mailbox_outcome_row_retains_custody', ''),
    ('missing_engine_outcome_row_retains_custody', ''),
    ('cancelled_mailbox_process_verify', ''),
]
SAFE_CODES = [
    'ambiguous_provider_outcome_lost_durable_custody',
    'uncertain_provider_was_acknowledged_drained', 'uncertain_source_resumed',
    'restart_replayed_unresolved_provider_effect', 'background_completion_deadline',
    'foreign_customer_completion_cleared_unrelated_effect', 'provider_accept_deadline',
]


def exact(name):
    return ('recovery::tests::' if name.startswith('wrong_token') else 'mailboxes::recovery_tests::') + name


def run(args):
    report = {'scope': 'review54#10 private Mail controlled CE; no production/Core/SMTP/S3 acceptance',
              'phase': args.phase, 'checks': [], 'head': subprocess.check_output(['git','rev-parse','HEAD'], cwd=ROOT, text=True).strip()}
    inventory_paths = [ROOT/p for p in ['plugins/mail/src/mailboxes.rs','plugins/mail/src/mailboxes/recovery_tests.rs','plugins/mail/src/recovery.rs','plugins/mail/src/recovery/tests.rs','plugins/mail/src/recovery.sql','plugins/mail/src/store.rs','plugins/mail/src/lib.rs','plugins/mail/src/engine.rs','plugins/mail/Cargo.toml','plugins/mail/Cargo.lock']] + [Path(__file__).resolve()]
    report['sourceInventory'] = {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in inventory_paths}
    build = ['cargo','test','--manifest-path','plugins/mail/Cargo.toml','--offline','--locked','--lib','--no-run','--message-format=json']
    result = subprocess.run(build, cwd=ROOT, capture_output=True, timeout=180)
    if result.returncode:
        print(result.stderr.decode(errors='replace'))
        raise RuntimeError('mail_fixture_build_failed')
    artifact = [json.loads(line) for line in result.stdout.splitlines() if line.startswith(b'{')]
    executable = next(Path(x['executable']) for x in artifact if x.get('reason')=='compiler-artifact' and x.get('executable') and x['target']['kind']==['lib'])
    report['buildCommand'] = build
    report['testExecutableSha256'] = hashlib.sha256(executable.read_bytes()).hexdigest()
    with tempfile.TemporaryDirectory(prefix='wd-mail-review10-') as directory:
        os.chmod(directory, 0o700)
        f = support.Fixture(directory, executable)
        try:
            pki = f.pki()
            container, port = f.postgres('review10', pki)
            f.sql(container, "CREATE ROLE mail_owner LOGIN PASSWORD 'controlled-fixture-only';")
            for index, (name, outcome) in enumerate(CASES):
                if args.case and name != args.case: continue
                database = 'review10_' + str(index)
                f.sql(container, f'CREATE DATABASE {database} OWNER mail_owner;')
                env = {**os.environ, 'WEBDOCK_MAIL_RECOVERY_TEST_URL': f'postgresql://mail_owner:controlled-fixture-only@127.0.0.1:{port}/{database}',
                       'WEBDOCK_MAIL_RECOVERY_TEST_CA': str(pki/'ca.crt'), 'WEBDOCK_MAIL_FIXTURE_OUTCOME': outcome}
                command = [str(executable), '--ignored', '--exact', exact(name), '--nocapture']
                if name == 'cancelled_mailbox_process_verify':
                    marker = Path(directory)/'accepted'
                    env['WEBDOCK_MAIL_FIXTURE_ACCEPTED'] = str(marker)
                    with (Path(directory)/'cancel-private.log').open('wb') as log:
                        child = subprocess.Popen([str(executable),'--ignored','--exact',exact('cancelled_mailbox_process_prepare')], cwd=ROOT, env=env, stdout=log, stderr=log, start_new_session=True)
                        try:
                            until = time.monotonic()+20
                            while not marker.exists():
                                if child.poll() is not None or time.monotonic() > until: raise RuntimeError('cancel_accept_deadline')
                                time.sleep(.05)
                            os.killpg(child.pid, signal.SIGKILL)
                            child.wait(timeout=5)
                            report['cancelledProcess'] = {'pid': child.pid, 'exitCode': child.returncode, 'acceptedBeforeKill': True, 'reaped': True}
                        finally:
                            if child.poll() is None:
                                os.killpg(child.pid, signal.SIGKILL)
                                child.wait(timeout=5)
                    f.inspect(container)  # Label + exact-ID proof before restarting own PG only.
                    f.docker('restart','--time','5',container)
                    port = f.inspect(container)['NetworkSettings']['Ports']['5432/tcp'][0]['HostPort']
                    env['WEBDOCK_MAIL_RECOVERY_TEST_URL'] = f'postgresql://mail_owner:controlled-fixture-only@127.0.0.1:{port}/{database}'
                    until = time.monotonic()+20
                    while f.docker('exec',container,'pg_isready','-h','127.0.0.1','-U','postgres',allowed=True).returncode:
                        if time.monotonic() > until: raise RuntimeError('restart_readiness_deadline')
                        time.sleep(.1)
                    report['ownedDatabaseRestarted'] = True
                started = time.monotonic()
                with subprocess.Popen(command, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True) as test_process:
                    try:
                        stdout, stderr = test_process.communicate(timeout=45)
                    except subprocess.TimeoutExpired:
                        os.killpg(test_process.pid, signal.SIGKILL)
                        test_process.communicate(timeout=5)
                        raise RuntimeError('owned_test_process_deadline')
                    result = subprocess.CompletedProcess(command, test_process.returncode, stdout, stderr)
                output = (result.stdout+result.stderr).decode(errors='replace')
                report['checks'].append({'test': exact(name), 'outcome': outcome, 'exitCode': result.returncode,
                                         'seconds': round(time.monotonic()-started, 2), 'processId': test_process.pid, 'processReaped': True, 'assertionCodes': [c for c in SAFE_CODES if c in output],
                                         'passed': 'test result: ok. 1 passed;' in output,
                                         'command': ['$owned_mail_lib_test','--ignored','--exact',exact(name),'--nocapture']})
                # Prove opaque custody survives ordinary dump serialization; never retain dump bytes.
                count = int(f.sql(container, 'SELECT count(*) FROM mail_recovery_effect;', database))
                dump = f.docker('exec', container, 'pg_dump', '-U', 'postgres', '-d', database,
                                '--data-only', '--table=mail_recovery_effect').stdout
                body = dump.split(b'FROM stdin;\n', 1)[1].split(b'\\.')[0]
                rows = len([line for line in body.splitlines() if line.strip()])
                if rows != count: raise RuntimeError('dump_effect_count_mismatch')
                report['checks'][-1]['privateDumpEffectRows'] = rows
                print(json.dumps(report['checks'][-1]), flush=True)
        except Exception as error:
            report['fixtureFailure'] = type(error).__name__
            if isinstance(error, RuntimeError): report['fixtureFailureCode'] = str(error)
            raise
        finally:
            report['containers'] = f.containers
            volumes = [{'containerId': item['id'], 'name': mount['Name']} for item in f.containers for mount in f.inspect(item['id'])['Mounts'] if mount['Type'] == 'volume']
            f.cleanup()
            for volume in volumes:
                removed = f.docker('volume', 'inspect', volume['name'], allowed=True)
                if not removed.returncode or b'no such volume' not in removed.stderr.lower(): raise RuntimeError('owned_volume_cleanup_unverified')
            report['volumes'] = volumes
            report['cleanup'] = {**f.report['cleanup'], 'volumesRemovedWithExactContainer': True,
                                 'providerFixtures': 'in test processes; all bounded processes exited or reaped',
                                 'temporaryPkiAndPrivateLogsRemoved': 'pending_temporary_directory_exit', 'minioKindK3sUnchanged': True}
            report['sourceInventoryUnchangedDuringRun'] = all(hashlib.sha256(p.read_bytes()).hexdigest() == report['sourceInventory'][str(p.relative_to(ROOT))] for p in inventory_paths)
            args.evidence.parent.mkdir(parents=True, exist_ok=True)
            args.evidence.write_text(json.dumps(report, indent=2)+'\n')
    if Path(directory).exists(): raise RuntimeError('temporary_directory_cleanup_failed')
    report['cleanup']['temporaryPkiAndPrivateLogsRemoved'] = True
    args.evidence.write_text(json.dumps(report, indent=2)+'\n')
    failed = [x for x in report['checks'] if x['exitCode'] or not x['passed']]
    if args.phase == 'green' and failed: raise RuntimeError('mail_fixture_green_assertions_failed')
    if args.phase == 'red' and not failed: raise RuntimeError('mail_fixture_red_not_observed')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--phase', choices=['red','green'], required=True)
    parser.add_argument('--case')
    parser.add_argument('--evidence', type=Path, required=True)
    run(parser.parse_args())
