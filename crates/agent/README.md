# Runtime execution adapters

This crate contains the privileged execution boundary. It uses Bollard for Docker Engine operations and kube for Kubernetes observations/resource ownership checks. Core's public HTTP process and plugin workloads must not receive the Docker socket or kubeconfig. The runnable `webdock-agent` daemon composes these adapters with `webdock-supervisor::Executor` and a live Core `Authorizer`; no permissive local authorization mode exists.

## Kubernetes and Helm

`KubernetesAdapter::connect` requires an absolute kubeconfig path, an explicit named context, an absolute Helm executable and an `ApprovedChart`. It never loads the default context, accepts plaintext/unverified Kubernetes TLS, or runs kubeconfig executable credential helpers. The chart's reviewed SHA-256 tree digest must match both at construction and immediately before use. Symlinks and oversized charts are rejected.

The adapter verifies owned namespace/release resources and generation/revision annotations, then invokes fixed Helm `upgrade --install` arguments with generated typed values. Hooks and CRDs are disabled; no shell or plugin-supplied chart is accepted. Helm owns release specifications. Kubernetes controllers own Pod convergence and restarts; the agent does not manually restart Pods. Stop upgrades to zero replicas, retaining PVCs and the namespace. Release failures and interrupted Helm work require reconciliation; the adapter does not report them as success.

The bundled chart uses a dedicated namespace/service account, disables service-account token mounts, enforces non-root execution, dropped capabilities, read-only root filesystems, seccomp, resource requests/limits, internal ClusterIP service, retained PVC annotations and restricted NetworkPolicy. The cluster must enforce NetworkPolicy. Core's namespace and workload must carry the chart's documented `webdock.dev/role=control-plane` and `webdock.dev/component=control-plane` labels. Provider HTTP access is intended through Core's scoped broker on port 443; the policy does not claim URL/FQDN filtering.

The adapter reports Deployment availability/observed generation, confirms owned Pod readiness/termination, and returns bounded Pod logs. The separate retained-layout path verifies original UID/owner labels, keeps wd-<projectID>/app-<appID> names, preserves PVC/PV/local paths and secret references, and updates admitted namespace quotas; it does not Helm-adopt or duplicate existing resources. New retained-layout allocation uses the optional verified storage helper and the original namespace/PV/PVC layout. Byte-size changes remain unsupported, matching the existing fixed-allocation contract. Public routing and unverified hard byte quotas are not advertised. Retained volumes require an explicit requested size; the cluster's configured default StorageClass supplies them. Storage-class admission, placement policy and workload mTLS credentials still need production integration.

## Docker

`DockerAdapter::connect` accepts an explicit local Unix Engine socket and installation identity. It ignores Docker context/DOCKER_HOST defaults. The adapter uses immutable image digests, deterministic names and ownership labels, one internal bridge per workload, bounded CPU/memory/PIDs/log rotation, non-root UID 65532, dropped capabilities, no-new-privileges, a read-only root and bounded `/tmp` tmpfs. It publishes no host ports, mounts no Engine socket, and delegates crash restart to Docker's restart policy. The only optional host binds are resource-scoped private socket directories and read-only secret files admitted by the reviewed mount policy below.

Reconciliation inspects existing ownership, generation and effective specification before changes. Retries reuse matching containers; upgrades replace the owned container while retaining its named volume. Stop preserves containers, networks and storage. Explicit Retire removes the owned runtime while retaining data. PurgeStorage requires an absent workload and verifies exact storage ownership and all volume consumers before removal.

Writable retained volumes require a separately approved, digest-pinned initializer image containing `/bin/chown`. A short-lived isolated helper, with only the specific named volume and CHOWN capability, sets its root directory to UID/GID 65532. It has no network or host socket, and runs fixed arguments without a shell. This permits non-root workloads to write `/data` without granting them elevated privileges. Volume contents are not recursively changed.

Docker exposes bounded logs, actual CPU-time/memory snapshots and optional HTTP health checks to the owned container's private IP. It does not advertise multi-replica placement or public routing. Ordinary Docker local volumes have no byte quotas. Optional `runtime.storageSocket` binds a separately privileged helper to the same rootful Engine node; only that configuration advertises enforced volume quotas and accepts a sized retained volume. Cross-host volume migration is not implied.

A Docker-only installation can build without Kubernetes libraries:

```sh
cargo check -p webdock-agent --no-default-features --features docker
```

## Verification and remaining integration

Normal tests validate journal recovery/fencing, default denial, revocation, constrained Docker configuration, chart integrity and actual Helm rendering/linting into typed Kubernetes objects. The ignored Engine integration test requires an explicit disposable image digest:

```sh
WEBDOCK_DOCKER_TEST_IMAGE=docker.io/library/busybox@sha256:<reviewed-digest> \
  cargo test -p webdock-agent --test docker_lifecycle -- --ignored
```

It only creates randomly named test-owned resources and removes those exact resources after testing. It verifies idempotent reconciliation, health, stop/resume, retained data, resource-limit upgrades, ownership denial, logs and metrics. No default Kubernetes context is used by tests. A separate explicit Kind lifecycle test has verified rollout, suspend/resume, revision/resource changes, approved Secret retention and bounded logs. NetworkPolicy enforcement has not been demonstrated by those lifecycle tests.

The dedicated management protocol and daemon are implemented below. Core enrollment/dispatch/live lease authority and durable receipt replay are integrated and tested against retained Hosting authority. Automated certificate issuance, artifact/grant distribution, public routing/TLS and final deployment packaging remain. Retained legacy hosting resources are a separate migration path; the generic Helm release must not silently replace them.


## Runnable outbound daemon

Build and run:

```sh
cargo build -p webdock-agent --no-default-features --features docker
webdock-agent --config /var/lib/webdock/agent/agent.json validate
webdock-agent --config /var/lib/webdock/agent/agent.json run
```

The configuration, TLS certificate/key/CA files and state files must be owned by the agent UID and mode 0600. State/socket directories must be canonical absolute private directories, mode 0700, with trusted ancestors. The state directory holds the exclusive journal lock, execution receipts and atomically persisted identity/generation/sequence. An example local configuration is:

```json
{
  "version": 1,
  "installation": "example",
  "agentId": "node-one",
  "generation": 1,
  "stateDirectory": "/var/lib/webdock/agent",
  "control": {"kind": "unix", "path": "/var/lib/webdock/control/agent.sock"},
  "runtime": {"kind": "docker", "socket": "/var/run/docker.sock", "initializerImage": null}
}
```

A remote control endpoint uses `{"kind":"mtls","origin":"https://core.example:7443","domain":"core.example","ca":"/var/lib/webdock/agent/ca.pem","certificate":"/var/lib/webdock/agent/client.pem","key":"/var/lib/webdock/agent/client.key"}`. Both client and server identities are verified by Tonic/Rustls. No plaintext remote fallback exists. Core must enroll the operator-issued client certificate before startup; automated issuance is not implemented here.

The daemon uses the independent `webdock-agent-protocol` contract. It registers, sends durable monotonic heartbeats, polls typed work, rechecks Core's live Guard before/during execution, and delivers committed receipts. Connection loss drops pending work, retains resources and receipts, and reconnects with bounded 1–30 second backoff. An exact receipt is atomically persisted to a private pending outbox before Complete. A lost completion acknowledgment is recovered after authenticated registration. While Hosting receipt delivery remains unavailable, the daemon retains exact custody and holds new Hosting polling, while heartbeats and other domain dispatch continue. Database and Mail callbacks have independent private outboxes and the same separation. Live enrollment revocation remains fatal. Delivery replay does not execute work or relax terminal operation guards. The outbox is cleared only after acknowledgment; changed receipts cannot overwrite it. Unacknowledged log/result bytes remain in that mode-0600 file until delivery. Expected prior container IDs prevent unapproved replacement; an interrupted replacement can resume only when the resulting owned container binds the exact operation/spec and an allowed generation.

Atomically replace approved certificate/key files to rotate credentials. Changes invalidate the current session and trigger an authenticated re-registration without resetting sequence. Invalid or expired client certificates prevent execution even on an existing channel. A live enrollment revocation persists locally and exits without deleting workloads. Re-enrollment requires an explicitly approved new certificate or higher enrollment generation; changing a lower generation is rejected. Core must also recheck peer certificate validity/revocation on established connections.

An outstanding Hosting receipt whose original operation loses authority can still hold subsequent
Hosting dispatch after reconciliation, because it has no accepted receipt hash to replay. Safe
stale-custody acknowledgement for that case remains a separate gate. The independent Mail/database
queues and heartbeats can continue; this does not claim complete Hosting recovery for that case.

## Incoming Mail execution

`mailRuntime` optionally names an owned mode-0600 JSON file containing
`{"image":"<reviewed restricted repository@sha256:digest>","installation":"<same agent installation>"}`.
For the current local Docker implementation, the main runtime must use an explicit Unix Engine
socket. The image must be the restricted artifact built from `infra/mail/Dockerfile`, not the upstream
image with its file capability. Startup inspects the installed image before advertising Mail support.
Configuration is fixed for the process; replace it through an explicitly reviewed daemon restart.
Kubernetes incoming Mail execution and mailbox configuration execution remain separate open gates;
the daemon rejects an unsupported configured Mail runtime instead of silently ignoring it.

Mail RPCs share the same enrolled Unix/mTLS channel and live cluster generation as Hosting.
An absent or disabled Hosting module supplies no new Hosting dispatch and does not prevent Mail
heartbeats or work. Mail callbacks have their own durable private outbox. Pending delivery is retained
without rerunning mutations and cannot block another domain's polling. Enrollment revocation still
stops all work.

Core allocates and encrypts bootstrap and permanent credentials once. The agent receives them only
as ephemeral claim material; checkpoints and receipts contain no passwords. The initial checkpoint
confirms the deterministic account/configuration write under recovery. The agent then removes recovery
mode, verifies permanent authentication and Community Edition, and compares the observed submission
TLS certificate with the durable configured certificate. A replayed accepted checkpoint resumes this
sequence without generating another password. Each subsequent Ensure also checkpoints its verified
permanent identity for that operation. Exact receipt replay after acknowledgement loss does not restart
the engine.

The Docker runtime has one private internal network and owned named volume per customer, no published
ports, a non-root user, dropped capabilities, a read-only root, bounded memory/CPU/PIDs/logs and high
listener ports. Suspension retains the container, volume, data, credentials and certificate. It never
deletes an instance implicitly. Native ownership checks bind installation/customer/cluster/profile and
credential fingerprint; an explicitly re-enrolled higher generation can operate the same retained
native resources under a new live Core lease. Legacy/foreign resources are not silently adopted.
Ordinary Docker named volumes do not provide a hard filesystem byte quota. This remains an explicitly
owned local integration backend, not the selected production managed-Mail placement.

The real adapter acceptance is opt-in:

```sh
WEBDOCK_MAIL_RUNTIME_DOCKER_TEST=1 \
WEBDOCK_MAIL_RUNTIME_IMAGE=<reviewed-restricted-repository@sha256:digest> \
cargo test -p webdock-agent --test mail_runtime --locked -- --ignored
```

The separate `webdock-agent-control` test `native_mail` exercises the real restricted database,
enrolled daemon and engine together, including lost checkpoint/completion acknowledgements,
retained suspend/resume and enrollment revocation/re-enrollment. It requires the verified local
database fixture, `WEBDOCK_MAIL_NATIVE_DOCKER_TEST=1` and the same explicit image variable.

## Reviewed plugin artifacts and mounts

Optional `RuntimeSpec.plugin` binds the exact reviewed descriptor SHA-256, plugin ID/version and runtime generation to its immutable OCI image. The policy loads private `<sha256>.json` descriptor files from its configured artifact directory, verifies original bytes and the full manifest, and rejects unapproved images or unsupported lifecycle declarations. Plugin identity/manifest digest are supplied as non-secret runtime metadata. Core must still verify the runtime's Describe/Health contract before activation.

`RuntimeSpec.mounts` contains only `{name,revision}` references. Private policy grants bind each reference to installation/project/resource, kind and, for secrets, content SHA-256. No command supplies a host path, target path, arbitrary Kubernetes Secret or egress destination. Local paths are derived exclusively as `<mountRoot>/<installation>/<projectID>/<resourceID>/secrets/<name>` or `.../sockets/<name>`. All directories and entries require verified ownership and private permissions; symlinks and foreign ownership are rejected. Secrets are read-only files under `/run/secrets/<name>`; socket directories are mounted under `/run/webdock/<name>`. Run the local socket peers at the configured non-root runtime UID. Place approved writable socket directories on bounded storage; Docker local directory byte quotas are not advertised.

A plugin secret grant must also appear in the reviewed descriptor's declared secrets with `plugin.secrets.read`. Grant revision changes require new desired state and policy approval; restart the daemon to load changed artifact/grant policy. The default policy denies all mounts and unreviewed plugin bindings.

On Kubernetes, socket directory grants are rejected; use network mTLS. A secret reference resolves only to `wd-secret-<resourceID>-<name>` inside the owned namespace. The agent checks owner labels, the single `value` entry and exact content digest before Helm references it. Pod mounts are read-only. Provider networking remains confined to the approved Core broker; no command can enable arbitrary external egress.


Heartbeat observations come from actual Engine host information or Ready Kubernetes nodes. They retain the existing bounded node/version/architecture/CPU/memory shape and identify the runtime kind. As in the original observer, provisionable capacity budgets and unverified isolation/storage/ingress capability flags stay zero/false until separate operator verification. An empty observation payload means connectivity-only; Core must not treat it as refreshed capacity evidence. Observation failure is not an empty inventory.


## Fixed app storage helper

`webdock-storage --config /absolute/private.json initialize|restore|serve --socket /absolute/private/storage.sock` ports the existing app-storage pool format and root-only Unix RPC (`ensure`, `retained`, `purge`, `capability`); `info` is a bounded configuration discovery extension. Config fields remain `root`, `node`, `clusterID`, `poolID` (UUID), `capacityBytes`, and `reserveBytes`. Create an empty root-owned mode-0700 pool directory and mode-0600 config before the explicit one-time `initialize`. Never initialize an existing pool. `serve` restores recorded ready mounts and holds an exclusive pool lock. The example unit in `deploy/webdock-app-storage.service` uses a separate service/socket; it does not replace the existing combined app/database helper.

Allocation reserves metadata before allocating an ext4 image with an immutable filesystem UUID and fixed byte size. The helper verifies image size/UUID, mount type, exact loop backing file and pool ownership, mounts `nodev,nosuid,noexec`, and creates `/data` with UID/GID 65532. A missing retained image is an error, never an empty replacement. Stop retains storage. Explicit helper purge commits a tombstone and checks that no loop device remains associated before unlinking. Online resizing is intentionally denied, as in the original app-storage contract. Interrupted initialization/purge requires reconciliation; unverified or failed mounts never establish storage readiness.

The helper requires Linux, root, ext4/loop support and trusted system `mount`, `umount`, `findmnt`, `losetup`, `blkid`, `mkfs.ext4`, and `sync` executables at the fixed distro paths in `storage.rs`. Run it in the Engine/kubelet host mount namespace. Its private socket accepts only UID 0 peers; Core receives neither the socket nor cluster/Engine credentials. The agent passes only fixed app/project/cluster IDs, bytes, revision and lease expiry. Live Core guard cancellation closes the helper connection and cancels further helper steps. Runtime commands still default-deny without a Core authorizer.

Docker's optional `storageSocket` verifies a rootful, non-userns Engine on the configured helper node. It creates an owned named `local` volume whose bind source is derived from the helper's verified pool, and verifies pool ID, bytes, owner labels, driver and exact options before reuse. No workload command supplies a host path. This path requires runtime UID 65532; other UID mappings fail closed. The default unsized named-volume path remains available with its reviewed initializer. The helper does not implement cross-host migration.

Kubernetes' optional `storageSocket` defaults to requiring exactly one matching cluster node. A trusted agent configuration may additionally set `runtime.storageNode` to select one local storage node in a cluster of at most 200 nodes. The helper's configured node must equal that selection, the complete bounded Kubernetes list must contain it exactly once, and both its `status.nodeInfo.machineID` and `systemUUID` must match the agent host's `/etc/machine-id` and `/sys/class/dmi/id/product_uuid`. Machine IDs must be nonzero 32-character hexadecimal values; system UUIDs must be valid and nonzero. Every listed node must have a system UUID and those UUIDs must be unique (comparison ignores UUID letter case), so cloned machine IDs alone cannot authorize another host. The selected node must uniquely own the matching `kubernetes.io/hostname` label used by retained PV affinity. Missing or changed identity fails closed; the agent must run on the selected storage host. Observations recheck that binding and advertise only the verified helper pool capacity, without summing nodes or allocating on other hosts. Omitting `storageNode` preserves the original single-node guard.

Native retained-layout create/update uses the original `wd-<projectID>`, `app-<appID>`, `app-<appID>-data` and `wd-<appID>-data` identities, `Retain` local PVs, admitted namespace quotas and restricted namespace policies. Existing resources retain UID/resourceVersion fencing and secret references. An initial exact-command retry may reuse the newly created Deployment; a different operation without its expected UID cannot adopt it. Existing storage is validated before helper calls. Generic Helm StorageClass requests still do not prove hard byte limits, so generic Kubernetes quota capability remains false. NetworkPolicy manifest creation is not proof of enforcement.

The retained Python app storage service supports the native client's read-only `info` and `state` RPCs without replacing its combined database RPC routing. `info` accepts only null and reads the existing configuration/pool identity. `state` requires the existing fenced app packet and verifies owner, exact size, image UUID, mount/backing image and UID/GID 65532 data directory for a ready allocation; a purged allocation must retain its empty directory. Missing, initializing or purging allocations fail closed. These reads never allocate, mount, repair or rewrite retained storage.

The additive app `state` RPC inspects an existing allocation without mounting, repairing, deleting or creating resources. It requires exact app/project/cluster/byte binding and either the original ready filesystem UUID, live mount and data owner, or a purged record with an empty allocation directory. Missing records, interrupted states, unmounted filesystems and recreated files remain unavailable. Upgrade the helper before enabling renewed-generation lifecycle verification on agents; older helpers reject `state`, and agents never fall back to the weaker `retained` check.

The default helper serves apps only; database storage requires the separate mode below. Native Hosting maps persisted app authority, quota, expected UID and retained storage proof to typed Apply/Stop/Retire/PurgeStorage commands. Secret issuance, service-binding changes and database provisioning are separate domain operations, never arbitrary helper commands.

Verification includes an ignored root-only fixture test that fills an actual 64 MiB ext4 filesystem to `ENOSPC`, remounts retained content, rejects ownership/resize/missing-image replacement, and verifies purge idempotence. `tests/fixtures/Dockerfile.storage` builds a disposable Ubuntu fixture with filesystem tools and a nested Engine. The Docker acceptance test uses only that nested Engine's `/run/webdock-owned-docker.sock`, with no host socket or pool mount, and verifies container writes fail at the byte limit while stop/resume retains data. Run only in an explicitly owned privileged disposable container with private cgroup namespace/delegation; never change host cgroup settings or run global loop cleanup. The Kind-node allocation test likewise requires explicit disposable-node/config/context environment variables and cleans only its exact namespace, PV and pool. Default test runs skip these privileged integration fixtures.

## Database filesystem helper

`webdock-storage --database --config /absolute/private-database-pool.json initialize|restore|serve --socket /absolute/private/database.sock` uses a separately configured pool and private socket. The example `deploy/webdock-database-storage.service` never shares the app pool. A durable `_pool/service-kind.json` marker prevents opening the same pool in the other mode; the existing `_pool/state.json` and volume records remain unchanged. On first opening a retained pool, ready filesystems must already have the correct data owner. The helper never repairs a foreign UID or replaces missing retained data.

The retained version-1 `database.ensure`, `database.state`, `database.retained`, `database.purge` and `database.capability` RPCs are implemented. Each mutation packet binds installation ID, cluster, node, pool UUID, revision, fixed bytes and an unexpired lease no longer than 300 seconds. The physical installation ID owns the volume; a logical customer database never receives its own PV. New data directories receive UID/GID 26 before the ready record commits. PVs retain `wd-db-<installationID>`, namespace `wd-db-<installationID>`, claim `postgres-1`, `Retain`, exact verified local path and node affinity. Capability reports actual allocations, free space after reserve and verified mount/owner readiness. The original 4 KiB request/8 KiB response bounds remain; an oversized inventory fails closed rather than silently dropping allocations.

The root-only disposable `database_storage` test verifies private RPC separation, pool-mode fencing, data owner, allocation inventory, immutable pool/size, changed UID denial, cold remount/content identity and idempotent purge. This does not by itself complete database provisioning, migration or backup execution; the domain executor and its separate authenticated dispatch are being integrated.

## Hosted environment and registry material

Hosted environment references bind the exact persisted randomized ciphertext and revision. Core decrypts only the leased operation snapshot and sends ephemeral material over verified transport; command/journal serialization omits plaintext. Docker replacement and retained Kubernetes immutable revision Secrets apply rotations and explicit empty clears, including desired replicas zero. Real Docker and Kind tests cover rotation, clear, paused create/update and other scoped Secret preservation; the full Core/daemon/Docker test verifies lost receipts and actual container environment.

Optional private `policy.registryConfig` uses the retained version-1 project/repository policy, read-only credential evidence and EU-storage evidence. Docker obtains only matching project pull credentials from a root-private file; Kubernetes verifies the configured project pull Secret and exact authority. Cached images still require current repository authorization. No ambient Docker credential helpers, host-wide trust changes or insecure TLS fallback are used. Real tests verify private-CA TLS pulls, read-only authentication, foreign-project denial and Kubernetes imagePullSecrets. `registryVersion: 1` requires verified configured credentials; an absent/degraded policy cannot establish readiness.

Native read dispatch keeps logs, inspect and metrics separate from mutations. The daemon checks live authority and expected runtime UID before and after reads. Logs are bounded to 12,000 UTF-8 bytes; Core's receipt journal retains only the result digest, while Hosting redacts current and historic secrets before persisting logs. Durable private agent receipt outboxes replay before new Poll calls after reconnect, including a lost Complete response after the database has committed.

Completed operations renewed under a new lease use command-aware read-only inspection. Docker verifies absent main/init containers and network, no storage consumers, and an owned retained volume or verified purge state. Native Helm verifies an uninstalled retained release, absent workload resources and pods, the bound retained claim/PV or completed purge tombstone with both absent. Retained Kubernetes verifies its original names, ownership, claim/PV binding and physical helper state. Recreated or foreign resources fail closed. Docker Stop verifies the same stopped runtime under the generation fence, reporting the admitted stop intent revision while preserving immutable container labels. None of these verification paths repeats stop, uninstall, purge, SSA, quota changes or storage restoration.

## Disposable enforcement acceptance

The separate owned Kind fixture `runtime-native/fixture.json` uses Calico 3.33.0 with Kind's default CNI disabled. `tests/fixtures/verify-network-policy.py` applies the actual Helm-rendered policy and probes real traffic: labeled Core ingress, Core TCP443 egress and cluster DNS succeed; foreign ingress, wrong-port Core egress and unrelated egress fail. It deletes only its randomly named test namespaces. This is evidence for that fixture/chart combination, not automatic verification of another cluster. Production heartbeat isolation flags remain false without independent operator evidence. CNPG 1.30.1 and cert-manager 1.21.2 prerequisites are installed only in this disposable cluster, with source manifest hashes and observed image digests recorded; their readiness is not proof of a completed database provisioning operation.

Kubernetes metrics use namespace-scoped `metrics.k8s.io` PodMetrics over the authenticated API connection. The agent needs only metrics Pod `get` access in approved namespaces; it never requests `nodes/proxy`. Each response is bounded to 64 KiB and ten seconds. Exact current Pod ownership/UID and creation time are checked before and after collection, and sample timestamps must be fresh and newer than that Pod. CPU is reported as `cpuNanocores` (rate), with `cpuNanoseconds` absent on Kubernetes; Docker retains cumulative CPU counters. Missing/stale samples remain unavailable rather than fabricated zeros.

Retained Kubernetes service bindings use independent binding and credential revisions. `serviceBindings: null`/omission preserves existing bindings; an explicit empty list removes them. Immutable `app-<appID>-services-<revision>` Secrets mount only approved public CAs at `/var/run/webdock/services/<bindingID>/ca.crt`. Egress permits only the selected database namespace/pod selector on TCP5432 or approved single-host mail CIDRs on TCP443. Real Calico tests verify CA rotation, preserved mounts during environment-only changes, explicit clear, permitted backend traffic and denied wrong-port/other-backend traffic. `serviceBindingsVersion: 1` describes this implementation support; isolation/readiness must still be verified separately. Docker binding application remains denied pending its scoped egress implementation.

## Native database dispatch

An optional top-level `databaseRuntime` path in a Kubernetes agent configuration enables the native database executor. It points to an owned mode-0600 `DatabaseRuntimeConfig`; the existing explicit Kubernetes context and verified client are reused. Docker configurations reject this option. Absence leaves database polling disabled.

Database commands, checkpoints and completions use their own typed RPC service on the same enrolled Core channel. The daemon checks the bounded specification hash, enrollment generation, renewable lease and ephemeral credential material before execution. It rechecks live authority during execution. A separate durable outbox replays the exact checkpoint or completion before polling again after reconnect/restart; decrypted runtime credentials never enter the outbox. Authorization lease tokens remain private and require the protected state directory. Dropped checkpoint acknowledgement cannot be replaced by a different error receipt.

Receipt delivery does not require a still-live tenant execution grant. Core verifies the exact previously issued specification and lease under the currently enrolled agent, then durably classifies obsolete checkpoints and completions without installing stale proofs, renewing leases, confirming credentials or starting billing. Reconciliation changes only the matching operation generation and database operation/revision. Successful state transitions still require current grant, capability and an unexpired lease. An unclassifiable envelope stays private and durable; database polling pauses while hosting and heartbeats continue. Delivery retries have a five-second RPC deadline and five-second backoff. Transport revocation still stops the agent.

Database capability observation requires the available CNPG operator, both retained denying admission policies, schedulable healthy nodes and independently verified database storage helpers. Capacity includes unplaced and init-container requests and occupied NodePorts. Unavailable or incomplete observations do not establish capacity. This observer reports placement capacity; the separate database usage-observation RPC remains to be connected.

The owned Kind fixture verifies Apply/Configure/Observe, actual SCRAM authentication over verified direct and pooled TLS, restart, credential rotation, CPU/memory resizing, pause/resume with retained physical identity/data, retained deletion and explicit dedicated purge. Private Unix transport tests verify checkpoint/completion acknowledgement loss, exact restart replay and live denial. The `--domain` fixture creates a fresh uniquely named database with existing restricted roles and exercises native operator/MFA authorization, plan/order admission, enrolled Core dispatch, daemon restart with lost callback acknowledgements and CNPG maintenance through retained deletion/purge. It cleans only that database and its exact runtime resources. Physical-only shared-pool readiness, usage observation, migration execution and Docker service bindings remain open; this does not establish full database parity.

For setup, trust profiles and operator configuration, see the internal [native plugin installation guide](../../docs/operations/native-plugin-installation.md).

Installation-owned plugin runtimes are separately opt-in with `pluginRuntimeEnabled: true`
(Kubernetes only). `PluginRuntimeControl` shares the enrolled outbound connection; an unavailable
optional Core service leaves that connection healthy. Dispatches contain a strict installation
specification and separate fenced lease, never customer application `RuntimeSpec` or arbitrary YAML.
The native profile creates only its derived Namespace, immutable Secret, Deployment, ClusterIP
Service, NetworkPolicy and Traefik TCP route. It stages the six approved UTF8 files into runtime-owned
memory as regular mode0600 files and passes `/run/webdock/runtime/runtime.json` to the artifact.
No metadata PVC or purge is part of this domain: private metadata storage is external and retained.

UID checkpoints and exact receipts use `plugin-runtime-outbox.json` in the protected agent state
directory. Pending delivery blocks new installation dispatch until acknowledged, including across
restart; decrypted file material never enters the outbox. Every Kubernetes action and wait retains
live Core authorization. Convergence proves the owned Deployment and profile, while Core must
separately authenticate candidate Describe/Health before publishing runtime readiness.

Existing initial spec1 fingerprints and resource names are preserved. Update/rollback spec2 uses a
namespace derived from platform, plugin and runtime generation, retaining the fixed resource names
inside it. Each generation has a distinct runtime hostname/SNI and new TLS identity; metadata
database credentials and the installation sealing key remain stable. Rollback runs the retained
release as a new generation, never reviving old runtime authority. See the guide's
[update and rollback flow](../../docs/operations/native-plugin-installation.md#update-and-rollback)
for explicit disable/drain, API and logical schema high-water requirements. This is not SQL undo.

The dedicated wire-compatible `PluginRuntimePollRequest` carries `max_spec_version`; Poll responses
carry `supported_spec_version`. Absent/zero means1. The daemon requests2 but advertises capability2
only after this connection negotiates2, resetting on reconnect so old Core continues seeing1.
Core also requires the current authenticated capability2 observation and request max2 for lifecycle
dispatch, including suspension of an old spec1 source; a stale capability2 observation cannot
authorize a downgraded agent.

Suspension requires the exact retained spec/material and all six owned UIDs before mutation. It
conditionally writes replicas0 using the Deployment UID/resourceVersion and a metadata-only
`webdock.dev/suspension-fence` annotation containing SHA256 of the exact serialized lease. A fresh
lease changes metadata even at replicas0. The agent waits for observed generation and an empty
Pod list, including terminating/unready Pods, under live guards. Missing custody or an unknown
writer outcome blocks progression with resources retained; retry does not authorize adoption or
purge. Stopped generation namespaces remain retained. Compatible code-only signed-OCI/Console
update/rollback acceptance is in progress and is not claimed by the loopback tests below.

The `plugin_runtime` test target uses a loopback Kubernetes API fixture by default. Its ignored
real-cluster test requires explicit `WEBDOCK_PLUGIN_RUNTIME_TEST_KUBECONFIG`,
`WEBDOCK_PLUGIN_RUNTIME_TEST_CONTEXT` and private `WEBDOCK_PLUGIN_RUNTIME_TEST_CLAIM` paths.
Only owned `kind-webdock-rust-*`/`k3d-webdock-rust-*` contexts are accepted by default. The exact
`webdock-dev` context additionally requires `WEBDOCK_PLUGIN_RUNTIME_ALLOW_DEV_K3S=true` and verified
`https://127.0.0.1:56443`. The claim requires a `test-*` platform identity and a fresh derived
namespace; the test removes only that namespace using UID/resource-version preconditions.
