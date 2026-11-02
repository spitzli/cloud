# Standalone build host

Provisioned on 2026-10-29 for the explicitly authorized new Contabo VPS.
Contabo display name: **Webdock Build EU - 01**. Linux/Kubernetes node name:
`webdock-build-eu-01`. Host: `167.86.75.144` (instance `vmd210866`).
This is a separate build cluster; the existing runtime cluster was not modified.

## Delivered boundary

- Ubuntu 24.04.5, kernel 6.8.0-147-generic, 6 vCPUs, 11960 MiB RAM, 200 GB disk.
- K3s `v1.35.9+k3s1`, checksum-verified binary, containerd `2.2.7-k3s1`.
- Single server/control-plane/worker, own datastore and generated cluster identity.
- Private host/API `10.83.0.1`, pods `10.84.0.0/16`, services `10.85.0.0/16`.
  `webdock-build0` is a local dummy interface, **not a VPN**. Access uses SSH
  forwarding; persistent controller networking and later worker connectivity still
  require setup. These ranges differ from the runtime 10.42/10.43 ranges found in
  repository evidence; recheck all VPN routes before connecting networks.
- API/kubelet bind private addresses. The 2026-10-30 trusted-system network
  profile runs only CoreDNS/metrics-server on reviewed host networking. Customer
  Pod interfaces and Pod CIDRs are denied host access before conntrack, except
  exact private DNS/package-proxy paths and explicitly owned temporary data ports.
  No system-Pod IP grants API authority; the old IP-refresh timer is disabled and
  its compatibility script can only flush the old set. See the [network cutover
  runbook](../../docs/operations/build-network.md) and current functional evidence.
- No Traefik, ServiceLB, local-path provisioner, customer apps or run agent installed.
- Secret encryption at rest; root-only kubeconfig and host configuration.
- Tokenless build Pod identity. The diagnostic dispatcher has no Secret access;
  the qualified executor additionally creates/gets/deletes only admitted immutable
  attempt handoffs. Neither identity can list Secrets, exec, create Pods or inspect nodes.
- Restricted Pod Security plus fail-closed Job/Pod admission, quota/limits,
  default-deny ingress/egress, narrowly scoped CoreDNS allowance.
- Namespace storage/CPU/RAM quotas and pod deadlines; no automatic retries or TTL
  before outcome/log capture. The qualified namespace admits only the operator-approved immutable runner.
  The original diagnostic namespace keeps its strict Restricted profile.

Initial **configured planning limit: one build at a time**, not a throughput or
hostile-code readiness claim. The namespace caps workloads at 4 CPUs, 6 GiB memory,
40 GiB ephemeral storage. Kubernetes reserves 1 CPU and 2 GiB for host/Kubernetes,
with additional memory/disk eviction thresholds. Measure real builds before raising
concurrency; a namespace aggregate quota is not the future dispatcher's fair queue.

## Files

| File | Responsibility |
| --- | --- |
| `00-webdock-keys-only.conf` | Explicitly authorized SSH public-key-only policy, installed separately after key verification. |
| `bootstrap.sh` | Fresh-host guard, pinned binary install, systemd and hostname. |
| `config.yaml` | Explicit standalone K3s configuration; no run-cluster URL/token. |
| `host.nft` | Dedicated host table, ingress restriction and additional pod egress blocks. |
| `sync-system-pods.py` | Compatibility cleanup only: flush obsolete IP authority; never grants Pod API access. |
| `trusted-system-network.py` | Explicit reviewed CoreDNS/metrics host-network cutover with verification; separate from fresh bootstrap. |
| `bounded-logs.py` | Separate fixed-filesystem log mount and fail-closed K3s lifetime binding. |
| `foundation.yaml` | Namespace, identities/RBAC, quotas, policies and admission. |
| `smoke.yaml` | Nonroot/tokenless DNS and negative egress check Job. |
| `buildkit-qualification.yaml` | Rootless BuildKit scratch/COPY-to-OCI qualification under unchanged profiles. |
| `verify.py` | Actual admission dry-run/RBAC negative tests, explicitly targeted. |

No Pulumi project: this existing VPS needs configuration, not another cloud resource
lifecycle/state store. The versioned host profile is sufficient for one server and
can later be invoked by an existing provisioning system. No credential is in these
files. Application changes are separate from this host bootstrap; see the
[implementation runbook](../../docs/operations/kubernetes-builds.md).

## Repeatable setup

The commands below are the **original fresh-host foundation**, not the complete
current hostile-build profile. Before source Jobs, follow the separately reviewed
[trusted-network cutover](../../docs/operations/build-network.md),
[hard storage lifecycle](../../docs/operations/build-storage.md), bounded-log
mount, sandbox and current runner/controller qualification. Never rerun the old
bootstrap over a hardened live host merely to update it; its drift guard and
independent operating profiles must be preserved.


Do not run on a runtime node. The bootstrap refuses an existing unowned K3s
installation and refuses config/version drift on rerun; it never formats storage,
uninstalls clusters or rotates account/cluster credentials. SSH identity is an
operator prerequisite, not embedded in the bootstrap.

```sh
# Offline, from repository root:
python3 -B -m unittest discover -s tools/tests -p test_build_cluster.py
bash -n deploy/build-cluster/bootstrap.sh

# Copy this directory to the explicitly selected new build host first.
# On that host, only after inventory/authorization:
bash /root/webdock-build-bootstrap/bootstrap.sh --standalone-build-server
k3s kubectl wait --for=condition=Ready node/webdock-build-eu-01 --timeout=90s
k3s kubectl apply -f /root/webdock-build-bootstrap/foundation.yaml
```

Allow admission-policy propagation before running tests. Verify policy
`.status.typeChecking.expressionWarnings` is empty; `verify.py` checks this too.

```sh
# Local conversion avoids requiring PyYAML on the server:
python3 -c 'import json,yaml; print(json.dumps(yaml.safe_load(open("deploy/build-cluster/smoke.yaml"))))' > /tmp/smoke.json
# Transfer smoke.json to the build host's private bootstrap directory.
# On the build host (does not create persistent hostile Jobs):
python3 /root/webdock-build-bootstrap/verify.py --on-build-host \
  --job-json /root/webdock-build-bootstrap/smoke.json
k3s kubectl apply -f /root/webdock-build-bootstrap/smoke.yaml
k3s kubectl wait -n webdock-builds --for=condition=complete job/foundation-smoke --timeout=90s
k3s kubectl logs -n webdock-builds job/foundation-smoke
```

The validation script refuses a cluster whose node inventory differs from this
single named build node. `--kubeconfig` is an alternative to `--on-build-host` for an
explicit **administrator** fixture connection; the script's node inspection,
impersonation and policy inspection are intentionally beyond dispatcher authority.
Never grant those privileges to the dispatcher merely to run the verifier.

Use `create --dry-run=server` for negative tests: admission must reject privilege,
host namespaces/mounts, token mounts, mutable images, profile changes, secret env,
unsafe init containers, placement overrides, fanout and premature TTL. Real smoke
execution checks DNS and denied API/kubelet, runtime API/PostgreSQL, metadata and
public Internet connections. It is not an exhaustive adversarial certification.

## Private API access

Example local operator tunnel (leave it running in its own terminal):

```sh
ssh -o IdentityAgent=none -o IdentitiesOnly=yes -i ~/.ssh/id_ed25519 \
  -N -L 127.0.0.1:16443:10.83.0.1:6443 root@167.86.75.144
```

Verify TLS using the build cluster's CA and the `127.0.0.1` certificate SAN.
Use a namespace-scoped, short-lived dispatcher token for application connectivity;
never copy `/etc/rancher/k3s/k3s.yaml` into the Webdock API or customer configuration.
A one-hour diagnostic dispatcher credential was issued to verify this path; it is
not a durable integration. Credential files stayed in ignored/private storage.
A production dispatcher needs a separately managed identity and renewal procedure.

SSH Ed25519 host fingerprint observed on first use:
`SHA256:cR6EZrVnRWpFr4iEicoJijtPF/VJwMwTB/lR99eng5Y`.
The existing local operator public key was appended without removing other keys.
No password or existing live credential was rotated.

## SSH key-only authentication — 2026-10-29

At the owner's explicit request, the supplied RSA 3072-bit public key was added to
root's authorized keys, preserving existing operator keys. Fingerprint:
`SHA256:CwQmWNIewJdih7OvnfCDkYiJb0n/fz1nByLSvwLMK/o`.

`00-webdock-keys-only.conf` is installed under `/etc/ssh/sshd_config.d/` before
cloud-init's settings. It requires public-key authentication, disables password
and keyboard-interactive authentication, and permits root only with keys. The
existing account password was not changed. RSA uses modern negotiated signatures;
legacy SHA-1 `ssh-rsa` signature algorithms were not enabled.

After confirming a separate key-authenticated connection, the repeatable install
on this host is:

```sh
install -o root -g root -m 0600 /root/webdock-build-bootstrap/00-webdock-keys-only.conf \
  /etc/ssh/sshd_config.d/00-webdock-keys-only.conf
/usr/sbin/sshd -t
systemctl reload ssh
```

The actual change was syntax-checked before reload, then verified with a fresh
operator-key connection and a connection without public-key authentication, which
the server rejected with only `publickey` offered. The owner's private RSA key was
not requested or accessed; its public key format/fingerprint and installation were
verified. These SSH settings persist independently of the K3s bootstrap.

## Qualified container runtime and bounded exception

Ordinary rootless BuildKit under the original Restricted profile failed at
RootlessKit startup (`operation not permitted`). This failed fixture remains in
`buildkit-qualification.yaml` as evidence, rather than weakening its policy.

The separate `webdock-build-qualification` namespace uses pinned gVisor
`20261026.0`, **systrap** platform and sandbox networking. This is a container
runtime with an application kernel; it does not require KVM or a per-build VM.
The host retains `kernel.apparmor_restrict_unprivileged_userns=1`.

RootlessKit's setuid ID-map helpers require `SETUID`, `SETGID` and
`allowPrivilegeEscalation` inside that sandbox. This reviewed exception applies
only to the approved immutable main runner in the exact qualified namespace and
RuntimeClass. Init containers retain no capabilities/no privilege escalation.
PSA Baseline is supplemented by fail-closed Job **and Pod** admission to restore
all the other restrictions. There is no privileged Pod, host mount/socket/network,
Unconfined profile or `--oci-worker-no-process-sandbox` fallback. A real
Dockerfile `COPY` and `RUN` exported validated OCI under this profile.

`install-sandbox.sh --qualified-build-host` refuses other hosts and active builds
before restarting K3s. Render the qualified namespace offline with an actual
approved digest, then apply its profile and both admission policies as an
operator. Do not put an administrator kubeconfig in the application.

```sh
python3 deploy/build-cluster/render-sandbox.py \
  --runner-image '<registry>/<runner>@sha256:<verified manifest digest>' \
  > /private/qualified-build-profile.yaml
# On the explicitly selected build host, with no active builds:
bash /root/webdock-build-bootstrap/install-sandbox.sh --qualified-build-host
k3s kubectl apply -f /private/qualified-build-profile.yaml
k3s kubectl apply -f /root/webdock-build-bootstrap/foundation.yaml
k3s kubectl apply -f /root/webdock-build-bootstrap/handoff-admission.yaml
```

`verify-qualified.py` accepts an explicit administrator connection plus an observed
private runner Job JSON. It performs only server dry runs of Jobs **and Pods**:
fixed runner/download commands and mounts pass; diagnostic-image substitutions,
process-sandbox overrides, init capabilities and executable/configuration shadowing
are denied. The qualified namespace no longer admits the old diagnostic BuildKit
image. A review found and closed that bypass before application activation.

After tightening templates, already-deleting historical objects may need their
finalizers removed. Admission allows that metadata cleanup only with identical
spec, UID, labels, annotations and owner references and no added finalizers.
Changing an image while deleting is still rejected. This termination-only path
was tested against the actual API and cannot authorize new execution.

The ordinary namespace remains independent. Do not remove the profile label,
swap RuntimeClass or approve a mutable image to get a build working.

## Controlled egress

The versioned [package proxy](package-proxy/README.md) admits only operator-approved
HTTPS package/image domains on CONNECT port 443. The host firewall independently
blocks private, metadata, CGNAT and known runtime-server addresses for the proxy
UID. DNS-rebinding checks and IPv4/IPv6 denials were run on this installation.
Build Pods have only CoreDNS, that proxy and a separate exact staging endpoint
allowance. They have no direct registry write/Git/platform authority.

The operator must configure staging in **both** places: an independent HTTPS
origin/CA policy embedded in the runner and controller, and a literal endpoint
network policy plus any required external firewall. A URL permission does not
grant network access. The real upload test found a missing staging egress rule;
we fixed the exact endpoint allowance rather than opening general Internet access.

```sh
# Render only; review the endpoint's ownership and routing before applying.
python3 deploy/build-cluster/render-staging-policy.py \
  --address '<verified staging IP>' --port 443 > /private/staging-egress.json
```

The helper emits a /32 or /128 destination for build-job Pods only. It rejects
unspecified, loopback, link-local/metadata and multicast destinations. It cannot
establish endpoint ownership or replace TLS and host/VPN firewall verification.
The temporary owned TLS test store on port 19443 is not a production artifact
service; its credentials, firewall exceptions and resources have bounded custody.

Customer admission remains disabled by default and requires explicit trusted
project enrollment. gVisor and network qualification reduce exposure; they do
not constitute a public arbitrary-code certification. See the implementation
runbook for application state, actual acceptance evidence and remaining gates.

## Operations and recovery

Inspect `systemctl status k3s webdock-build-network`, confirm the obsolete
`webdock-build-system-pods.timer` stays disabled, inspect `journalctl -u k3s`,
`k3s kubectl get nodes,pods -A` and `nft list table inet webdock_build_host`.
Systemd recreates the private dummy interface and firewall before K3s on boot.
Do not run `nft flush ruleset`: Kubernetes owns its forwarding/NAT chains.
Only the dedicated `webdock_build_host` table belongs to this profile.

Before deleting any diagnostic Job, persist its log/outcome in the root-only
bootstrap directory. Application builds persist bounded authorized logs before deletion; manual
diagnostic capture remains separate from that application log path.
`verify.py` only dry-runs negative payloads and never deletes workloads.

Local configuration and cluster SQLite state live under `/etc/rancher/k3s` and
`/var/lib/rancher/k3s`. A complete encrypted, off-host recovery procedure must retain
the server token and encryption material with a consistent datastore backup.
No off-host backup destination/credential was supplied, so backup/recovery has not
been qualified. Do not delete these paths, regenerate the token or run an uninstall
script as an upgrade. Single-node loss stops new builds; it cannot stop the separate
runtime cluster. This cluster is not HA, and no disaster-recovery SLA is claimed.

References: [K3s configuration](https://docs.k3s.io/installation/configuration),
[hardening](https://docs.k3s.io/security/hardening-guide),
[requirements](https://docs.k3s.io/installation/requirements),
[BuildKit rootless](https://github.com/moby/buildkit/blob/master/docs/rootless.md).
