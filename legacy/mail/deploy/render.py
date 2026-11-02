#!/usr/bin/env python3
"""Render the shared Mail gateway. No secrets or persistent storage are created."""
import argparse
import json
import re

NAMESPACE = 'webdock-mail-gateway'


def render(api_image, gateway_image):
    for image in (api_image, gateway_image):
        if not re.fullmatch(r'[a-zA-Z0-9./_-]+@sha256:[0-9a-f]{64}', image):
            raise ValueError('Use an immutable image repository digest')
    resources = []
    labels = {'security.webdock.dev/managed-by': 'webdock'}

    def add(kind, name, spec=None):
        version = 'apps/v1' if kind == 'Deployment' else 'networking.k8s.io/v1' if kind in ('NetworkPolicy', 'Ingress') else 'v1'
        item = {'apiVersion': version, 'kind': kind, 'metadata': {'name': name, 'namespace': NAMESPACE, 'labels': dict(labels)}}
        if kind == 'Namespace':
            del item['metadata']['namespace']
        if spec is not None:
            item['spec'] = spec
        resources.append(item)
        return item

    namespace = add('Namespace', NAMESPACE)
    namespace['metadata']['labels'].update({'pod-security.kubernetes.io/enforce': 'restricted', 'pod-security.kubernetes.io/audit': 'restricted'})
    add('NetworkPolicy', 'default-deny', {'podSelector': {}, 'policyTypes': ['Ingress', 'Egress']})
    system = {'namespaceSelector': {'matchLabels': {'kubernetes.io/metadata.name': 'kube-system'}}}
    dns = {**system, 'podSelector': {'matchLabels': {'k8s-app': 'kube-dns'}}}
    ingress = {**system, 'podSelector': {'matchLabels': {'app.kubernetes.io/name': 'traefik'}}}
    mail = {'namespaceSelector': {'matchLabels': labels}, 'podSelector': {'matchExpressions': [{'key': 'webdock.dev/mail-customer-id', 'operator': 'Exists'}]}}
    public = {'ipBlock': {'cidr': '0.0.0.0/0', 'except': ['0.0.0.0/8', '10.0.0.0/8', '100.64.0.0/10', '127.0.0.0/8', '169.254.0.0/16', '172.16.0.0/12', '192.168.0.0/16', '224.0.0.0/4', '240.0.0.0/4']}}
    ports = lambda *values: [{'protocol': 'TCP', 'port': value} for value in values]
    add('NetworkPolicy', 'mail-network', {
        'podSelector': {'matchLabels': {'app': 'mail-gateway'}}, 'policyTypes': ['Ingress', 'Egress'],
        'ingress': [{'from': [ingress], 'ports': ports(3138, 3139)}, {'from': [mail], 'ports': ports(3139)}],
        'egress': [{'to': [dns], 'ports': [{'protocol': protocol, 'port': 53} for protocol in ('TCP', 'UDP')]},
                   {'to': [public], 'ports': ports(443)}, {'to': [mail], 'ports': ports(8080, 2465)}],
    })
    security = {'allowPrivilegeEscalation': False, 'readOnlyRootFilesystem': True, 'capabilities': {'drop': ['ALL']}}

    def container(name, image, port, memory, values, secrets):
        return {'name': name, 'image': image, 'imagePullPolicy': 'Never', 'securityContext': security,
                'resources': {'requests': {'cpu': '50m', 'memory': '64Mi', 'ephemeral-storage': '16Mi'},
                              'limits': {'cpu': '1000m', 'memory': memory, 'ephemeral-storage': '64Mi'}},
                'env': [{'name': key, 'value': value} for key, value in values.items()] + [
                    {'name': key, 'valueFrom': {'secretKeyRef': {'name': 'mail-control', 'key': key}}} for key in secrets],
                'ports': [{'containerPort': port}],
                **{probe: {'httpGet': {'path': '/healthz', 'port': port}, 'periodSeconds': 10,
                           'failureThreshold': 12 if probe == 'startupProbe' else 3}
                   for probe in ('startupProbe', 'readinessProbe', 'livenessProbe')}}

    common = {'WEBDOCK_AUTH_ORIGIN': 'https://auth.webdock.dev'}
    api = container('api', api_image, 3139, '512Mi', {**common, 'WEBDOCK_NATIVE_MAIL_SENDING_ENABLED': 'true', 'MAIL_PAYLOAD_DIRECTORY': '/data/payloads'},
                    ['MAIL_SEND_WORKER_SECRET', 'MAIL_GATEWAY_SECRET', 'MAIL_ENCRYPTION_KEY'])
    api['volumeMounts'] = [{'name': 'payloads', 'mountPath': '/data'}]
    gateway = container('gateway', gateway_image, 3138, '256Mi', {**common, 'MAIL_BROWSER_ORIGINS': 'https://studio.webdock.dev', 'MAIL_SEND_INTERNAL_ORIGIN': 'http://127.0.0.1:3139'}, ['MAIL_GATEWAY_SECRET'])
    add('Deployment', 'mail-gateway', {
        'replicas': 1, 'strategy': {'type': 'Recreate'}, 'selector': {'matchLabels': {'app': 'mail-gateway'}},
        'template': {'metadata': {'labels': {**labels, 'app': 'mail-gateway'}}, 'spec': {
            'automountServiceAccountToken': False, 'terminationGracePeriodSeconds': 45,
            'securityContext': {'runAsNonRoot': True, 'runAsUser': 65532, 'runAsGroup': 65532, 'fsGroup': 65532,
                                'fsGroupChangePolicy': 'OnRootMismatch', 'seccompProfile': {'type': 'RuntimeDefault'}},
            'containers': [api, gateway], 'volumes': [{'name': 'payloads', 'persistentVolumeClaim': {'claimName': 'mail-payloads'}}],
        }},
    })
    add('Service', 'mail-gateway', {'selector': {'app': 'mail-gateway'}, 'ports': [
        {'name': 'webmail', 'port': 3138, 'targetPort': 3138}, {'name': 'api', 'port': 3139, 'targetPort': 3139}]})
    rules = []
    for host, port, paths in [('api.webdock.dev', 3139, [('/v1/emails', 'Prefix')]),
                              ('mail.webdock.dev', 3138, [('/session', 'Exact'), ('/s/', 'Prefix')])]:
        rules.append({'host': host, 'http': {'paths': [{'path': path, 'pathType': match,
            'backend': {'service': {'name': 'mail-gateway', 'port': {'number': port}}}} for path, match in paths]}})
    route = add('Ingress', 'mail-gateway', {'ingressClassName': 'traefik', 'rules': rules, 'tls': [{'hosts': [r['host'] for r in rules]}]})
    route['metadata']['annotations'] = {'traefik.ingress.kubernetes.io/router.entrypoints': 'websecure',
        'traefik.ingress.kubernetes.io/router.tls': 'true', 'traefik.ingress.kubernetes.io/router.tls.certresolver': 'webdock'}
    return resources


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--api-image', required=True)
    parser.add_argument('--gateway-image', required=True)
    args = parser.parse_args()
    print(json.dumps({'apiVersion': 'v1', 'kind': 'List', 'items': render(args.api_image, args.gateway_image)}))
