#!/usr/bin/python3
"""Credential-free diagnostics in a temporary, restricted namespace on the owned build node."""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shlex
import socket
import subprocess
import time

ROOT = Path(__file__).resolve().parent
SPEC = importlib.util.spec_from_file_location("package_proxy_manage", ROOT / "manage.py")
manage = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(manage)
NAMESPACE = "webdock-package-proxy-diagnostics"
IMAGE = "docker.io/moby/buildkit:v0.34.1-rootless@sha256:f68752a5a94acfcdad2d07867b7075601a421638e17fc5f28efc1c814d0c4f89"
PROXY = "10.83.0.1"

def apply(value):
    manage.run(["/usr/local/bin/k3s", "kubectl", "--kubeconfig", "/etc/rancher/k3s/k3s.yaml", "--context", "default", "apply", "-f", "-"], input=json.dumps(value).encode())

def setup():
    apply({"apiVersion":"v1","kind":"Namespace","metadata":{"name":NAMESPACE,"labels":{"webdock.dev/owner":"package-proxy-diagnostics","pod-security.kubernetes.io/enforce":"restricted","pod-security.kubernetes.io/enforce-version":"v1.35"}}})
    apply({"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicy","metadata":{"name":"deny-all","namespace":NAMESPACE},"spec":{"podSelector":{},"policyTypes":["Ingress","Egress"],"ingress":[],"egress":[]}})
    apply({"apiVersion":"networking.k8s.io/v1","kind":"NetworkPolicy","metadata":{"name":"dns-and-package-proxy","namespace":NAMESPACE},"spec":{"podSelector":{},"policyTypes":["Egress"],"egress":[{"to":[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":"kube-system"}},"podSelector":{"matchLabels":{"k8s-app":"kube-dns"}}}],"ports":[{"protocol":"UDP","port":53},{"protocol":"TCP","port":53}]},{"to":[{"ipBlock":{"cidr":"10.83.0.1/32"}}],"ports":[{"protocol":"TCP","port":3128}]}]}})
    apply({"apiVersion":"v1","kind":"ResourceQuota","metadata":{"name":"diagnostic-limit","namespace":NAMESPACE},"spec":{"hard":{"pods":"2","requests.cpu":"1","limits.cpu":"1","requests.memory":"512Mi","limits.memory":"512Mi","requests.ephemeral-storage":"128Mi","limits.ephemeral-storage":"128Mi"}}})

def job(name, script):
    value={"apiVersion":"batch/v1","kind":"Job","metadata":{"name":name,"namespace":NAMESPACE,"labels":{"webdock.dev/owner":"package-proxy-diagnostics"}},"spec":{"backoffLimit":0,"activeDeadlineSeconds":90,"template":{"metadata":{"labels":{"webdock.dev/component":"package-proxy-diagnostic"}},"spec":{"runtimeClassName":"webdock-build-sandbox","automountServiceAccountToken":False,"restartPolicy":"Never","enableServiceLinks":False,"securityContext":{"runAsUser":1000,"runAsGroup":1000,"runAsNonRoot":True,"fsGroup":1000,"seccompProfile":{"type":"RuntimeDefault"}},"containers":[{"name":"probe","image":IMAGE,"imagePullPolicy":"IfNotPresent","command":["/bin/sh","-ec",script],"securityContext":{"allowPrivilegeEscalation":False,"privileged":False,"readOnlyRootFilesystem":True,"capabilities":{"drop":["ALL"]}},"resources":{"requests":{"cpu":"250m","memory":"128Mi","ephemeral-storage":"32Mi"},"limits":{"cpu":"250m","memory":"128Mi","ephemeral-storage":"32Mi"}},"volumeMounts":[{"name":"tmp","mountPath":"/tmp"}]}],"volumes":[{"name":"tmp","emptyDir":{"sizeLimit":"16Mi"}}]}}}}
    apply(value)
    deadline=time.monotonic()+100
    while time.monotonic()<deadline:
        observed=json.loads(manage.kubectl("get","job",name,"-n",NAMESPACE,"-o","json").stdout)
        conditions=observed.get("status",{}).get("conditions",[])
        if any(c.get("status")=="True" and c.get("type") in {"Complete","Failed"} for c in conditions):
            logs=manage.kubectl("logs","job/"+name,"-n",NAMESPACE,"--tail=100","--limit-bytes=24000").stdout.decode()
            manage.kubectl("delete","job",name,"-n",NAMESPACE,"--wait=true","--timeout=30s")
            if not any(c.get("status")=="True" and c.get("type")=="Complete" for c in conditions):
                raise RuntimeError("diagnostic_job_failed:"+logs[:1000])
            return logs
        time.sleep(1)
    raise RuntimeError("diagnostic_job_timeout")

def denied(name, authority, method="CONNECT"):
    target=authority if method=="CONNECT" else "http://"+authority+"/"
    payload=f"{method} {target} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
    return f"reply=$(printf '%s' {shlex.quote(payload)} | nc -w 3 {PROXY} 3128 | head -c 1024); code=$(printf '%s' \"$reply\" | head -n 1 | awk '{{print $2}}'); case \"$code\" in 400|403) echo 'PASS {name} denied';; *) echo 'FAIL {name} unexpected-status'; exit 1;; esac\n"

def positive(host,path,status,method="HEAD"):
    request=f"{method} {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n"
    return f"set +e; printf '%s' {shlex.quote(request)} | timeout 15 openssl s_client -quiet -ignore_unexpected_eof -proxy {PROXY}:3128 -connect {host}:443 -servername {host} -verify_hostname {host} -verify_return_error -CAfile /etc/ssl/certs/ca-certificates.crt >/tmp/tls.response 2>/tmp/tls.error; tls_status=$?; set -e; if [ \"$tls_status\" != 0 ]; then echo 'FAIL {host} TLS'; head -n 8 /tmp/tls.error; exit 1; fi; if ! head -n 1 /tmp/tls.response | grep -q ' {status} '; then echo 'FAIL {host} HTTP-status'; head -n 1 /tmp/tls.response; exit 1; fi; echo 'PASS {host} HTTPS verified'\n"

def smoke():
    script="set -eu\nnslookup registry.npmjs.org >/tmp/dns.out\n"
    for name,target in [("private","127.0.0.1:443"),("api-private","10.83.0.1:443"),("metadata","169.254.169.254:443"),("cgnat","100.100.100.200:443"),("run-1","213.136.65.22:443"),("run-2","79.143.177.157:443"),("run-3","169.58.95.66:443"),("unknown-domain","example.com:443"),("wrong-port","registry.npmjs.org:80"),("ipv6-local","[::1]:443")]:
        script+=denied(name,target)
    script+=denied("plain-http","registry.npmjs.org","GET")
    script+=denied("debian-plain-http","deb.debian.org","GET")
    script+="if timeout 4 nc -z -w 3 registry.npmjs.org 443; then echo 'FAIL direct-approved-TCP'; exit 1; else echo 'PASS direct-approved-TCP denied'; fi\n"
    script+=positive("registry.npmjs.org","/is-number",200)
    script+=positive("ghcr.io","/v2/",401,"GET")
    script+=positive("deb.debian.org","/debian/dists/trixie/InRelease",200)
    return job("package-proxy-smoke",script)

def rebind():
    hosts=Path("/etc/squid/webdock-package-proxy.hosts")
    original=hosts.read_bytes()
    if original:
        raise RuntimeError("unexpected_proxy_hosts_override")
    manage.run(["systemctl","stop","webdock-package-proxy-firewall.timer"])
    results=[]
    try:
        for name,address in [("private","127.0.0.1"),("metadata","169.254.169.254"),("run","213.136.65.22")]:
            manage.write(hosts,(address+" registry.npmjs.org\n").encode())
            manage.run(["systemctl","reload","webdock-package-proxy"])
            results.append(job("package-rebind-"+name,"set -eu\n"+denied("approved-host-rebound-"+name,"registry.npmjs.org:443")))
    finally:
        manage.write(hosts,original)
        manage.run(["systemctl","reload","webdock-package-proxy"])
        manage.run(["systemctl","start","webdock-package-proxy-firewall.timer"])
    return "".join(results)

def kernel_boundary():
    def counters():
        rules=json.loads(manage.run(["nft","-j","list","chain","inet","webdock_package_proxy","proxy_output"]).stdout)["nftables"]
        return sum(expression["counter"]["packets"] for item in rules if "rule" in item for expression in item["rule"].get("expr",[]) if "counter" in expression)
    program="import socket,sys; s=socket.socket(); s.settimeout(2); result=s.connect_ex((sys.argv[1],443)); print(result); sys.exit(0 if result else 1)"
    manage.run(["systemctl","stop","webdock-package-proxy-firewall.timer","webdock-package-proxy-firewall-refresh.service"])
    try:
        before=counters()
        addresses=["127.0.0.1","169.254.169.254","100.100.100.200","213.136.65.22","79.143.177.157","169.58.95.66"]
        for address in addresses:
            result=manage.run(["/usr/sbin/runuser","-u","proxy","--","/usr/bin/python3","-c",program,address])
            if result.stdout.strip()==b"0":raise RuntimeError("proxy_uid_egress_bypass")
        if counters()-before<len(addresses):raise RuntimeError("proxy_uid_denials_not_proven_by_kernel_counters")
    finally:
        manage.run(["systemctl","start","webdock-package-proxy-firewall.timer"])
    return "PASS direct proxy-UID private/metadata/Run egress denied; kernel counters confirmed\n"

def cleanup():
    manage.kubectl("delete","namespace",NAMESPACE,"--wait=true","--timeout=45s")

def main():
    parser=argparse.ArgumentParser();parser.add_argument("--on-new-build-host",action="store_true",required=True);args=parser.parse_args()
    if os.geteuid()!=0:raise RuntimeError("root_required")
    manage.assert_inventory(manage.inventory());manage.check_installed()
    result={"smoke":"","rebind":"","kernel":""}
    setup()
    try:
        result["smoke"]=smoke();result["kernel"]=kernel_boundary();result["rebind"]=rebind()
        result["postRestore"]=job("package-proxy-restored","set -eu\n"+positive("registry.npmjs.org","/is-number",200))
        print(json.dumps(result,indent=2))
    finally:
        cleanup()

if __name__=="__main__":main()
