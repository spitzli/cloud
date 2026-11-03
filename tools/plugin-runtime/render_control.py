"""Render the trusted private plugin listeners; contains references, never secrets."""
import argparse
import ipaddress
import json
import re


def render(image, namespace, prefix, environment_secret, source_secret, issuer_secret,
           callback_sources, runtime_host, replicas=0):
    for value in [namespace, prefix, environment_secret, source_secret, issuer_secret]:
        if not re.fullmatch(r'[a-z](?:[a-z0-9-]{0,61}[a-z0-9])?', value):
            raise ValueError('Explicit Kubernetes names required')
    if len(prefix) > 45 or len({environment_secret, source_secret, issuer_secret}) != 3:
        raise ValueError('Distinct environment, common and issuer custody required')
    if (not re.fullmatch(r'[a-z0-9.-]+(?::[0-9]+)?/[a-z0-9_./-]+@sha256:[a-f0-9]{64}', image)
            or re.search(r'([a-f0-9])\1{63}$', image)):
        raise ValueError('Real immutable backend image required')
    if replicas not in [0, 1] or not callback_sources:
        raise ValueError('Reviewed singleton activation and callback sources required')
    for value in [*callback_sources, runtime_host]:
        address = ipaddress.ip_address(value)
        if address.version != 4 or not any(address in ipaddress.ip_network(n)
                for n in ['10.0.0.0/8', '172.16.0.0/12', '192.168.0.0/16']):
            raise ValueError('Observed RFC1918 host addresses required')
    security = {'allowPrivilegeEscalation': False, 'readOnlyRootFilesystem': True,
                'capabilities': {'drop': ['ALL']}}
    resources = {'requests': {'cpu': '100m', 'memory': '256Mi'},
                 'limits': {'cpu': '1', 'memory': '768Mi'}}
    stage = ('umask 077; mkdir -p /private/data; chmod 0700 /private/data; '
             'for p in /source/*; do test -f "$p"; '
             'cat -- "$p" > "/private/data/${p##*/}"; '
             'chmod 0600 "/private/data/${p##*/}"; done')
    common_labels = {'app.kubernetes.io/instance': prefix}
    objects = []
    for kind in ['control', 'controller']:
        name = prefix + '-plugin-' + kind
        labels = {**common_labels, 'app.kubernetes.io/component': 'plugin-' + kind}
        metadata = {'name': name, 'namespace': namespace}
        mounts = [{'name': 'private', 'mountPath': '/private-plugins', 'readOnly': True},
                  {'name': 'tmp', 'mountPath': '/tmp'}]
        volumes = [{'name': 'source', 'secret': {'secretName': source_secret, 'defaultMode': 288}},
                   {'name': 'private', 'emptyDir': {'medium': 'Memory', 'sizeLimit': '2Mi'}},
                   {'name': 'tmp', 'emptyDir': {'medium': 'Memory', 'sizeLimit': '64Mi'}}]
        init = [{'name': 'prepare-private', 'image': image,
                 'command': ['/bin/sh', '-ec', stage], 'securityContext': security,
                 'resources': resources, 'volumeMounts': [
                     {'name': 'source', 'mountPath': '/source', 'readOnly': True},
                     {'name': 'private', 'mountPath': '/private'}]}]
        args = ['--config', '/private-plugins/data/plugin-config.json']
        if kind == 'controller':
            mounts.append({'name': 'issuer-private', 'mountPath': '/private-issuer', 'readOnly': True})
            volumes += [{'name': 'issuer-source', 'secret': {'secretName': issuer_secret, 'defaultMode': 288}},
                        {'name': 'issuer-private', 'emptyDir': {'medium': 'Memory', 'sizeLimit': '1Mi'}}]
            init.append({**init[0], 'name': 'prepare-issuer', 'volumeMounts': [
                {'name': 'issuer-source', 'mountPath': '/source', 'readOnly': True},
                {'name': 'issuer-private', 'mountPath': '/private'}]})
            args += ['--installation-config', '/private-issuer/data/worker.json']
        container = {'name': 'plugin-' + kind, 'image': image, 'imagePullPolicy': 'IfNotPresent',
                     'command': ['/usr/local/bin/webdock-plugin-' + kind], 'args': args,
                     'envFrom': [{'secretRef': {'name': environment_secret}}],
                     'securityContext': security, 'resources': resources, 'volumeMounts': mounts}
        if kind == 'control':
            container.update(ports=[{'name': 'mtls', 'containerPort': 3132}],
                             readinessProbe={'tcpSocket': {'port': 'mtls'}, 'periodSeconds': 3})
        pod = {'automountServiceAccountToken': False, 'enableServiceLinks': False,
               'terminationGracePeriodSeconds': 30,
               'securityContext': {'runAsNonRoot': True, 'runAsUser': 65532,
                   'runAsGroup': 65532, 'fsGroup': 65532, 'fsGroupChangePolicy': 'OnRootMismatch',
                   'seccompProfile': {'type': 'RuntimeDefault'}},
               'initContainers': init, 'containers': [container], 'volumes': volumes}
        public_except = ['0.0.0.0/8', '10.0.0.0/8', '100.64.0.0/10', '127.0.0.0/8',
                         '169.254.0.0/16', '172.16.0.0/12', '192.168.0.0/16',
                         '224.0.0.0/4', '240.0.0.0/4']
        dns = {'namespaceSelector': {'matchLabels': {'kubernetes.io/metadata.name': 'kube-system'}},
               'podSelector': {'matchLabels': {'k8s-app': 'kube-dns'}}}
        policy = {'podSelector': {'matchLabels': labels}, 'policyTypes': ['Ingress', 'Egress'],
                  'ingress': [], 'egress': [
                      {'to': [dns], 'ports': [{'protocol': p, 'port': 53} for p in ['UDP', 'TCP']]},
                      {'to': [{'ipBlock': {'cidr': '0.0.0.0/0', 'except': public_except}}],
                       'ports': [{'protocol': 'TCP', 'port': p} for p in [443, 5432]]},
                      {'to': [{'ipBlock': {'cidr': runtime_host + '/32'}}],
                       'ports': [{'protocol': 'TCP', 'port': 443}]}]}
        if kind == 'control':
            policy['ingress'] = [{'from': [{'ipBlock': {'cidr': s + '/32'}} for s in callback_sources],
                                  'ports': [{'protocol': 'TCP', 'port': 3132}]}]
        objects.append({'apiVersion': 'networking.k8s.io/v1', 'kind': 'NetworkPolicy',
                        'metadata': metadata, 'spec': policy})
        if kind == 'control':
            objects.append({'apiVersion': 'v1', 'kind': 'Service', 'metadata': metadata,
                            'spec': {'type': 'ClusterIP', 'selector': labels,
                                     'ports': [{'name': 'mtls', 'port': 3132, 'targetPort': 'mtls'}]}})
        objects.append({'apiVersion': 'apps/v1', 'kind': 'Deployment', 'metadata': metadata,
                        'spec': {'replicas': replicas, 'strategy': {'type': 'Recreate'},
                                 'selector': {'matchLabels': labels},
                                 'template': {'metadata': {'labels': labels}, 'spec': pod}}})
    return {'apiVersion': 'v1', 'kind': 'List', 'items': objects}


def main():
    p = argparse.ArgumentParser(description=__doc__)
    for option in ['image', 'namespace', 'prefix', 'environment-secret', 'source-secret', 'issuer-secret', 'runtime-host']:
        p.add_argument('--' + option, required=True)
    p.add_argument('--callback-source', action='append', required=True)
    p.add_argument('--replicas', type=int, choices=[0, 1], default=0)
    a = vars(p.parse_args())
    a['callback_sources'] = a.pop('callback_source')
    print(json.dumps(render(**a), indent=2))


if __name__ == '__main__':
    main()
