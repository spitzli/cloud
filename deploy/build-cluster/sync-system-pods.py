#!/usr/bin/env python3
"""Retire the legacy IP timer fail-closed; an address never grants API authority."""
import subprocess


def rules(items):
    # Compatibility with old installed timers: revoke every entry, even on Pod churn.
    return 'flush set inet webdock_build_host system_pods\n'


def main():
    present = subprocess.run(['/usr/sbin/nft', 'list', 'set', 'inet', 'webdock_build_host', 'system_pods'], capture_output=True, timeout=5)
    if present.returncode == 0:
        subprocess.run(['/usr/sbin/nft', '-f', '-'], input=rules([]), text=True, check=True, timeout=5)


if __name__ == '__main__': main()
