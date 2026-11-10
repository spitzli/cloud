#!/usr/bin/env python3
"""Offline Dockerfile rendering or explicit owned gVisor network diagnostics.

The rendered Dockerfile must also run through the real native build handoff.
The live diagnostics exercise gVisor Pod networking, proxy TLS/rebinding and system
Pod churn; they do not substitute for that actual BuildKit RUN execution.
"""
import argparse
import importlib.util
import ipaddress
import json
from pathlib import Path
import shlex
import subprocess
import time

ROOT = Path(__file__).resolve().parents[2]
CLUSTER = ROOT / 'deploy/build-cluster'
NAMESPACE = 'webdock-network-diagnostics'
IMAGE = 'docker.io/moby/buildkit:v0.34.1-rootless@sha256:f68752a5a94acfcdad2d07867b7075601a421638e17fc5f28efc1c814d0c4f89'
TARGETS = [(host, port) for host in ['10.83.0.1', '10.85.0.1', '167.86.75.144', '213.136.65.22', '79.143.177.157', '169.58.95.66', 'kubernetes.default.svc.cluster.local', 'webdock-build-eu-01'] for port in [443, 6443, 10250, 10251, 5432]] + [(host, 80) for host in ['169.254.169.254', '169.254.170.2', '100.100.100.200', '10.43.0.1', '10.42.0.1', '172.17.0.1', '192.168.0.1']] + [(host, 6443) for host in ['::1', '::ffff:10.83.0.1', '::ffff:10.85.0.1', '::ffff:169.254.169.254', 'fd00:ec2::254', 'fe80::1', '64:ff9b::a53:1']]


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def denied_script(targets=TARGETS):
    script = ''
    for index, (host, port) in enumerate(targets):
        script += f"if timeout 2 nc -z -w 1 {shlex.quote(host)} {port} >/tmp/network.err 2>&1; then echo 'FAIL forbidden-{index} {host}:{port}'; exit 1; fi; echo 'PASS forbidden-{index} {host}:{port}'\n"
    return script


def probe_script():
    proxy = load('proxy_probe', CLUSTER / 'package-proxy/diagnose.py')
    wire = lambda command: command.replace("printf '%s'", "printf '%b'", 1).replace('\r\n', r'\r\n')
    return ('set -eu\nnslookup kubernetes.default.svc.cluster.local\nnslookup registry.npmjs.org\nnc -z -w 3 10.85.0.10 53\n'
            + wire(proxy.positive('registry.npmjs.org', '/is-number', 200))
            + wire(proxy.positive('ghcr.io', '/v2/', 401, 'GET'))
            + wire(proxy.positive('deb.debian.org', '/debian/dists/trixie/InRelease', 200))
            + denied_script()
            + "if timeout 2 nc -z -w 1 1.1.1.1 443; then echo 'FAIL direct-public'; exit 1; fi\n"
            + wire(proxy.denied('mapped-api', '[::ffff:10.83.0.1]:443'))
            + wire(proxy.denied('service-api', '10.85.0.1:443'))
            + wire(proxy.denied('ipv6-link-local', '[fe80::1]:443'))
            + "echo 'PASS network boundary and positive DNS/package/TLS controls'\n")


def dockerfile():
    # Heredoc preserves command and quoted HTTP-request newlines within one RUN.
    return f"FROM {IMAGE}\nUSER 1000:1000\nRUN <<'SPITZLI_NETWORK_PROBE'\n" + probe_script() + 'SPITZLI_NETWORK_PROBE\n'



def diagnostic_job(script, name='network-probe'):
    return {'apiVersion': 'batch/v1', 'kind': 'Job', 'metadata': {'name': name, 'namespace': NAMESPACE, 'labels': {'webdock.dev/owner': 'network-gate-v1'}}, 'spec': {'backoffLimit': 0, 'activeDeadlineSeconds': 300, 'template': {'metadata': {'labels': {'webdock.dev/component': 'network-probe'}}, 'spec': {'runtimeClassName': 'webdock-build-sandbox', 'automountServiceAccountToken': False, 'enableServiceLinks': False, 'restartPolicy': 'Never', 'securityContext': {'runAsUser': 1000, 'runAsGroup': 1000, 'runAsNonRoot': True, 'fsGroup': 1000, 'seccompProfile': {'type': 'RuntimeDefault'}}, 'containers': [{'name': 'probe', 'image': IMAGE, 'command': ['/bin/sh', '-ec', script], 'securityContext': {'allowPrivilegeEscalation': False, 'privileged': False, 'readOnlyRootFilesystem': True, 'capabilities': {'drop': ['ALL']}}, 'resources': {'requests': {'cpu': '250m', 'memory': '128Mi', 'ephemeral-storage': '32Mi'}, 'limits': {'cpu': '250m', 'memory': '128Mi', 'ephemeral-storage': '32Mi'}}, 'volumeMounts': [{'name': 'tmp', 'mountPath': '/tmp'}]}], 'volumes': [{'name': 'tmp', 'emptyDir': {'sizeLimit': '16Mi'}}]}}}}


def dns_outage_script():
    return ("set -eu\nif timeout 5 nslookup kubernetes.default.svc.cluster.local; then echo 'FAIL DNS answered during outage'; exit 1; fi\n"
            "if nc -z -w 1 10.85.0.10 53; then echo 'FAIL TCP DNS survived outage'; exit 1; fi\n"
            "echo 'PASS dns-failure-observed'\nfor attempt in $(seq 1 40); do\n"
            "if nc -z -w 1 10.85.0.10 53 && timeout 2 nslookup kubernetes.default.svc.cluster.local; then echo 'PASS dns-recovery-observed'; exit 0; fi\nsleep 1\ndone\nexit 1\n")


def live(churn=False, dns_outage=False):
    trusted = load('trusted_network', CLUSTER / 'trusted-system-network.py')
    trusted.preflight(idle=True)
    before = trusted.verify()
    # The namespace must be absent; never adopt/delete an operator's other objects.
    present = subprocess.run(['/usr/local/bin/k3s', 'kubectl', '--kubeconfig=/etc/rancher/k3s/k3s.yaml', 'get', 'namespace', NAMESPACE], capture_output=True)
    if present.returncode == 0: raise RuntimeError('diagnostic namespace already exists; inspect exact owned cleanup first')
    def apply(item): trusted.kubectl('apply', '-f', '-', input=json.dumps(item))
    apply({'apiVersion': 'v1', 'kind': 'Namespace', 'metadata': {'name': NAMESPACE, 'labels': {'webdock.dev/owner': 'network-gate-v1', 'pod-security.kubernetes.io/enforce': 'restricted', 'pod-security.kubernetes.io/enforce-version': 'v1.35'}}})
    namespace_uid = trusted.get('namespace', NAMESPACE, NAMESPACE)['metadata']['uid']
    logs = {}
    try:
        apply({'apiVersion': 'networking.k8s.io/v1', 'kind': 'NetworkPolicy', 'metadata': {'name': 'default-deny', 'namespace': NAMESPACE}, 'spec': {'podSelector': {}, 'policyTypes': ['Ingress', 'Egress'], 'ingress': [], 'egress': []}})
        apply(trusted.dns_policy(NAMESPACE))
        apply({'apiVersion': 'networking.k8s.io/v1', 'kind': 'NetworkPolicy', 'metadata': {'name': 'package-proxy-only', 'namespace': NAMESPACE}, 'spec': {'podSelector': {}, 'policyTypes': ['Egress'], 'egress': [{'to': [{'ipBlock': {'cidr': '10.83.0.1/32'}}], 'ports': [{'protocol': 'TCP', 'port': 3128}]}]}})
        apply({'apiVersion': 'v1', 'kind': 'ResourceQuota', 'metadata': {'name': 'bounded-probe', 'namespace': NAMESPACE}, 'spec': {'hard': {'pods': '2', 'requests.cpu': '1', 'limits.cpu': '1', 'requests.memory': '512Mi', 'limits.memory': '512Mi', 'requests.ephemeral-storage': '128Mi', 'limits.ephemeral-storage': '128Mi'}}})
        def finish(name):
            # Job failure is surfaced with bounded credential-free output.
            try:
                trusted.kubectl('wait', '--for=condition=complete', 'job/' + name, '-n', NAMESPACE, '--timeout=300s')
            finally:
                logs[name] = trusted.kubectl('logs', 'job/' + name, '-n', NAMESPACE, '--tail=200', '--limit-bytes=24000').stdout
            trusted.kubectl('delete', 'job', name, '-n', NAMESPACE, '--wait=true', '--timeout=30s')
        if not dns_outage:
            apply(diagnostic_job(probe_script()))
            finish('network-probe')
        else:
            try:
                trusted.kubectl('scale', 'deployment/coredns', '-n', 'kube-system', '--replicas=0')
                trusted.kubectl('wait', '--for=delete', 'pod', '-n', 'kube-system', '-l', 'k8s-app=kube-dns', '--timeout=60s')
                apply(diagnostic_job(dns_outage_script(), 'dns-outage'))
                deadline = time.monotonic() + 45
                while time.monotonic() < deadline:
                    observed = subprocess.run(['/usr/local/bin/k3s', 'kubectl', '--kubeconfig=/etc/rancher/k3s/k3s.yaml', 'logs', '-n', NAMESPACE, 'job/dns-outage', '--tail=10'], capture_output=True, text=True, timeout=10)
                    if 'PASS dns-failure-observed' in observed.stdout: break
                    time.sleep(1)
                else: raise RuntimeError('DNS outage was not observed by the sandbox')
            finally:
                trusted.kubectl('scale', 'deployment/coredns', '-n', 'kube-system', '--replicas=1')
                trusted.kubectl('rollout', 'status', 'deployment/coredns', '-n', 'kube-system', '--timeout=180s')
            finish('dns-outage')
        if churn:
            # Attack while both trusted systems disappear and recover. They have no
            # CNI IP to donate now; the firewall has no set or per-address authority.
            attack = "set -eu\necho ATTACK_STARTED\nfor attempt in $(seq 1 40); do\n" + denied_script([('10.83.0.1', 6443), ('10.83.0.1', 10250), ('10.85.0.1', 443)]) + "done\nnslookup kubernetes.default.svc.cluster.local\necho 'PASS trusted-system churn'\n"
            apply(diagnostic_job(attack, 'network-churn'))
            trusted.kubectl('wait', '--for=condition=Ready', 'pod', '-n', NAMESPACE, '-l', 'job-name=network-churn', '--timeout=60s')
            trusted.kubectl('delete', 'pod', '-n', 'kube-system', '-l', 'k8s-app=kube-dns', '--wait=false')
            trusted.kubectl('delete', 'pod', '-n', 'kube-system', '-l', 'k8s-app=metrics-server', '--wait=false')
            for name in ['coredns', 'metrics-server']:
                trusted.kubectl('rollout', 'status', 'deployment/' + name, '-n', 'kube-system', '--timeout=180s')
            finish('network-churn')
        return {'profile': before['profile'], 'gvisorPodProbes': logs, 'after': trusted.verify(), 'actualDockerfileRun': 'pending separate native build integration', 'dnsRebinding': 'run package-proxy/diagnose.py independently; it owns resolver restoration'}
    finally:
        current = trusted.get('namespace', NAMESPACE, NAMESPACE)
        if current['metadata']['uid'] != namespace_uid: raise RuntimeError('namespace identity changed; refusing cleanup')
        trusted.kubectl('delete', 'namespace', NAMESPACE, '--wait=true', '--timeout=60s')


def reuse_address(address, pod_cidr, allocated):
    value = ipaddress.ip_address(address)
    subnet = ipaddress.ip_network(pod_cidr)
    if (value.version != 4 or subnet.version != 4 or not subnet.subnet_of(ipaddress.ip_network('10.84.0.0/16'))
            or value not in subnet or int(value) <= int(subnet.network_address) + 1
            or value == subnet.broadcast_address or str(value) in allocated):
        raise ValueError('reuse proof needs an unallocated non-gateway address in the exact BUILD Pod subnet')
    return str(value)


def reuse_proof(address):
    trusted = load('trusted_reuse', CLUSTER / 'trusted-system-network.py')
    node = trusted.preflight(idle=True)
    trusted.verify()
    pods = json.loads(trusted.kubectl('get', 'pods', '-A', '-o', 'json').stdout)['items']
    allocated = {entry['ip'] for pod in pods for entry in pod.get('status', {}).get('podIPs', [])}
    address = reuse_address(address, node['spec']['podCIDR'], allocated)
    namespace, host_link = 'wd-build-net-proof', 'wd-net-proof0'
    if namespace in [line.split()[0] for line in trusted.run(['ip', 'netns', 'list']).stdout.splitlines()]:
        raise RuntimeError('proof namespace already exists; refusing adoption')
    links = json.loads(trusted.run(['ip', '-j', 'link']).stdout)
    if any(link['ifname'] == host_link for link in links): raise RuntimeError('proof link already exists')
    def counter():
        rules = json.loads(trusted.run(['nft', '-j', 'list', 'chain', 'inet', 'webdock_build_host', 'input']).stdout)['nftables']
        return sum(expr['counter']['packets'] for item in rules if 'rule' in item for expr in item['rule'].get('expr', []) if 'counter' in expr)
    # Plain connect only, running as nobody: no raw packet or customer privileges.
    program = "import json,socket; r=[];\nfor p in [53,3128,443,6443,10250,10251,22]:\n s=socket.socket();s.settimeout(1);e=s.connect_ex(('10.83.0.1',p));s.close();r.append({'port':p,'connected':e==0})\nprint(json.dumps(r))\nassert all(v['connected']==(v['port'] in [53,3128]) for v in r)"
    before = counter()
    trusted.run(['ip', 'netns', 'add', namespace])
    try:
        trusted.run(['ip', 'link', 'add', host_link, 'type', 'veth', 'peer', 'name', 'eth0', 'netns', namespace])
        trusted.run(['ip', 'link', 'set', host_link, 'master', 'cni0'])
        trusted.run(['ip', 'link', 'set', host_link, 'up'])
        for command in [['link', 'set', 'lo', 'up'], ['address', 'add', address + '/' + str(ipaddress.ip_network(node['spec']['podCIDR']).prefixlen), 'dev', 'eth0'], ['link', 'set', 'eth0', 'up'], ['route', 'add', 'default', 'via', str(ipaddress.ip_network(node['spec']['podCIDR']).network_address + 1)]]:
            trusted.run(['ip', '-n', namespace, *command])
        result = trusted.run(['ip', 'netns', 'exec', namespace, 'runuser', '-u', 'nobody', '--', '/usr/bin/python3', '-c', program])
        dropped = counter() - before
        if dropped < 5: raise RuntimeError('host gate did not prove every forbidden connection')
        return {'formerTrustedPodAddress': address, 'source': 'operator-owned netns, unprivileged TCP connects', 'connections': json.loads(result.stdout), 'hostInputDroppedPackets': dropped, 'cleanup': 'exact proof netns and veth removed in finally'}
    finally:
        trusted.run(['ip', 'netns', 'delete', namespace])
        # Network-namespace destruction is asynchronous while TCP sockets retire.
        # Wait on observed absence rather than declaring a leaked veth immediately.
        deadline = time.monotonic() + 10
        while True:
            remaining = json.loads(trusted.run(['ip', '-j', 'link']).stdout)
            if not any(link['ifname'] == host_link for link in remaining): break
            if time.monotonic() >= deadline: raise RuntimeError('proof veth cleanup incomplete')
            time.sleep(0.1)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--dockerfile', action='store_true')
    mode.add_argument('--on-build-host', action='store_true')
    parser.add_argument('--exercise-churn', action='store_true')
    parser.add_argument('--exercise-dns-outage', action='store_true')
    parser.add_argument('--reuse-system-ip', help='owned former system-Pod IP; standalone root-netns proof only')
    args = parser.parse_args()
    if args.dockerfile:
        if args.exercise_churn or args.exercise_dns_outage or args.reuse_system_ip: parser.error('live probe flags require --on-build-host')
        print(dockerfile(), end='')
    else:
        if sum([bool(args.reuse_system_ip), args.exercise_churn, args.exercise_dns_outage]) > 1: parser.error('run churn, outage and former-IP reuse separately')
        print(json.dumps(reuse_proof(args.reuse_system_ip) if args.reuse_system_ip else live(args.exercise_churn, args.exercise_dns_outage), indent=2))


if __name__ == '__main__': main()
