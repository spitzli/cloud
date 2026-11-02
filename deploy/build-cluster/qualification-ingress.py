#!/usr/bin/env python3
"""Explicit, expiring BUILD-only native qualification data-plane ingress.

The installed host rule independently caps the destination and ports. Namespace
policies, TLS and per-attempt source/upload authority are still required.
"""
import argparse
import fcntl
import importlib.util
import ipaddress
import json
import os
from pathlib import Path
import re
import time

DIRECTORY = Path('/etc/rancher/k3s')
MARKER = DIRECTORY / 'qualification-ingress-owner.json'
PORTS = [19443, 19444]


def controller_peer(connection):
    fields = connection.split()
    if len(fields) != 4 or fields[2] != '167.86.75.144': raise ValueError('current BUILD SSH connection is required')
    address = ipaddress.ip_address(fields[0])
    if address.version != 4 or address.is_unspecified or address.is_multicast or address.is_loopback or address.is_link_local:
        raise ValueError('current SSH controller must be one non-local IPv4 host')
    if not all(port.isdigit() and 1 <= int(port) <= 65535 for port in [fields[1], fields[3]]):
        raise ValueError('invalid SSH connection ports')
    return str(address)


def lease(owner, ttl, now, peer):
    if not isinstance(owner, str) or not re.fullmatch(r'[a-z0-9][a-z0-9-]{0,79}', owner):
        raise ValueError('a bounded lowercase qualification run owner is required')
    if type(ttl) is not int or not 1 <= ttl <= 3600:
        raise ValueError('qualification ingress TTL must be 1..3600 seconds')
    return {'owner': owner, 'expiresAt': now + ttl, 'ports': PORTS[:], 'controllerIPv4': controller_peer(peer + ' 1 167.86.75.144 22')}


def open_rules(value, now):
    remaining = value['expiresAt'] - now
    if lease(value['owner'], remaining, now, value['controllerIPv4']) != value:
        raise ValueError('invalid qualification lease')
    return ('flush set inet webdock_build_host qualification_ports\nflush set inet webdock_build_host qualification_operator4\n'
            + 'add element inet webdock_build_host qualification_ports { '
            + ', '.join(str(port) + ' timeout ' + str(remaining) + 's' for port in PORTS) + ' }\n'
            + 'add element inet webdock_build_host qualification_operator4 { ' + value['controllerIPv4'] + ' timeout ' + str(remaining) + 's }\n')


def authorize_close(value, owner):
    if value['owner'] != owner: raise ValueError('qualification ingress belongs to another run')
    return 'flush set inet webdock_build_host qualification_ports\nflush set inet webdock_build_host qualification_operator4\n'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--open', action='store_true')
    mode.add_argument('--close', action='store_true')
    mode.add_argument('--status', action='store_true')
    parser.add_argument('--on-build-host', action='store_true', required=True)
    parser.add_argument('--owner')
    parser.add_argument('--ttl-seconds', type=int, default=1800)
    args = parser.parse_args()
    if not args.status and not args.owner: parser.error('--owner is required for a qualification mutation')
    spec = importlib.util.spec_from_file_location('trusted_network_ingress', Path(__file__).with_name('trusted-system-network.py'))
    trusted = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(trusted)
    trusted.verify()
    with (DIRECTORY / 'qualification-ingress.lock').open('a') as lock:
        os.fchmod(lock.fileno(), 0o600)
        fcntl.flock(lock, fcntl.LOCK_EX)
        current = json.loads(MARKER.read_text()) if MARKER.exists() else None
        live = {'nftables': []}
        for name in ['qualification_ports', 'qualification_operator4']:
            live['nftables'] += json.loads(trusted.run(['nft', '-j', 'list', 'set', 'inet', 'webdock_build_host', name]).stdout)['nftables']
        active = any(item.get('set', {}).get('elem') for item in live['nftables'])
        now = int(time.time())
        if args.status:
            print(json.dumps({'lease': current, 'kernelSet': live}, indent=2))
            return
        if args.open:
            value = lease(args.owner, args.ttl_seconds, now, controller_peer(os.environ.get('SSH_CONNECTION', '')))
            if active and (current is None or current['owner'] != args.owner):
                raise ValueError('live ingress belongs to another or unknown run')
            # Persist custody first; a crash before nft leaves the gate closed.
            MARKER.write_text(json.dumps(value, indent=2))
            MARKER.chmod(0o600)
            trusted.run(['nft', '-f', '-'], input=open_rules(value, now))
            print(json.dumps(value))
        else:
            if current is None: raise ValueError('no owned qualification ingress marker')
            trusted.run(['nft', '-f', '-'], input=authorize_close(current, args.owner))
            MARKER.unlink()
            print(json.dumps({'owner': args.owner, 'closed': True}))


if __name__ == '__main__': main()
