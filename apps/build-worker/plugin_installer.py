#!/usr/bin/env python3
"""Dedicated plugin installer. Source code runs only on a disposable private GitHub Actions runner."""
import argparse
import base64
import fcntl
import hashlib
import io
import ipaddress
import json
from pathlib import Path
import re
import secrets
import subprocess
import tarfile
import tempfile
import time
import urllib.parse

import plugin_source
import plugin_actions
import publisher
import worker

MAX_ZIP = 32 * 1024 * 1024
LABEL = 'webdock.dev/source-import'


def exact_json(raw):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise worker.Rejected('duplicate_json_key')
            result[key] = value
        return result
    return json.loads(raw, object_pairs_hook=pairs)


def fence(control, job):
    if control.request('/lease', {'id': job['id'], 'claimToken': job['claimToken']}) != {'active': True}:
        raise worker.LeaseLost('lease_lost')


def acquire_source(control, job):
    if job['source']['kind'] == 'git':
        source = job['source']
        return plugin_source.fetch_git(source['url'], source.get('ref', 'HEAD'), source.get('credential'))
    size = job['sourceSize']
    if type(size) is not int or not 0 < size <= MAX_ZIP:
        raise worker.Rejected('source_limit')
    output = bytearray()
    while len(output) < size:
        part = control.request('/source', {'id': job['id'], 'claimToken': job['claimToken'], 'offset': len(output)})
        data = base64.b64decode(part['data'], validate=True)
        if not data or len(data) > 65536 or len(output) + len(data) > size:
            raise worker.Rejected('source_limit')
        output.extend(data)
        if part['done'] != (len(output) == size):
            raise worker.Rejected('source_size_mismatch')
    return {'archive': plugin_source.normalize_zip(bytes(output)), 'commit': job['source'].get('commit')}


def build_source(config, control, job, source):
    digest = hashlib.sha256(source['archive']).hexdigest()
    with tarfile.open(fileobj=io.BytesIO(source['archive']), mode='r:') as archive:
        entry = archive.getmember('tabularium.json')
        if entry.size > 262144:
            raise worker.Rejected('manifest_limit')
        manifest = exact_json(archive.extractfile(entry).read())
        if not isinstance(manifest, dict) or not archive.getmember('Dockerfile').isfile():
            raise worker.Rejected('missing_dockerfile')
    if config.get('isolation') != 'artifact-only':
        raise worker.Rejected('artifact_only_required')
    request = {'buildID': job['id'], 'customerID': job['customerID'], 'projectID': job['projectID'], 'generation': 1}
    build_job = {field: job[field] for field in ('id', 'claimToken', 'projectID', 'customerID')}
    artifact, _logs = plugin_actions.build(config, control, build_job, source)
    try:
        fence(control, job)
        with tempfile.TemporaryDirectory(prefix='wd-plugin-publish-') as directory:
            output = Path(directory)
            publisher.load_artifact(config['artifactRoot'], {**request, 'releaseID': job['id'], 'artifact': artifact}, output)
            registry = config['registry']
            published = publisher.publish_oci(output, registry['repository'], registry['authFile'], job['id'])
        manifest['image'] = published['image']
        result = {'phase': 'build', 'manifestRaw': json.dumps(manifest, sort_keys=True, separators=(',', ':')) + '\n', 'sourceSHA256': digest}
        if source.get('commit'):
            result['commit'] = source['commit']
        return result
    finally:
        worker.remove_artifact_keys(config, [artifact['storageKey']])


def console_origin(value):
    if value is None:
        return None
    try:
        if not isinstance(value, str) or len(value) > 2048:
            raise ValueError('Invalid origin')
        parsed = urllib.parse.urlsplit(value)
        if parsed.scheme != 'https' or not parsed.hostname or parsed.username or parsed.password or parsed.path or parsed.query or parsed.fragment:
            raise ValueError('Invalid origin')
        host = parsed.hostname
        if ':' in host:
            if '%' in host:
                raise ValueError('Scoped IPv6 is not a URL origin')
            host = '[' + ipaddress.ip_address(host).compressed + ']'
        elif not re.fullmatch(r'[a-z0-9.-]+', host):
            raise ValueError('Invalid host')
        elif re.fullmatch(r'[0-9]+|0x[a-f0-9]+', host.rstrip('.').rsplit('.', 1)[-1]):
            host = str(ipaddress.IPv4Address(host))
        port = parsed.port
        canonical = 'https://' + host + (':' + str(port) if port is not None and port != 443 else '')
        if value != canonical:
            raise ValueError('Noncanonical origin')
        return value
    except ValueError:
        raise worker.Rejected('invalid_console_origin') from None


def runtime_resources(config, job, token, public_key, certificate, key):
    runtime = config['runtime']
    manifest = exact_json(job['manifestRaw'])
    name = 'pi-' + job['id']
    labels = {LABEL: job['id']}
    def resource(kind, body, api='v1', suffix=''):
        return {'apiVersion': api, 'kind': kind, 'metadata': {'name': name + suffix, 'namespace': runtime['namespace'], 'labels': labels}, **body}
    navigation = bool(manifest.get('navigation'))
    console = console_origin(runtime.get('consoleOrigin'))
    if navigation and (not all(runtime.get(field) for field in ('consoleOrigin', 'uiBaseDomain', 'ingressClass', 'ingressNamespace', 'ingressPodLabels'))
                       or bool(runtime.get('uiTLSSecret')) == bool(runtime.get('uiCertResolver'))):
        raise worker.Rejected('plugin_ui_setup_required')
    identity = {'SPITZLI_PLUGIN_ID': manifest['name'], 'SPITZLI_PLUGIN_VERSION': manifest['version'],
                'SPITZLI_PLUGIN_GENERATION': '1', 'SPITZLI_PLUGIN_HOST_URL': runtime['hostOrigin'],
                'SPITZLI_PLUGIN_RUNTIME_TOKEN': token, 'SPITZLI_PLUGIN_PUBLIC_KEY': public_key,
                'SPITZLI_PLUGIN_CONFIGURATION': json.dumps(job.get('configuration', {})),
                'SPITZLI_PLUGIN_TLS_CERT': '/runtime-tls/tls.crt', 'SPITZLI_PLUGIN_TLS_KEY': '/runtime-tls/tls.key'}
    if console is not None:
        identity['SPITZLI_PLUGIN_CONSOLE_ORIGIN'] = console
    secret = resource('Secret', {'type': 'Opaque', 'stringData': identity}, suffix='-identity')
    tls = resource('Secret', {'type': 'kubernetes.io/tls', 'stringData': {'tls.crt': certificate, 'tls.key': key}}, suffix='-tls')
    security = {'allowPrivilegeEscalation': False, 'readOnlyRootFilesystem': True, 'capabilities': {'drop': ['ALL']}}
    pod = {'automountServiceAccountToken': False,
           'securityContext': {'runAsNonRoot': True, 'runAsUser': 1000, 'runAsGroup': 1000, 'fsGroup': 1000, 'seccompProfile': {'type': 'RuntimeDefault'}},
           'containers': [{'name': 'plugin', 'image': manifest['image'], 'imagePullPolicy': 'IfNotPresent',
                           'envFrom': [{'secretRef': {'name': name + '-identity'}}], 'securityContext': security,
                           'ports': [{'name': 'runtime', 'containerPort': 3443}, {'name': 'ui', 'containerPort': 3444}],
                           'resources': {'requests': {'cpu': '50m', 'memory': '64Mi', 'ephemeral-storage': '16Mi'},
                                         'limits': {'cpu': '500m', 'memory': '256Mi', 'ephemeral-storage': '64Mi'}},
                           'volumeMounts': [{'name': 'tls', 'mountPath': '/runtime-tls', 'readOnly': True}, {'name': 'tmp', 'mountPath': '/tmp'}],
                           'readinessProbe': {'tcpSocket': {'port': 3443}, 'periodSeconds': 5},
                           'livenessProbe': {'tcpSocket': {'port': 3443}, 'periodSeconds': 10, 'failureThreshold': 3},
                           'startupProbe': {'tcpSocket': {'port': 3443}, 'periodSeconds': 5, 'failureThreshold': 60}}],
           'volumes': [{'name': 'tls', 'secret': {'secretName': name + '-tls', 'defaultMode': 0o440}},
                       {'name': 'tmp', 'emptyDir': {'sizeLimit': '32Mi'}}]}
    if runtime.get('imagePullSecret'):
        pod['imagePullSecrets'] = [{'name': runtime['imagePullSecret']}]
    deployment = resource('Deployment', {'spec': {'replicas': 1, 'strategy': {'type': 'Recreate'}, 'selector': {'matchLabels': labels},
                                                  'template': {'metadata': {'labels': labels}, 'spec': pod}}}, 'apps/v1')
    service = resource('Service', {'spec': {'selector': labels, 'ports': [{'name': 'runtime', 'port': 3443, 'targetPort': 3443}, {'name': 'ui', 'port': 3444, 'targetPort': 3444}]}})
    api_peer = {'namespaceSelector': {'matchLabels': {'kubernetes.io/metadata.name': runtime['apiNamespace']}}, 'podSelector': {'matchLabels': runtime['apiPodLabels']}}
    egress = [{'to': [{'ipBlock': {'cidr': str(ipaddress.ip_network(address + ('/32' if ':' not in address else '/128'), strict=True))}}], 'ports': [{'protocol': 'TCP', 'port': 443}]} for address in runtime['apiAddresses']]
    if runtime.get('ingressNamespace') and runtime.get('ingressPodLabels'):
        # Some CNIs enforce the destination after public-API traffic is DNATed to ingress pods.
        egress.append({'to': [{'namespaceSelector': {'matchLabels': {'kubernetes.io/metadata.name': runtime['ingressNamespace']}},
                               'podSelector': {'matchLabels': runtime['ingressPodLabels']}}],
                       'ports': [{'protocol': 'TCP', 'port': 443}, {'protocol': 'TCP', 'port': 8443}]})
    egress.append({'to': [{'namespaceSelector': {'matchLabels': {'kubernetes.io/metadata.name': 'kube-system'}}, 'podSelector': {'matchLabels': {'k8s-app': 'kube-dns'}}}], 'ports': [{'protocol': 'UDP', 'port': 53}, {'protocol': 'TCP', 'port': 53}]})
    ingress = [{'from': [api_peer], 'ports': [{'protocol': 'TCP', 'port': 3443}]}]
    resources = [secret, tls, service, deployment]
    if navigation:
        host = name + '.' + runtime['uiBaseDomain']
        ingress.append({'from': [{'namespaceSelector': {'matchLabels': {'kubernetes.io/metadata.name': runtime['ingressNamespace']}}, 'podSelector': {'matchLabels': runtime['ingressPodLabels']}}], 'ports': [{'protocol': 'TCP', 'port': 3444}]})
        tls = {'hosts': [host]}
        if runtime.get('uiTLSSecret'):
            tls['secretName'] = runtime['uiTLSSecret']
        route = resource('Ingress', {'spec': {'ingressClassName': runtime['ingressClass'], 'tls': [tls],
                         'rules': [{'host': host, 'http': {'paths': [{'path': '/', 'pathType': 'Prefix', 'backend': {'service': {'name': name, 'port': {'number': 3444}}}}]}}]}}, 'networking.k8s.io/v1')
        if runtime.get('uiCertResolver'):
            route['metadata']['annotations'] = {'traefik.ingress.kubernetes.io/router.entrypoints': 'websecure',
                                               'traefik.ingress.kubernetes.io/router.tls': 'true',
                                               'traefik.ingress.kubernetes.io/router.tls.certresolver': runtime['uiCertResolver']}
        resources.append(route)
    # Establish isolation before the deployment is created.
    resources.insert(0, resource('NetworkPolicy', {'spec': {'podSelector': {'matchLabels': labels}, 'policyTypes': ['Ingress', 'Egress'], 'ingress': ingress, 'egress': egress}}, 'networking.k8s.io/v1'))
    return resources


class Kubernetes:
    def __init__(self, config):
        self.runtime = config['runtime']
        self.kubeconfig = str(worker.private_file(self.runtime['kubeconfigFile']))

    def call(self, args, value=None, timeout=30):
        with tempfile.TemporaryDirectory(prefix='wd-plugin-kube-') as directory:
            command = ['/usr/bin/kubectl', '--kubeconfig', self.kubeconfig, *args]
            if value is not None:
                source = Path(directory) / 'resource.json'
                source.write_text(json.dumps(value)); source.chmod(0o600)
                command += ['-f', str(source)]
            return worker.run_bounded(command, env={'PATH': '/usr/bin:/bin', 'HOME': directory}, timeout=timeout, max_bytes=2 * 1024 * 1024)

    def get(self, kind, name, namespace=None):
        return exact_json(self.call(['get', kind, name, '-n', namespace or self.runtime['namespace'], '-o', 'json']))


def generate_runtime_tls(dns):
    with tempfile.TemporaryDirectory(prefix='wd-plugin-tls-') as directory:
        root = Path(directory)
        # RSA transport certificates interoperate with Bun's Node-compatible HTTPS client.
        worker.run_bounded(['/usr/bin/openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-sha256', '-noenc', '-days', '365', '-subj', '/CN=' + dns,
                            '-addext', 'subjectAltName=DNS:' + dns, '-addext', 'basicConstraints=critical,CA:FALSE',
                            '-addext', 'extendedKeyUsage=serverAuth', '-keyout', str(root / 'tls.key'), '-out', str(root / 'tls.crt')], timeout=10)
        return (root / 'tls.crt').read_text(), (root / 'tls.key').read_text()


def deploy_runtime(config, control, job):
    runtime = config['runtime']
    manifest = exact_json(job['manifestRaw'])
    if not re.fullmatch(r'[a-z][a-z0-9-]{0,63}', manifest['name']) or not re.fullmatch(r'[a-z0-9.-]+(?::[0-9]+)?/[a-z0-9/_-]+@sha256:[a-f0-9]{64}', manifest['image']):
        raise worker.Rejected('invalid_manifest')
    kube = Kubernetes(config)
    namespace = kube.get('namespace', runtime['namespace'])
    if namespace.get('metadata', {}).get('labels', {}).get('pod-security.kubernetes.io/enforce') != 'restricted':
        raise worker.Rejected('restricted_namespace_required')
    secret_ref = runtime['configSecret']
    secret = kube.get('secret', secret_ref['name'], secret_ref['namespace'])
    settings = exact_json(base64.b64decode(secret['data'][secret_ref['key']], validate=True))
    if settings['hostOrigin'] != runtime['hostOrigin'] or manifest['name'] in settings['runtimes']:
        raise worker.Rejected('runtime_already_configured')
    public = subprocess.run(['/usr/bin/openssl', 'pkey', '-pubout'], input=settings['privateKey'].encode(), capture_output=True, timeout=10, check=True).stdout.decode()
    name = 'pi-' + job['id']
    token = secrets.token_urlsafe(48)
    dns = name + '.' + runtime['namespace'] + '.svc.cluster.local'
    cert, key = generate_runtime_tls(dns)
    resources = runtime_resources(config, job, token, public, cert, key)
    fence(control, job)
    # create, never apply: an existing workload or ambiguous retry cannot be overwritten.
    for resource in resources:
        fence(control, job)
        kube.call(['create'], resource)
    kube.call(['rollout', 'status', 'deployment/' + name, '-n', runtime['namespace'], '--timeout=300s'], timeout=310)
    pods = exact_json(kube.call(['get', 'pods', '-n', runtime['namespace'], '-l', LABEL + '=' + job['id'], '-o', 'json']))['items']
    if len(pods) != 1:
        raise worker.Rejected('runtime_identity_mismatch')
    pod = pods[0]
    statuses = pod.get('status', {}).get('containerStatuses', [])
    if len(statuses) != 1 or not statuses[0].get('ready') or pod['spec']['containers'][0]['image'] != manifest['image'] or not statuses[0].get('imageID', '').endswith(manifest['image'].split('@')[1]):
        raise worker.Rejected('runtime_image_mismatch')
    address = str(ipaddress.ip_address(kube.get('service', name)['spec']['clusterIP']))
    binding = {'generation': 1, 'namespace': runtime['namespace'], 'service': name, 'addresses': [address], 'credential': token, 'ca': cert, 'projects': {}}
    if manifest.get('navigation'):
        binding['uiOrigin'] = 'https://' + name + '.' + runtime['uiBaseDomain']
    fence(control, job)
    # The resourceVersion read above is a CAS fence for all existing runtime bindings/key material.
    settings['runtimes'][manifest['name']] = binding
    secret['data'][secret_ref['key']] = base64.b64encode(json.dumps(settings).encode()).decode()
    kube.call(['replace'], secret)
    return {'phase': 'deploy', 'binding': binding, 'clusterID': runtime['clusterID']}


def run_once(config, control):
    job = control.request('/claim', {})
    if not job:
        return False
    try:
        if not all(worker.ID.fullmatch(job[field]) for field in ('id', 'projectID', 'customerID')) or job['mode'] != 'build':
            raise worker.Rejected('invalid_job')
        if job['phase'] == 'build':
            result = build_source(config, control, job, acquire_source(control, job))
        elif job['phase'] == 'deploy':
            result = deploy_runtime(config, control, job)
        else:
            raise worker.Rejected('invalid_phase')
    except worker.LeaseLost:
        return True
    except Exception:
        result = {'phase': 'failed', 'code': 'BUILD_FAILED' if job.get('phase') == 'build' else 'INSTALLATION_FAILED'}
    # An uncertain completion is not retried as a new build/deployment. The expired claim
    # stays visible for operator reconciliation instead of overwriting a live runtime.
    control.request('/complete', {'id': job['id'], 'claimToken': job['claimToken'], 'result': result})
    return True


def validate_config(config):
    if config.get('isolation') != 'artifact-only':
        raise worker.Rejected('artifact_only_required')
    worker.validate_config(config)
    plugin_actions.validate_config(config['github'])
    origin = urllib.parse.urlsplit(config['controlURL'])
    if origin.username or origin.password or origin.query or origin.fragment or origin.path != '/v1/plugin-installer':
        raise worker.Rejected('invalid_control_url')
    registry = config['registry']
    if not re.fullmatch(r'[a-z0-9.-]+(?::[0-9]+)?/[a-z0-9/_-]+', registry['repository']) or '..' in registry['repository']:
        raise worker.Rejected('invalid_registry')
    worker.private_file(registry['authFile'])
    runtime = config['runtime']
    console_origin(runtime.get('consoleOrigin'))
    if bool(runtime.get('ingressNamespace')) != bool(runtime.get('ingressPodLabels')):
        raise worker.Rejected('invalid_ingress_selector')
    for name in ['namespace', 'apiNamespace'] + (['ingressNamespace'] if runtime.get('ingressNamespace') else []):
        if not re.fullmatch(r'[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?', runtime[name]):
            raise worker.Rejected('invalid_namespace')
    if not runtime['apiAddresses'] or len(runtime['apiAddresses']) > 8:
        raise worker.Rejected('invalid_api_addresses')
    for address in runtime['apiAddresses']:
        ipaddress.ip_address(address)
    host = urllib.parse.urlsplit(runtime['hostOrigin'])
    if host.scheme != 'https' or not host.hostname or host.username or host.password or host.path not in ('', '/') or host.query or host.fragment:
        raise worker.Rejected('invalid_host_origin')
    for labels in [runtime['apiPodLabels']] + ([runtime['ingressPodLabels']] if runtime.get('ingressNamespace') else []):
        if not isinstance(labels, dict) or not labels or any(not isinstance(value, str) or not value for value in labels.values()):
            raise worker.Rejected('invalid_pod_selector')
    if any(runtime.get(field) for field in ('uiBaseDomain', 'uiTLSSecret', 'uiCertResolver')):
        if (not all(runtime.get(field) for field in ('consoleOrigin', 'uiBaseDomain', 'ingressClass', 'ingressNamespace', 'ingressPodLabels'))
                or bool(runtime.get('uiTLSSecret')) == bool(runtime.get('uiCertResolver'))):
            raise worker.Rejected('plugin_ui_setup_required')
    if runtime.get('uiBaseDomain') and (not re.fullmatch(r'[a-z0-9.-]+', runtime['uiBaseDomain']) or runtime['uiBaseDomain'] == host.hostname):
        raise worker.Rejected('invalid_ui_domain')
    if runtime.get('uiCertResolver') and (not isinstance(runtime['uiCertResolver'], str)
                                       or not re.fullmatch(r'[A-Za-z0-9_-]{1,63}', runtime['uiCertResolver'])):
        raise worker.Rejected('invalid_ui_resolver')
    worker.private_file(runtime['kubeconfigFile'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', required=True)
    parser.add_argument('--once', action='store_true')
    args = parser.parse_args()
    config = exact_json(worker.private_file(args.config).read_text())
    validate_config(config)
    control = worker.Control(config)
    with (Path(config['artifactRoot']) / '.plugin-installer.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        while True:
            try:
                worked = run_once(config, control)
            except Exception:
                print(json.dumps({'event': 'plugin_installer_unavailable'}), flush=True)
                worked = False
            if args.once:
                return
            if not worked:
                time.sleep(5)


if __name__ == '__main__':
    main()
