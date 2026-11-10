"""Fixed Stalwart CE profile for operator-authorized Webdock Mail operations."""
import base64
from datetime import datetime, timezone
import re
from executor import manifests

def validate_mail_packet(packet):
    try:
        required = {'version', 'customerID', 'clusterID', 'operationID', 'revision', 'generation', 'leaseToken', 'leaseUntil', 'action', 'hostname', 'image', 'resources', 'credentials'}
        if set(packet) != required or type(packet['version']) is not int or packet['version'] != 1:
            raise ValueError()
        for key in ['customerID', 'clusterID', 'operationID']:
            if not isinstance(packet[key], str) or not re.fullmatch(r'[1-9][0-9]{0,18}', packet[key]) or int(packet[key]) > 9223372036854775807:
                raise ValueError()
        if packet['action'] not in ['ensure', 'suspend']:
            raise ValueError()
        for key in ['revision', 'generation']:
            if type(packet[key]) is not int or not 1 <= packet[key] <= 2147483647:
                raise ValueError()
        if not re.fullmatch(r'[A-Za-z0-9_-]{32,64}', packet['leaseToken']):
            raise ValueError()
        if datetime.fromisoformat(packet['leaseUntil'].replace('Z', '+00:00')) <= datetime.now(timezone.utc):
            raise ValueError()
        if len(packet['hostname']) > 253 or not re.fullmatch(r'mail-' + packet['customerID'] + r'\.(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}', packet['hostname']):
            raise ValueError()
        if not re.fullmatch(r'[A-Za-z0-9.:-]+/[A-Za-z0-9_./-]+@sha256:[a-f0-9]{64}', packet['image']):
            raise ValueError()
        limits = {'cpuMillicores': (10, 64000), 'memoryBytes': (16777216, 1099511627776), 'ephemeralBytes': (1048576, 1099511627776), 'volumeBytes': (67108864, 1099511627776)}
        if set(packet['resources']) != set(limits):
            raise ValueError()
        for key, (low, high) in limits.items():
            if type(packet['resources'][key]) is not int or not low <= packet['resources'][key] <= high:
                raise ValueError()
        credentials = packet['credentials']
        if not isinstance(credentials, dict) or set(credentials) - {'bootstrapPassword', 'username', 'password'}:
            raise ValueError()
        for key, value in credentials.items():
            if not isinstance(value, str) or not 1 <= len(value) <= 1024 or any(ord(char) < 32 for char in value):
                raise ValueError()
        if packet['action'] == 'ensure' and not (credentials.get('bootstrapPassword') or (credentials.get('username') and credentials.get('password'))):
            raise ValueError()
    except (KeyError, TypeError, AttributeError, OverflowError):
        raise ValueError('Invalid Mail operation packet') from None


def application_packet(packet):
    validate_mail_packet(packet)
    resource = packet['resources']
    # The storage helper uses numeric allocation-owner keys. Mail's immutable
    # customer ID owns this separate namespace and is never a caller-supplied project.
    return {
        'appID': packet['customerID'], 'projectID': packet['customerID'], 'clusterID': packet['clusterID'],
        'namespace': 'wd-mail-' + packet['customerID'], 'operationID': packet['operationID'],
        'revision': packet['revision'], 'generation': packet['generation'], 'leaseUntil': packet['leaseUntil'],
        'action': 'apply', 'expectedUID': None, 'storageVersion': 1,
        'quota': {'apps': 1, **resource},
        'spec': {'template': 'custom', 'image': packet['image'], 'args': ['--config', '/data/config.json'],
                 'port': 8080, 'healthPath': '/healthz/live', 'replicas': 1 if packet['action'] == 'ensure' else 0, **resource},
    }


def mail_manifests(packet):
    deployment, service = manifests(application_packet(packet))
    ports = [('http', 8080), ('smtp', 2525), ('submissions', 2465), ('imaps', 1993)]
    container = deployment['spec']['template']['spec']['containers'][0]
    container['ports'] = [{'name': name, 'containerPort': port} for name, port in ports]
    container['env'] = [{'name': 'STALWART_PUBLIC_URL', 'value': 'https://' + packet['hostname']}]
    if packet['action'] == 'ensure' and packet['credentials'].get('bootstrapPassword'):
        container['env'].append({'name': 'STALWART_RECOVERY_MODE', 'value': '1'})
        container['env'].append({'name': 'STALWART_RECOVERY_ADMIN', 'valueFrom': {'secretKeyRef': {'name': 'mail-bootstrap-' + packet['operationID'], 'key': 'administrator'}}})
    service['spec']['ports'] = [{'name': name, 'port': port, 'targetPort': port} for name, port in ports]
    for metadata in [deployment['metadata'], service['metadata'], deployment['spec']['template']['metadata']]:
        metadata['labels'].pop('security.webdock.dev/ingress', None)
        metadata['labels']['webdock.dev/mail-customer-id'] = packet['customerID']
    return deployment, service


def bootstrap_secret(packet):
    validate_mail_packet(packet)
    password = packet['credentials'].get('bootstrapPassword')
    if not password:
        return None
    deployment, _ = mail_manifests(packet)
    return {'apiVersion': 'v1', 'kind': 'Secret', 'metadata': {
        'name': 'mail-bootstrap-' + packet['operationID'], 'namespace': deployment['metadata']['namespace'],
        'labels': deployment['metadata']['labels'],
    }, 'immutable': True, 'type': 'Opaque', 'data': {
        'administrator': base64.b64encode(('admin:' + password).encode()).decode(),
    }}


def mail_gateway_policy(packet):
    deployment, _ = mail_manifests(packet)
    return {'apiVersion': 'networking.k8s.io/v1', 'kind': 'NetworkPolicy',
            'metadata': {**deployment['metadata'], 'name': 'spitzli-mail-gateway'},
            'spec': {'podSelector': {'matchLabels': {'webdock.dev/mail-customer-id': packet['customerID']}},
                     'policyTypes': ['Ingress'], 'ingress': [{'from': [{
                         'namespaceSelector': {'matchLabels': {'kubernetes.io/metadata.name': 'spitzli-mail-gateway'}},
                         'podSelector': {'matchLabels': {'app': 'mail-gateway'}}}],
                         'ports': [{'protocol': 'TCP', 'port': 8080}]}]}}


def mail_isolation_policy(customer_id, cluster_id):
    from executor import labels
    return {'apiVersion':'networking.k8s.io/v1','kind':'NetworkPolicy',
            'metadata':{'name':'webdock-internal','namespace':'wd-mail-'+customer_id,
                        'labels':labels({'projectID':customer_id,'clusterID':cluster_id},False)},
            'spec':{'podSelector':{},'policyTypes':['Ingress','Egress'],'ingress':[],'egress':[]}}


def execute_mail(packet, checkpoint):
    """Provision one fixed profile; checkpoint credentials before removing recovery access."""
    import copy
    import fcntl
    import os
    import time
    from executor import (lease, get, check_owner, apply, prepare_namespace, prepare_storage,
                          guarded_delete, pods, kubectl, ExecutionError)
    from mail_client import MailClient
    import storage_client

    validate_mail_packet(packet)
    packet = copy.deepcopy(packet)
    p = application_packet(packet)
    name, namespace = 'app-' + packet['customerID'], p['namespace']
    last_checkpoint = 0

    def renew(credentials=None):
        nonlocal last_checkpoint
        value = checkpoint(credentials)
        packet['leaseUntil'] = p['leaseUntil'] = value['leaseUntil']
        lease(p)
        last_checkpoint = time.monotonic()

    def wait(read):
        deadline = time.monotonic() + 70
        while True:
            lease(p)
            if time.monotonic() - last_checkpoint > 20:
                renew()
            value = read()
            if value:
                return value
            if time.monotonic() > deadline:
                raise ExecutionError('rollout_timeout')
            time.sleep(1)

    def ready():
        obj = get('deployment', name, namespace)
        if not obj:
            return False
        check_owner(obj, p, True)
        items = pods(p)
        for pod in items:
            check_owner(pod, p, True)
        status, desired = obj.get('status', {}), p['spec']['replicas']
        return (status.get('observedGeneration', 0) >= obj['metadata']['generation'] and
                status.get('updatedReplicas', 0) == desired and status.get('availableReplicas', 0) == desired and
                len(items) == desired and all(not pod['metadata'].get('deletionTimestamp') and
                any(c['type'] == 'Ready' and c['status'] == 'True' for c in pod.get('status', {}).get('conditions', [])) for pod in items))

    def client(username, password):
        service = get('service', name, namespace)
        check_owner(service, p, True)
        return MailClient('http://' + service['spec']['clusterIP'] + ':8080', username, password)

    with open(os.environ.get('WEBDOCK_EXECUTOR_LOCK', '/run/lock/webdock-managed-executor.lock'), 'w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        renew()
        existing = get('namespace', namespace)
        if existing:
            check_owner(existing, p)
        current = get('deployment', name, namespace) if existing else None
        if current:
            check_owner(current, p, True)
        if packet['action'] == 'suspend':
            if current:
                # Keep server-side-apply ownership of replicas across suspend/resume.
                deployment, _ = mail_manifests(packet)
                apply(deployment, p)
                wait(ready)
            elif existing and pods(p):
                raise ExecutionError('ownership_conflict')
            return {'namespace': namespace, 'revision': packet['revision'], 'running': False,
                    'edition': 'community', 'recoveryDisabled': True}
        if packet['credentials'].get('username'):
            # Known instances must never silently receive an empty replacement disk.
            storage_client.retained(p)
        prepare_namespace(p, namespace)
        apply(mail_isolation_policy(packet['customerID'],packet['clusterID']),p)
        apply(mail_gateway_policy(packet), p)
        prepare_storage(p, initial_allocation=not bool(packet['credentials'].get('username')))
        renew()
        deployment, service = mail_manifests(packet)
        secret = bootstrap_secret(packet)
        if secret:
            apply(secret, p)
        for obj in [service, deployment]:
            apply(obj, p)
        wait(ready)
        credentials = packet['credentials']
        if not credentials.get('username'):
            admin = client('admin', credentials['bootstrapPassword'])
            permanent = admin.bootstrap(packet['hostname'])
            renew(permanent)
            credentials.update(permanent)
            # Bootstrap writes the store config. Restart in recovery mode to configure listeners safely.
            deployment['spec']['template']['metadata']['annotations']['webdock.dev/mail-phase'] = 'configure'
            apply(deployment, p)
            wait(ready)
        if credentials.get('bootstrapPassword'):
            admin = client('admin', credentials['bootstrapPassword'])
            admin.configure_listeners()
            credentials.pop('bootstrapPassword')
            final, _ = mail_manifests(packet)
            apply(final, p)
            wait(ready)
        permanent = client(credentials['username'], credentials['password'])
        permanent.verify()
        certificate = permanent.submission_certificate()
        if secret:
            observed = get('secret', secret['metadata']['name'], namespace)
            if observed:
                guarded_delete('secrets', observed, p)
        return {'namespace': namespace, 'revision': packet['revision'], 'running': True,
                'edition': 'community', 'recoveryDisabled': True,
                'submissionCertificateSha256': certificate}


def mail_capability(nodes):
    """Enable only an operator-configured, locally cached image on the current single-node backend."""
    import json
    import os
    from pathlib import Path
    import subprocess
    from storage import private, StorageError
    try:
        path = private(Path(os.environ.get('WEBDOCK_MAIL_CONFIG', '/etc/webdock-mail.json')))
        config = json.loads(path.read_text())
        if set(config) != {'image', 'node'} or nodes != [config['node']] or not re.fullmatch(r'[A-Za-z0-9.:-]+/[A-Za-z0-9_./-]+@sha256:[a-f0-9]{64}', config['image']):
            return None
        result = subprocess.run(['k3s', 'crictl', 'inspecti', config['image']], capture_output=True, text=True, check=True, timeout=15)
        if len(result.stdout) > 1048576 or config['image'] not in json.loads(result.stdout).get('status', {}).get('repoDigests', []):
            return None
        return {'version': 1, 'image': config['image'], 'configurationVersion': 3}
    except (OSError, ValueError, KeyError, TypeError, StorageError, subprocess.SubprocessError):
        return None
