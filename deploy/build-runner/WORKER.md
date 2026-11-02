# Trusted Kubernetes build worker

Build the controller daemon separately from the disposable runner:

```sh
cargo build -p webdock-build --no-default-features --features service,worker,kubernetes-job \
  --bin webdock-kubernetes-build-worker
webdock-kubernetes-build-worker --config /private/kubernetes-worker.json
```

The hardened worker requires the `service` feature for its isolated bounded
upload listener. The minimal `worker,kubernetes-job` build supports only the
explicit `trustedPilotOnly` legacy path and never starts an upload server.

`--once` is rejected when an upload gateway/listener is configured, before any
new dispatch or background service starts: a finite dispatcher turn cannot leave
a build relying on a dead result listener. In explicit legacy pilot mode it runs
one dispatcher turn and optional publication turn, then cancels remaining local
Jobs before exiting. It can publish a claimed release; it is not a health check.
Publication can wait for its existing bounded release lease/rollout observation.

Run this daemon under a dedicated trusted Unix identity on the controller. Its
worker token, namespace-scoped Kubernetes token, S3 staging credentials and optional
registry credentials remain there. The disposable build Job receives only its own
source GET/result PUT capabilities and customer build inputs. This executable has
no VM image, KVM check, guest-command path or ambient backend factory.

`worker.example.json` is deliberately disabled and uses invalid placeholder hosts,
IDs and pins. Copy it into a private file and replace every operator input; it is
not deployable configuration. `webdock-kubernetes-build-worker.service` is an
uninstalled controller-side systemd template. Provision its dedicated Unix user,
owned mode-0600 configuration/credential files and the four distinct mode-0700
subdirectories under `/var/lib/webdock-build` before installing it. The controller
unit now budgets 6 GiB RAM with swap disabled. An actual maximum 512-MiB gateway
collect/verify overlapped with a real registry publisher at a measured 4.561-GiB
peak, leaving 1.439 GiB headroom with no OOM. See the [measured overlap evidence](../../docs/verification/2026-10-30-build-worker-overlap-memory.json).
This does not qualify the complete Core/dispatcher plus MAX_SOURCE parsing path;
Core remains a separate process and build admission remains one. This cap is a
worker-process budget, not a build Pod limit. Do not install the controller daemon on a customer build
execution node or add customer execution privileges to this service.

Both the configuration's `enabled` flag and Core's operator-controlled private
Kubernetes admission must be enabled explicitly. An omitted `enabled` is false;
the daemon fails closed before touching credentials or discovering a cluster.
No public enrollment or customer trust is inferred from configuration.

## Configuration fields

The file is strict JSON; unknown fields are rejected. Secret values belong only
in separate private credential files, not in the main configuration.

| Field | Contract |
| --- | --- |
| `enabled` | Optional, default false. |
| `controlURL` | Explicit HTTPS private worker API base, with no embedded credentials/query/fragment. |
| `credentialFile` | Private file containing the existing enrolled 43-character Webdock worker token. |
| `kubeconfigFile` | One explicit, private, minimal kubeconfig; no ambient fallback. |
| `kubeContext` | Exact named context selected from that file. |
| `apiEndpoint` | Expected HTTPS Kubernetes API origin, including its explicit port when needed. |
| `caSHA256` | SHA-256, lowercase hex, of the **base64-decoded PEM bytes** in `certificate-authority-data`; not a DER certificate fingerprint. |
| `buildClusterID` | Positive numeric Core build-cluster ID, represented as a string. |
| `runnerImage` | Exact immutable OCI image reference with `@sha256:`; mutable/latest references are rejected. |
| `trustedProjects` | Explicit set of `{customer_id, project_id}` positive numeric ID pairs. |
| `stateRoot` | Private durable dispatcher receipts, completion outbox and process locks. |
| `artifactRoot` | Private retained local artifact store, independent of staging. |
| `stagingRoot` | Private durable per-attempt handoff state; contains scoped capabilities/customer inputs. |
| `transferPolicyFile` | Independently owned operator policy matching the pinned runner image's allowed HTTPS S3 origins and optional public CA. |
| `stagingStorage` | Existing `S3Config` under `storage`, and a private `credentialFile` for existing object-storage `Credentials`. |
| `artifactStoreLimit` | Optional bytes; default 20 GiB, finite range 512 MiB–200 GiB. |
| `pollSeconds` | Optional; default 5, finite range 1–60. |
| `publication` | Optional publication configuration described below. |
| `storage` | Required unless `trustedPilotOnly`; explicit signed storage target (`clusterID`, `node`, `poolID`), private `signingKeyFile`, distinct `requestsNamespace`/`receiptsNamespace`. No signer reaches a Pod. |
| `uploadGateway` | Required unless `trustedPilotOnly`; dedicated private hard-bounded tmpfs `root`, finite `filesystemCeilingBytes`, allowed HTTPS `origin`, loopback `listen` address with nonzero port, and bounded `timeoutSeconds`. Requires the `service` feature. |
| `ownedTokenFile` | Optional only for explicit pilot compatibility. Hardened enabled workers require this absolute, owned mode 0600 raw-token file under a distinct owned mode 0700 parent, paired with `identity`. |
| `identity` | Paired managed controller identity: actual `serviceAccountUID`, `revocationSecretUID` and exact API `audience`. Namespace/account and revocation Secret name are fixed in code. |
| `trustedPilotOnly` | Optional, default false. Explicitly permits legacy inline identity and emptyDir/provider-PUT pilot; it does not claim managed renewal, hard storage or bounded upload authority. |

The executor profile is fixed in code: namespace `webdock-build-qualification`,
RuntimeClass `webdock-build-sandbox`, service account `webdock-build-job`, and the
qualified RootlessKit id-map exception inside gVisor systrap. Configuration cannot
select a different namespace/runtime, shell backend, host socket or VM.
The namespace's admission policy must independently admit the exact runner digest.

All input files must be absolute regular files, owned by the daemon's effective
UID, without symlinks, multiple hardlinks or group/other permissions. Read-only
owner files are accepted. Paths through symlinked parent directories are rejected;
use their canonical paths. State/artifact/staging roots and optional publication
root must already exist with mode **0700**, belong to the daemon UID, and have no
equality or ancestor overlap. The daemon creates mode-0600 locks and state files.
A second process using the same state/publication directory is rejected.

## Kubernetes identity

Use a dedicated namespace-scoped dispatcher identity with the reviewed Job/Pod/log
and exact per-attempt Secret permissions. Never copy the cluster administrator's
kubeconfig, runtime-cluster credentials, or an application service account into
this daemon. The loader accepts one cluster, one context and one inline bearer
identity. It rejects client certificates/keys, credential exec/auth-provider
plugins, impersonation, external token/CA files, proxy/TLS-name overrides, insecure
TLS and unknown transport/authentication fields. Kubernetes still enforces RBAC;
the operator must provision and audit the scoped token's actual permissions.

Only the configured context is selected; `current-context`, `KUBECONFIG`, home
configuration, in-cluster discovery and proxy environment variables do not provide
fallbacks. The client is constructed directly after endpoint and CA-pin checks,
with TLS verification enabled, only the pinned root bundle, finite connection/I/O
timeouts, and automatic API retries disabled. Legacy inline-token compatibility is
restricted to explicit `trustedPilotOnly:true` factory configuration and low-level
owned tests. Hardened enabled factories require the paired managed identity even
when storage and upload are configured. CA changes remain an explicit private
configuration/requalification action.

The managed path still runs the same strict kubeconfig trust checks first; token
file or exec references inside kubeconfig remain rejected. Then, only for the
separate explicit `ownedTokenFile`, it verifies private file custody and asks the
CA-pinned private API to authenticate that exact bearer with SelfSubjectReview.
The API must report the exact controller ServiceAccount username and recorded UID,
with only its normal ServiceAccount groups. Only after API authentication does it
use JWT claims to check that UID, the exact audience, revocation Secret UID and
finite lifetime. No decoded unsigned claim establishes authority.

Managed identity uses `https://10.83.0.1:6443` over separately qualified private
connectivity and fixed account `webdock-build-controller` in that namespace. The
file contains the raw JWT with no whitespace/newline; its exact mode is0600,
immediate parent mode 0700, same effective owner, no symlink/hardlink and at most
16 KiB. The parent identity is fenced against directory replacement and must not
overlap state/artifact/staging/publication directories. Only the maintained
kube-client token-file auth is installed after clearing the inline token. It
reloads on requests after about 50 seconds; no dispatcher restart is used.

A separately supervised task requests a one-hour token every five minutes, bound
to the same revocation Secret name+UID. It checks the returned expiry and
reauthenticates the candidate through the pinned API before replacing the file.
Atomic mode 0600 write, file fsync, rename and directory fsync happen under the
same lock as the in-memory hash/expiry update. An exclusive directory lock also
prevents a second writer competing for the same identity file. There is no independently
replaceable expiry sidecar. Restart reauthenticates the sole persisted token,
including after a crash between file persistence and memory publication.

The task audits custody/hash every second independently of renewal I/O. It also
uses a monotonic lifetime bound, so a wall-clock rollback cannot extend authority.
Transient API failure retries every 15 seconds only while safe authority remains.
Unsafe/missing/replaced files, UID/audience/revocation mismatch, revoked API
credentials, or 35 minutes or less remaining authority stop dispatch through the
existing exact-attempt shutdown path while the upload gateway stays alive for
revocation/cancellation. Receipt/volume uncertainty is retained; nothing is purged.
The same margin is required before exposing a managed client and before each
dispatch turn. Startup first authenticates a still-valid bootstrap and proves
actual self-renewal, so missing renewal RBAC/admission fails before source effects;
a valid short-margin bootstrap is used only for that fresh renewal. Successful
rotation neither cancels Jobs nor restarts the worker. No token reaches a Job,
source/result bundle or registry publisher configuration.

Provision the separate scoped RoleBindings and named self-TokenRequest admission
from the [controller runbook](../../docs/operations/build-controller.md) before
opting in. Infrastructure bootstrap must first prove the exact owned VAP/binding
rejects a wrong-audience canary (HTTP 422 is valid); zero typechecking warnings and
positive renewal alone do not prove policy propagation. No bootstrap delivery or
worker start precedes that negative barrier. The backend/dispatcher target is the
existing Webdock RUN cluster, with frontend on Vercel; deployment and CI/CD follow
functional build qualification. No live identity, WireGuard peer, host rule or application was installed
by this code. Permanent host binding, pinned-K3s policy dry runs and real
revocation/restart qualification remain release gates.

The S3 policy is separate from Kubernetes trust. `certificateAuthorityPEM`, when
provided, contains public CA certificates for an owned private S3 endpoint. It is
validated as a nonempty certificate bundle and added as a trust root; HTTPS and
hostname verification remain mandatory. Never derive allowed origins or trust
roots from a signed capability URL. The configured staging endpoint must already
be present in that independent policy.

## Staging and publication

Give `stagingStorage` a dedicated staging bucket/prefix namespace, separate from
retained rollback artifacts. The factory passes it only to the maintained staging
facade and `KubernetesStagingTransfer`. Per-attempt handoffs are fsynced before
source staging/Secret creation; exact source and result keys stay bound to tenant,
project, build and generation. Results are re-downloaded and fully validated
against the expected Job UID and OCI contents. A staging receipt, status file or
Job stdout never establishes success. The dispatcher retains the verified artifact
locally before Core completion and releases capacity only after actual retirement.

Optional `publication` contains a private `stateRoot` and a `projects` map keyed
by project ID. Each entry has its exact `repository` and a private `credentialFile`
for the existing registry username/password model. The repository must equal
`<registry>/customers/<trusted customer>/projects/<trusted project>`. Use scoped
repository credentials, not a registry administrator identity. They never enter a
build Pod or a source/result handoff.

Publication runs in its own task through the existing publication-only helper,
sharing the retained local artifact store. Waiting for Hosting rollout does not
block dispatcher cancellation, log capture or reconciliation. It has a separate
locked outbox, and the same registry/lease guards as the existing worker. No dummy
VM object is constructed to enable publication. Remote retained-artifact backends
are not configured by this factory; staging S3 is not used as a rollback store.

Both loops poll at finite intervals and print only machine error codes. Worker
revocation, upload listener/expiry-task failure, publication-task failure and
SIGINT/SIGTERM all stop dispatching and pass through the same shutdown path.
Before background tasks are dropped, the worker revokes each locally journaled
attempt's transfer authority and cancels its observed exact Job UID/resource
version. Cancellation does not require Core to be available and does not retire
or purge volumes. Durable receipts/outboxes preserve uncertain operations; cleanup
errors are printed as safe codes, and an original terminal failure is retained.
An otherwise clean shutdown reports a cancellation error rather than success.
Stopping does not fabricate Pod absence, retirement or release success. Restart
with the same private state so uncertain operations reconcile before new claims;
build Pod deadlines continue to apply independently.

## Verification

Focused configuration tests cover default-disabled activation, malformed fields,
explicit context selection, foreign endpoints/CA pins, forbidden exec/proxy/admin
certificate paths, private file/directory rules, staging-origin mismatch, directory
overlap, and duplicate-process exclusion. A valid legacy pilot factory is constructed against a
controlled local listener without opening a network connection. Managed identity
is separately tested against controlled authenticated API responses; the same
maintained kube-client instance is observed sending the new bearer after its
real cache interval, without Job deletion or client restart. Forged claims,
wrong identity/audience/TTL/revocation, private-file changes, renewal failure,
monotonic expiry and exact-attempt failure cleanup have negative checks. No test copies a
live kubeconfig, contacts a cluster or changes SSH/host settings. Lifecycle checks
use the real dispatcher journal with a controlled Kubernetes API transport to
verify exact-UID deletion after revocation, no retirement/cleanup while Core is
unavailable, fatal gateway/expiry/publication exits, preserved error/receipt state,
and early `--once` rejection. Run with `--features service,worker,kubernetes-job`
as well to cover the upload-listener feature build.

```sh
cargo test -p webdock-build --no-default-features --features worker,kubernetes-job \
  --lib kubernetes_
```
