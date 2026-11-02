"""Offline network-gate checks; never use a kubeconfig or contact a host."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import unittest

ROOT = Path(__file__).resolve().parents[2]
CLUSTER = ROOT / 'deploy/build-cluster'


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class BuildNetwork(unittest.TestCase):
    def test_no_ip_can_authorize_pod_to_host_api_even_in_established_conntrack(self):
        text = (CLUSTER / 'host.nft').read_text()
        self.assertNotIn('system_pods', text)
        self.assertLess(text.index('iifname "cni0" counter drop'), text.index('ct state established,related accept'))
        self.assertLess(text.index('ip saddr 10.84.0.0/16 counter drop'), text.index('tcp dport 22 accept'))
        self.assertIn('ip daddr 10.83.0.1 udp dport 53 accept', text)
        self.assertIn('ip daddr 10.83.0.1 tcp dport { 53, 3128 } accept', text)
        self.assertNotIn('ip daddr != 10.84.0.0/16', text)

    def test_legacy_timer_has_no_api_authority(self):
        module = load('retired_timer', CLUSTER / 'sync-system-pods.py')
        self.assertEqual(module.rules([{'metadata': {'namespace': 'kube-system'}, 'spec': {'nodeName': 'webdock-build-eu-01'}, 'status': {'podIPs': [{'ip': '10.84.0.70'}]}}]), 'flush set inet webdock_build_host system_pods\n')

    def test_trusted_profile_binds_only_private_or_loopback_and_avoids_port_conflict(self):
        path = CLUSTER / 'trusted-system-network.py'
        self.assertTrue(path.is_file(), 'versioned trusted host-network profile is missing')
        module = load('trusted_network', path)
        patches = module.profile()
        dns = patches['coredns']['spec']['template']['spec']
        metrics = patches['metrics-server']['spec']['template']['spec']
        self.assertTrue(dns['hostNetwork'])
        self.assertEqual(dns['dnsPolicy'], 'Default')
        self.assertTrue(metrics['hostNetwork'])
        self.assertEqual(metrics['dnsPolicy'], 'ClusterFirstWithHostNet')
        for pod in [dns, metrics]:
            self.assertEqual(pod['nodeSelector']['kubernetes.io/hostname'], 'webdock-build-eu-01')
            self.assertNotIn('hostPID', pod)
            for container in pod['containers']:
                self.assertIn('@sha256:', container['image'])
                self.assertFalse(container['securityContext']['allowPrivilegeEscalation'])
        args = metrics['containers'][0]['args']
        self.assertIn('--bind-address=10.83.0.1', args)
        self.assertIn('--secure-port=10251', args)
        self.assertNotIn('--kubelet-insecure-tls', args)
        self.assertEqual(metrics['containers'][0]['ports'][1]['containerPort'], 10251)
        for container in [dns['containers'][0], metrics['containers'][0]]:
            for name in ['readinessProbe', 'livenessProbe']:
                self.assertIn(container[name]['httpGet']['host'], ['127.0.0.1', '10.83.0.1'])
        core = module.COREFILE
        for binding in ['bind 10.83.0.1', 'health 127.0.0.1:8080', 'ready 127.0.0.1:8181', 'prometheus 127.0.0.1:9153']:
            self.assertIn(binding, core)
        self.assertNotIn('import', core)

    def test_dns_profile_does_not_grant_other_host_ports_or_customer_hostnetwork(self):
        path = CLUSTER / 'trusted-system-network.py'
        self.assertTrue(path.is_file())
        module = load('trusted_policy', path)
        for namespace in ['webdock-builds', 'webdock-build-qualification']:
            policy = module.dns_policy(namespace)
            self.assertEqual(policy['metadata']['namespace'], namespace)
            self.assertEqual(policy['spec']['egress'], [{'to': [{'ipBlock': {'cidr': '10.83.0.1/32'}}], 'ports': [{'protocol': 'UDP', 'port': 53}, {'protocol': 'TCP', 'port': 53}]}])
        with self.assertRaises(ValueError): module.dns_policy('default')
        self.assertEqual(module.SKIP_FILES, ('coredns.yaml.skip', 'metrics-server/metrics-server-deployment.yaml.skip'))

    def test_no_live_operation_without_explicit_host_argument(self):
        for path in [CLUSTER / 'trusted-system-network.py', ROOT / 'tools/tests/build-network-isolation.py']:
            self.assertTrue(path.is_file())
            result = subprocess.run([sys.executable, str(path)], capture_output=True, text=True)
            self.assertEqual(result.returncode, 2)
            self.assertIn('--', result.stderr)

    def test_dns_outage_probe_requires_observed_failure_then_recovery(self):
        module = load('dns_outage_probe', ROOT / 'tools/tests/build-network-isolation.py')
        self.assertTrue(hasattr(module, 'dns_outage_script'))
        script = module.dns_outage_script()
        self.assertIn('PASS dns-failure-observed', script)
        self.assertIn('PASS dns-recovery-observed', script)
        self.assertIn('exit 1', script)
        self.assertEqual(subprocess.run(['sh', '-n'], input=script, text=True, capture_output=True).returncode, 0)

    def test_reuse_fixture_refuses_allocated_or_nonpod_addresses(self):
        module = load('reuse_probe', ROOT / 'tools/tests/build-network-isolation.py')
        self.assertTrue(hasattr(module, 'reuse_address'), 'controlled former system-IP proof is missing')
        self.assertEqual(module.reuse_address('10.84.0.70', '10.84.0.0/24', set()), '10.84.0.70')
        for address in ['10.83.0.1', '10.84.0.1', '10.84.0.0', '10.84.0.255', '::ffff:10.84.0.70']:
            with self.assertRaises(ValueError): module.reuse_address(address, '10.84.0.0/24', set())
        with self.assertRaises(ValueError): module.reuse_address('10.84.0.70', '10.84.0.0/24', {'10.84.0.70'})

    def test_temporary_source_ingress_has_literal_two_port_ceiling_and_expiring_owner(self):
        text = (CLUSTER / 'host.nft').read_text()
        self.assertIn('set qualification_ports', text)
        self.assertIn('set qualification_operator4', text)
        self.assertIn('flags timeout', text)
        self.assertIn('ip daddr 167.86.75.144 tcp dport { 19443, 19444 } tcp dport @qualification_ports accept', text)
        self.assertLess(text.index('tcp dport { 19443, 19444 } counter drop'), text.index('ct state established,related accept'))
        path = CLUSTER / 'qualification-ingress.py'
        self.assertTrue(path.is_file())
        module = load('qualification_ingress', path)
        self.assertTrue(hasattr(module, 'controller_peer'))
        peer = module.controller_peer('137.74.1.2 44221 167.86.75.144 22')
        self.assertEqual(peer, '137.74.1.2')
        for invalid in ['', '0.0.0.0 123 167.86.75.144 22', '137.74.1.2/24 123 167.86.75.144 22', '137.74.1.2 123 79.143.177.157 22', '::1 123 167.86.75.144 22']:
            with self.assertRaises(ValueError): module.controller_peer(invalid)
        owner = 'network-proof-20261030'
        lease = module.lease(owner, 1800, 1000, peer)
        self.assertEqual(lease, {'owner': owner, 'expiresAt': 2800, 'ports': [19443, 19444], 'controllerIPv4': peer})
        for bad_owner in ['', '../escape', 'x'*81]:
            with self.assertRaises(ValueError): module.lease(bad_owner, 1800, 1000, peer)
        for ttl in [0, 3601, True]:
            with self.assertRaises(ValueError): module.lease(owner, ttl, 1000, peer)
        with self.assertRaises(ValueError): module.authorize_close({'owner': 'another-run'}, owner)
        close = module.authorize_close(lease, owner)
        self.assertIn('flush set inet webdock_build_host qualification_ports', close)
        self.assertIn('flush set inet webdock_build_host qualification_operator4', close)
        self.assertIn('timeout 1800s', module.open_rules(lease, 1000))
        self.assertNotIn('6443', module.open_rules(lease, 1000))

    def test_actual_dockerfile_probes_cover_aliases_and_positive_tls_control(self):
        path = ROOT / 'tools/tests/build-network-isolation.py'
        self.assertTrue(path.is_file())
        module = load('network_probe', path)
        dockerfile = module.dockerfile()
        self.assertIn("RUN <<'WEBDOCK_NETWORK_PROBE'\n", dockerfile)
        shell = module.probe_script()
        result = subprocess.run(['sh', '-n'], input=shell, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        for target in ['10.83.0.1', '10.85.0.1', '167.86.75.144', '213.136.65.22', '79.143.177.157', '169.58.95.66', '169.254.169.254', '100.100.100.200', '::ffff:10.83.0.1', 'fd00:ec2::254', 'fe80::1', 'kubernetes.default.svc.cluster.local', 'webdock-build-eu-01']:
            self.assertIn(target, dockerfile)
        self.assertNotIn('\r', dockerfile)
        self.assertIn("printf '%b'", dockerfile)
        self.assertIn('-verify_hostname', dockerfile)
        self.assertIn('-verify_return_error', dockerfile)
        self.assertNotIn('curl -k', dockerfile)
        self.assertIn('10.85.0.10 53', dockerfile)


if __name__ == '__main__': unittest.main()
