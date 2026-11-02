#!/usr/bin/env python3
"""Render one operator-approved HTTPS staging address; never apply or resolve it."""
import argparse
import ipaddress
import json


def render(address, port=443):
    ip = ipaddress.ip_address(address)
    if ip.is_unspecified or ip.is_multicast or ip.is_loopback or ip.is_link_local or ip == ipaddress.ip_address('255.255.255.255'):
        raise ValueError('A concrete reachable staging address is required')
    if not isinstance(port, int) or isinstance(port, bool) or not 1 <= port <= 65535:
        raise ValueError('A concrete HTTPS endpoint port is required')
    return {
        'apiVersion': 'networking.k8s.io/v1', 'kind': 'NetworkPolicy',
        'metadata': {'name': 'webdock-staging', 'namespace': 'webdock-build-qualification'},
        'spec': {
            'podSelector': {'matchLabels': {'webdock.dev/component': 'build-job'}},
            'policyTypes': ['Egress'],
            'egress': [{'to': [{'ipBlock': {'cidr': f'{ip}/{ip.max_prefixlen}'}}],
                        'ports': [{'protocol': 'TCP', 'port': port}]}],
        },
    }


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--address', required=True, help='Operator-verified literal endpoint IP; no CIDR, hostname or default')
    parser.add_argument('--port', type=int, default=443)
    args = parser.parse_args()
    try:
        print(json.dumps(render(args.address, args.port), indent=2))
    except ValueError as error:
        parser.error(str(error))
