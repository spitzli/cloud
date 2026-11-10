#!/usr/bin/env python3
"""Owned maximum gateway+publisher overlap, bounded cgroup and cached local registry.
The external generator/proxy uses 64KiB buffers. No provider/live build endpoints
are contacted; registry backpressure holds a real OCI push during collection.
"""
import argparse
import datetime
import hashlib
import http.client
import importlib.util
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse

ROOT = Path(__file__).resolve().parents[2]
REGISTRY = 'registry@sha256:ddf754342cfc8acc51a56d5d0ab6af06826461864460636d8bd5c546dab2a7b8'
BUDGET = 6 * 1024**3
MIN_AVAILABLE = 12 * 1024**3
spec = importlib.util.spec_from_file_location('maximum_fixture', Path(__file__).with_name('build-upload-gateway-maximum.py'))
maximum = importlib.util.module_from_spec(spec)
spec.loader.exec_module(maximum)


def file_hash(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as file:
        while chunk := file.read(maximum.CHUNK):
            digest.update(chunk)
    return digest.hexdigest()


def admit_memory():
    available = int(next(line.split()[1] for line in Path('/proc/meminfo').read_text().splitlines() if line.startswith('MemAvailable:'))) * 1024
    if available < MIN_AVAILABLE:
        raise RuntimeError('at_least_12GiB_available_required')
    return available


def docker(*args):
    result = subprocess.run(['docker', *args], capture_output=True, text=True, timeout=20)
    if result.returncode:
        raise RuntimeError('owned_registry_' + args[0] + '_failed')
    return result.stdout.strip()


class RegistryProxy:
    def __init__(self, backend):
        self.backend = backend
        self.started = threading.Event()
        self.release = threading.Event()
        self.lock = threading.Lock()
        self.first = True
        self.bytes = 0
        self.held_at = None
        self.released_at = None
        owner = self
        class Handler(BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'
            def log_message(self, *args):
                pass
            def forward(self):
                upstream = http.client.HTTPConnection(*owner.backend, timeout=35)
                try:
                    length = int(self.headers.get('Content-Length', '0'))
                    assert 0 <= length <= maximum.MAX_OUTPUT
                    upstream.putrequest(self.command, self.path, skip_host=True, skip_accept_encoding=True)
                    for name, value in self.headers.items():
                        if name.lower() not in ['connection', 'transfer-encoding', 'accept-encoding']:
                            upstream.putheader(name, value)
                    upstream.putheader('Connection', 'close')
                    upstream.endheaders()
                    hold = False
                    if self.command == 'PATCH' and length >= maximum.CHUNK:
                        with owner.lock:
                            if owner.first:
                                owner.first = False
                                hold = True
                    remaining = length
                    while remaining:
                        chunk = self.rfile.read(min(maximum.CHUNK, remaining))
                        if not chunk:
                            raise RuntimeError('publisher_body_truncated')
                        upstream.send(chunk)
                        remaining -= len(chunk)
                        with owner.lock:
                            owner.bytes += len(chunk)
                        if hold:
                            owner.held_at = time.monotonic()
                            owner.started.set()
                            if not owner.release.wait(25):
                                raise RuntimeError('collector_overlap_deadline')
                            owner.released_at = time.monotonic()
                            hold = False
                    response = upstream.getresponse()
                    self.send_response(response.status)
                    for name, value in response.getheaders():
                        if name.lower() in ['connection', 'transfer-encoding']:
                            continue
                        if name.lower() == 'location' and value.startswith('http'):
                            url = urlparse(value)
                            value = 'http://127.0.0.1:' + str(owner.server.server_port) + url.path + ('?' + url.query if url.query else '')
                        self.send_header(name, value)
                    self.send_header('Connection', 'close')
                    self.end_headers()
                    if self.command != 'HEAD':
                        while chunk := response.read(maximum.CHUNK):
                            self.wfile.write(chunk)
                finally:
                    upstream.close()
            do_GET = do_HEAD = do_POST = do_PATCH = do_PUT = forward
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        assert self.server.server_port not in [19443, 19444, 19445]
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
    def close(self):
        self.release.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)


def case(binary, ingress, publication):
    available = admit_memory()
    run = secrets.token_hex(8)
    work = Path(tempfile.mkdtemp(prefix='worker-max-', dir=ROOT / '.runtime-test'))
    work.chmod(0o700)
    (work / 'gateway-tmpfs').mkdir(mode=0o700)
    (work / 'store').mkdir(mode=0o700)
    unit = 'webdock-worker-memory-' + run + '.service'
    registry = proxy = None
    sampled = {'memoryPeakBytes': 0, 'rssPeakBytes': 0, 'events': {}}
    stop = threading.Event()
    sampler = None
    report = {'memoryMaxBytes': BUDGET, 'memorySwapMaxBytes': 0, 'availableBeforeCaseBytes': available, 'uploadAccepted': False, 'collectorAccepted': False, 'publisherAccepted': False}
    try:
        with (work / 'publication-input.tar').open('xb') as file:
            file.write(maximum.header('image.tar', publication.image_size))
            for chunk in publication.chunks():
                file.write(chunk)
            file.write(bytes(1024))
            file.flush()
            os.fsync(file.fileno())
            assert file.tell() == maximum.MAX_OUTPUT
            os.posix_fadvise(file.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
        registry = docker('run', '--pull=never', '-d', '--label', 'webdock.worker-memory=' + run, '--memory=1g', '--memory-swap=1g', '--cpus=1', '--pids-limit=64', '--read-only', '--tmpfs', '/var/lib/registry:rw,nosuid,nodev,size=768m', '--tmpfs', '/tmp:rw,nosuid,nodev,size=16m', '-p', '127.0.0.1::5000', '-e', 'REGISTRY_HTTP_SECRET=owned-worker-memory-fixture', '-e', 'REGISTRY_HTTP_RELATIVEURLS=true', REGISTRY)
        info = json.loads(docker('inspect', registry))[0]
        assert info['Id'] == registry and info['Config']['Labels']['webdock.worker-memory'] == run
        assert not info['HostConfig'].get('Binds')
        assert info['HostConfig']['Memory'] == info['HostConfig']['MemorySwap'] == 1024**3
        host, port = docker('port', registry, '5000/tcp').split(':')
        assert host == '127.0.0.1' and int(port) not in [19443, 19444, 19445]
        backend = (host, int(port))
        for _ in range(50):
            try:
                connection = http.client.HTTPConnection(*backend, timeout=1)
                connection.request('GET', '/v2/')
                assert connection.getresponse().status == 200
                connection.close()
                break
            except (OSError, http.client.HTTPException):
                time.sleep(0.1)
        else:
            raise RuntimeError('owned_registry_not_ready')
        proxy = RegistryProxy(backend)
        maximum.private_write(work / 'config.json', {'registryAddress': '127.0.0.1:' + str(proxy.server.server_port)})
        script = 'mount --make-rprivate / && mount -t tmpfs -o size=671088640,mode=0700,nosuid,nodev,noexec owned-worker-maximum "$WEBDOCK_WORKER_MAX_WORK/gateway-tmpfs" && exec "$1" --exact maximum_worker_overlap_service --ignored --nocapture --test-threads=1'
        command = ['systemd-run', '--user', '--quiet', '--unit=' + unit, '--property=MemoryMax=' + str(BUDGET), '--property=MemorySwapMax=0', '--property=TasksMax=32', '--property=CPUQuota=200%', '--property=RuntimeMaxSec=300', '--property=TimeoutStopSec=5', '--property=OOMPolicy=stop', '--property=StandardOutput=null', '--property=StandardError=null', '--setenv=WEBDOCK_WORKER_MAX_WORK=' + str(work)]
        command += ['--setenv=' + name + '=' for name in ['HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'http_proxy', 'https_proxy', 'all_proxy']]
        command += ['--setenv=NO_PROXY=*', '--setenv=no_proxy=*', 'unshare', '--user', '--map-root-user', '--map-auto', '--mount', '--propagation', 'private', '/bin/bash', '-c', script, 'owned-worker-memory-fixture', str(binary)]
        subprocess.run(command, check=True, capture_output=True, timeout=20)
        control = maximum.wait_file(work / 'control.json', lambda value: True, unit, seconds=60)
        state = maximum.show(unit)
        pid = int(state['MainPID'])
        cgroup = Path('/sys/fs/cgroup') / state['ControlGroup'].lstrip('/')
        assert int((cgroup / 'memory.max').read_text()) == BUDGET
        assert int((cgroup / 'memory.swap.max').read_text()) == 0
        def sample():
            while not stop.wait(0.02):
                try:
                    sampled['memoryPeakBytes'] = max(sampled['memoryPeakBytes'], int((cgroup / 'memory.peak').read_text()))
                    sampled['events'] = {key: int(value) for key, value in (line.split() for line in (cgroup / 'memory.events').read_text().splitlines())}
                    status = Path('/proc', str(pid), 'status').read_text()
                    sampled['rssPeakBytes'] = max(sampled['rssPeakBytes'], int(next(line.split()[1] for line in status.splitlines() if line.startswith('VmHWM:'))) * 1024)
                except (OSError, StopIteration):
                    return
        sampler = threading.Thread(target=sample, daemon=True)
        sampler.start()
        maximum.private_write(work / 'command.json', 'publish')
        maximum.wait_file(work / 'publisher-ready.json', lambda value: value['parsed'], unit, seconds=60)
        connection = maximum.connect(control)
        try:
            sent = 0
            for chunk in ingress.result(control['identity']):
                maximum.chunk(connection, chunk)
                sent += len(chunk)
            assert sent == maximum.MAX_RESULT
            connection.send(b'0\r\n\r\n')
            response = connection.getresponse()
            report['uploadStatus'] = response.status
            report['uploadAccepted'] = response.status == 200
            response.read(128)
        finally:
            connection.close()
        if not report['uploadAccepted']:
            raise RuntimeError('maximum_ingress_failed')
        maximum.private_write(work / 'command.json', 'allow-push')
        if not proxy.started.wait(15):
            raise RuntimeError('actual_publisher_push_not_started')
        report['realRegistryBlobBytesBeforeCollector'] = proxy.bytes
        maximum.private_write(work / 'command.json', 'collect')
        maximum.wait_file(work / 'collector-started.json', lambda value: value['started'], unit)
        finished = maximum.wait_file(work / 'collector-finished.json', lambda value: value['collected'], unit, seconds=25)
        assert proxy.released_at is None, 'push must remain backpressured across actual collector verification'
        report['collectorAccepted'] = True
        report['collectorPeakRssBytes'] = finished['peakRssBytes']
        report['physicalGatewayBytesAfterRetirement'] = finished['tmpfsUsedBytes']
        proxy.release.set()
        progress = maximum.wait_file(work / 'progress.json', lambda value: value['published'], unit, seconds=60)
        report['publisherAccepted'] = True
        report['publisherGuardChecks'] = progress['publisherChecks']
        assert progress['collected'] and progress['tmpfsUsedBytes'] == 0 and progress['publisherChecks'] > 5
        # Independent bounded GET readback from the actual cached registry, not the proxy.
        connection = http.client.HTTPConnection(*backend, timeout=30)
        connection.request('GET', '/v2/customers/1002/projects/2002/manifests/release-4002', headers={'Accept': 'application/vnd.oci.image.manifest.v1+json'})
        response = connection.getresponse()
        assert response.status == 200
        manifest = response.read(65537)
        assert len(manifest) <= 65536
        assert 'sha256:' + hashlib.sha256(manifest).hexdigest() == control['publisherManifestDigest']
        connection.close()
        connection = http.client.HTTPConnection(*backend, timeout=30)
        connection.request('GET', '/v2/customers/1002/projects/2002/blobs/sha256:' + publication.layer_sha)
        response = connection.getresponse()
        assert response.status == 200
        digest = hashlib.sha256()
        size = 0
        while chunk := response.read(maximum.CHUNK):
            digest.update(chunk)
            size += len(chunk)
        connection.close()
        assert size == publication.layer_size and digest.hexdigest() == publication.layer_sha
        report.update(independentManifestVerified=True, independentMaximumLayerVerified=True, maximumLayerBytes=size, publisherArtifactBytes=maximum.MAX_OUTPUT, publisherInnerOciBytes=publication.image_size, gatewayImageBytes=maximum.MAX_OUTPUT, gatewayResultBytes=maximum.MAX_RESULT, actualPushBackpressureSeconds=round(proxy.released_at - proxy.held_at, 3), actualOverlapVerified=True)
        observed = maximum.show(unit)
        report.update(systemdResult=observed['Result'], memoryPeakBytes=max(sampled['memoryPeakBytes'], int(observed['MemoryPeak'])), independentPeakRssBytes=sampled['rssPeakBytes'], cgroupMemoryEvents=sampled['events'], remainingAt6GiBBytes=BUDGET-max(sampled['memoryPeakBytes'], int(observed['MemoryPeak'])))
        maximum.private_write(work / 'command.json', 'quit')
        for _ in range(100):
            if maximum.show(unit).get('ActiveState') != 'active':
                break
            time.sleep(0.05)
        report['finalSystemdResult'] = maximum.show(unit).get('Result')
        assert report['finalSystemdResult'] == 'success'
    except Exception as failure:
        report['failure'] = type(failure).__name__
        observed = maximum.show(unit)
        report.update(systemdResult=observed.get('Result'), memoryPeakBytes=sampled['memoryPeakBytes'], cgroupMemoryEvents=sampled['events'])
    finally:
        if proxy:
            proxy.release.set()
        subprocess.run(['systemctl', '--user', 'stop', unit], capture_output=True, timeout=10)
        stop.set()
        if sampler:
            sampler.join(timeout=2)
        if proxy:
            proxy.close()
        if any((work / 'gateway-tmpfs').iterdir()):
            raise RuntimeError('unexpected_underlying_gateway_payload_retained')
        if registry:
            info = json.loads(docker('inspect', registry))[0]
            assert info['Id'] == registry and info['Config']['Labels']['webdock.worker-memory'] == run
            docker('rm', '-f', '-v', registry)
        shutil.rmtree(work)
        subprocess.run(['systemctl', '--user', 'reset-failed', unit], capture_output=True, timeout=10)
        report['exactOwnedCleanup'] = True
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--evidence', type=Path, default=ROOT / 'docs/verification/2026-10-30-build-worker-overlap-memory.json')
    args = parser.parse_args()
    available = admit_memory()
    docker('image', 'inspect', REGISTRY)
    built = subprocess.run(['cargo', 'test', '-p', 'spitzli-build', '--features', 'local-registry-test,kubernetes-job', '--test', 'worker_memory_overlap', '--no-run', '--message-format=json', '-j', '2'], cwd=ROOT, capture_output=True, text=True, timeout=120)
    if built.returncode:
        raise RuntimeError('worker_memory_fixture_compile_failed')
    binaries = [row['executable'] for row in map(json.loads, built.stdout.splitlines()) if row.get('reason') == 'compiler-artifact' and row.get('target', {}).get('name') == 'worker_memory_overlap' and row.get('executable')]
    assert len(binaries) == 1
    ingress = maximum.Image()
    maximum.MAX_OUTPUT -= 1536
    try:
        publication = maximum.Image()
    finally:
        maximum.MAX_OUTPUT += 1536
    binary = Path(binaries[0])
    measured = case(binary, ingress, publication)
    result = {'verifiedAt': datetime.datetime.now(datetime.timezone.utc).isoformat(), 'profile': 'actual-maximum-native-gateway-collector-and-trusted-registry-publisher-overlap', 'availableMemoryBeforeBuildBytes': available, 'workerCgroupMemoryMaxBytes': BUDGET, 'workerCgroupSwapMaxBytes': 0, 'registryImage': REGISTRY, 'registryMemoryMaxBytes': 1024**3, 'measurement': measured, 'scope': ['Actual gateway HTTP ingress/read/verify_result and maintained ArtifactStore/RegistryPublisher APIs; no synthetic heap filler.', 'A real registry PATCH forwards64KiB then backpressures until actual gateway collection/verification finishes; publisher data remains in the real OCI client push.', 'Gateway exactMAX image512MiB and separate publisher exactMAX retained envelope512MiB (innerOCI512MiB-1536B).', 'No Core/dispatcher full worker loop or MAX_SOURCE concurrent parsing; no production/live-provider/BUILD endpoint changed.', 'A bare512MiB gateway OCI exceeds ArtifactStore512MiB once wrapped; that separate product boundary is not repaired by this fixture.'], 'recommendation': {'sixGiBOverlapPassed': measured.get('actualOverlapVerified', False) and measured.get('publisherAccepted', False) and measured.get('collectorAccepted', False) and measured.get('finalSystemdResult') == 'success', 'productionWorkerChanged': False}}
    result['testBinarySHA256'] = file_hash(binary)
    result['testProfile'] = 'cargo test: unoptimized GNU/glibc host integration binary; not deployed static/release daemon'
    result['publisherMemoryReview'] = ['ArtifactStore.read verifies the maximum envelope and OCI before publication.', 'RegistryPublisher retains envelope bytes, parsed envelope/image maps and clones each layer Vec for maintained oci-client.push_blob.', 'Gateway tmpfs receipt plus read/verify_result buffers coexist with actual publisher data and the active real blob push.']
    result['recommendation'].update({'workerMemoryMaxBytes': BUDGET, 'testedConcurrency': {'gatewayUploadOrCollect': 1, 'publication': 1}, 'reason': '6GiB passed this maximum two-operation overlap with measured headroom; observed peak exceeds4GiB. Keep finite concurrency. This is not full-worker worst-case qualification.', 'nextQualification': 'Repeat with actual dispatcher envelope persistence and MAX_SOURCE parsing before claiming the entire worker maximum; serialize heavy phases if those paths cannot fit the reserved bound.'})
    args.evidence.parent.mkdir(parents=True, exist_ok=True)
    args.evidence.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result, indent=2))
    assert result['recommendation']['sixGiBOverlapPassed'], 'actual_overlap_not_qualified'
    assert measured['cgroupMemoryEvents'].get('oom_kill', 0) == 0


if __name__ == '__main__':
    main()
