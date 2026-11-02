# Webdock Mail integration lab

This Compose lab is a disposable integration fixture, not a production deployment template. The selected architecture is [one CE instance per email-enabled customer](../../docs/architecture/mail-deployment-decision.md). Tenant activation and the managed-agent foundation are implemented separately. Production ingress, provider routing and Webmail remain in progress; the fixture does not enable any real customer.

## Run

Requires Bun, a local Linux Docker engine, Docker Compose and installed workspace dependencies:

```sh
bun install --frozen-lockfile
bun test -w @webdock/mail
bun run --filter @webdock/mail check
bun run --filter @webdock/mail test:integration
```

The integration command creates a randomly named `webdock-mail-test-*` Compose project, runs the real server tests and removes that project's containers, networks and volumes even when a test fails. Images stay cached. No existing mail service, credential, DNS record or Turbo callback is used. Do not run the test file directly against a persistent deployment.

`compose.test.yml` contains two independent Stalwart installations with separate configuration/data volumes and separate **internal** Docker networks. No ports are published and the instances have no internet egress. The host-side test runner reaches the Linux bridge addresses. Remote Docker engines and Docker Desktop are not supported by this runner.

## Pinned version

- Stalwart `v0.16.25`.
- Image digest `sha256:74e5a7d55303ba525d939c6bf97ed4e010df7521f52d80afc22a815b66bd53f3`.
- Image revision reported by Docker: `3f657330c0f49a015a3a372fb59669b5cccbca6d`.
- Each fixture is limited to 1 CPU and 1 GiB memory. These are lab limits, not a production sizing result.

The credentials in this file/test are deliberately public, disposable fixture credentials. Recovery administration over HTTP and self-signed SMTP TLS are confined to the egress-blocked lab. Production requires verified HTTPS/TLS, generated secret-manager credentials, removal of recovery access, and a separately reviewed network policy. Do not reuse this Compose file as the production template.

## What it checks

- Noninteractive bootstrap through the actual JMAP API and a server restart.
- Explicit `community` edition reported by `/api/account`.
- Creation and readback of independent domains/accounts.
- Wrong-instance administrator/user authentication denial and non-admin management denial.
- Authenticated SMTP over implicit TLS on port 465, local delivery, and JMAP message listing.
- No message appears in the other instance; an unauthorized SMTP envelope sender is rejected.
- Management-client response bounds, credential-bearing redirect refusal, response identity, per-object errors and uncertain write handling.

See [integration evidence](../../docs/architecture/mail-integration-evidence.md) for limits and remaining gates. This lab's lack of external DNS/HTTPS produces expected upstream messages about unavailable WebUI bundle downloads, Pyzor DNS and DNSSEC. Those network integrations are not claimed tested. The lab exercises APIs directly and does not need the downloaded Stalwart admin SPA.

## Operational implications found already

Object IDs are local to an installation: both independent domains/accounts can have the same ID. Application references must include the Webdock instance/customer binding.

Bootstrap creates port 465 submission by default in the pinned release; exposing 587 in Docker alone does not start a listener there. API management calls work through `/jmap` in the tested image.

An HTTP/JMAP success creating a `Tenant` object is not proof that CE enforces Enterprise tenancy. The lab verifies the edition and relies on physical instance/store separation, never that object.

The documented MTA strategy behavior can fall back to direct MX delivery when a named route is absent. Production must therefore verify referenced routes and enforce an independent egress policy allowing only the configured relays. A route-name string alone does not meet Webdock's no-fallback requirement. [Strategies](https://stalw.art/docs/mta/outbound/strategy/)

## Managed cluster profile

`Dockerfile` derives a non-root image from the pinned CE base and removes the
binary file capability, which otherwise prevents execution after dropping all
container capabilities. Build locally with:

```sh
docker build --network none -f infra/mail/Dockerfile -t webdock.local/mail-stalwart:0.16.25-restricted .
```

Production configuration uses the resulting immutable repository digest, never
this mutable build tag. See the [Mail agent setup](../../agents/kubernetes/README.md#native-mail-agent-profile).

## Mail product ticks

`cronjobs.yaml` is a suspended template for the existing control namespace, not
for the isolated customer mailbox namespaces. It calls the four production Auth
job handlers and contains no worker implementation or embedded secret. Confirm
one scheduler and provision `webdock-mail-cron` through the existing secret manager
before enabling anything. See [Mail operations](../../docs/architecture/mail-operations.md).

## Native Rust local lifecycle acceptance

The Rust agent uses the same tracked `infra/mail/Dockerfile` restricted profile.
The original upstream image is a build input, not a compatible runtime artifact:
its file capability and default user/configuration do not satisfy the agent's
non-root, capability-free profile. The existing release builder also produces
`mail-runtime.oci.tar` from this recipe and records its immutable registry digest.

Build with an explicit local Engine (no remote context):

```sh
docker --host unix:///var/run/docker.sock build --network none \
  -f infra/mail/Dockerfile -t webdock.local/mail-stalwart:0.16.25-restricted .
```

Import the reviewed resulting artifact by its actual repository digest into the
same local Engine. Do not use the upstream digest, a mutable build tag, or assume
that a rebuild has the same digest as an earlier artifact. Verify it locally with
`docker --host unix:///var/run/docker.sock image inspect REGISTRY/IMAGE@sha256:DIGEST`.
The acceptance test requires that explicit reviewed digest and never pulls an image:

```sh
WEBDOCK_MAIL_RUNTIME_IMAGE=REGISTRY/IMAGE@sha256:DIGEST \
WEBDOCK_MAIL_RUNTIME_DOCKER_TEST=1 \
cargo test -p webdock-agent --test mail_runtime --no-default-features \
  --features docker -- --ignored
```

The fixture checks actual CE bootstrap, permanent credentials, checkpoint response
loss, removal of recovery access, stored-certificate/observed-TLS equality, denied
foreign ownership and changed profiles, live guard refusal, retained suspension,
and reenrollment with a new authorized generation. A real JMAP draft's subject and
body survive stop/resume. An unallocated suspension creates no storage. All fixture
resources use a unique owner label and no published host ports; cleanup is restricted
to that owner. The image's unused inherited volume paths are bounded tmpfs mounts,
so neither the initializer nor runtime creates anonymous image volumes.

This remains the explicit Linux local test backend. Its named-volume retention is
verified; filesystem byte quotas, public mail routing and production Kubernetes
acceptance are separate gates. Runtime configuration is fixed for each agent process;
restart the agent after changing the private configuration. Every mutation still
requires the current enrolled Core claim/guard.

The bootstrap checkpoint records immutable credential custody and acknowledged
configuration writes; it does **not** mark the service ready. The pinned CE release
rejects normal account authentication while recovery mode is active, including
after restarting that recovery process. The executor therefore removes recovery
only after the checkpoint is acknowledged, then requires permanent-account
management authentication, Community Edition verification and the persisted TLS
certificate proof before returning readiness. A lost checkpoint acknowledgement
leaves recovery and stored data intact.

Allocation retry also inspects the fixed ownership initializer. An interruption
after volume creation or after helper creation can resume that exact owned helper;
foreign labels or a changed helper command/profile are refused. The only permitted
ownership command is non-recursive `chown 65532:65532 /data`: existing mailbox files
are never traversed or recursively reassigned. Every start/removal uses the verified
container ID and a fresh guard; a failed initializer remains for reconciliation.

## Native Rust managed Kubernetes lifecycle

The managed runtime runs in the existing node-installed agent with its local,
quota-backed storage helper. It reuses that agent's explicit verified Kubernetes
connection; Core receives no kubeconfig. The generic Helm observer/BYOK agent is a
separate deployment profile. Gateway Pod connectivity does not establish access
for an arbitrary Pod-hosted management agent.

A private `KubernetesMailConfig` binds the installation, node, restricted image
digest, gateway namespace, and any explicitly reviewed retained resource UIDs.
The runtime rechecks that file and the enrolled live claim before mutations. The
fixed customer profile uses a 10 GiB retained allocation, 500m CPU, 1 GiB memory,
non-root read-only containers, no service-account token, high ports, and default
network denial with scoped DNS/gateway exceptions. An immutable image uses
`IfNotPresent`, allowing a clean reviewed node to pull it. It never falls back to
a mutable tag or a default Kubernetes context.

The native fixture runs inside the explicitly owned disposable Kind node so it
can reach the private ClusterIP through the same node-local topology. Its runner
requires an explicit kubeconfig/context and reviewed Mail/BusyBox digests, verifies
node ownership, imports only those images, and allows only one 10 GiB allocation
at a time. It observes Pod status through the Kubernetes API; it uses no Pod exec,
API proxy, or port-forward endpoint. Cleanup checks exact object UIDs and helper
allocation ownership before removing the fresh namespace, PV, allocation, and
runner directory. It preserves uncertain fixtures instead of pruning resources.

The lifecycle fixture verifies real CE bootstrap, checkpoint response loss,
permanent authentication after recovery removal, stored/observed TLS equality,
exact replay without Pod restart, a retained JMAP message, suspension and enrolled
generation rotation. Accepted checkpoint replay verifies the Deployment UID,
material and stored certificate before skipping bootstrap configuration; it still
requires final normal-mode authentication and TLS before reporting readiness.
The original claim lease is never extended.

Resources retain `webdock` server-side-apply replica ownership and the separate
`webdock-mail-config` submission annotation. Atomic creation uses a separate
creation manager. Unchanged fields retain that custody; constrained force is used
only for intended changes after checking current UID/resourceVersion, native
profile, creation provenance and overlapping foreign field ownership. Recovery
mode is removed with a conditional patch deleting exactly its two fixed environment
entries: omission from Apply cannot delete entries another manager still owns.
The patch neither edits managedFields nor takes replica ownership. RV conflicts
cause bounded re-read and full revalidation.

Legacy resources require a private operator-reviewed namespace/Deployment/Service/
PV/PVC UID binding, plus any retained bootstrap Secret and extra NetworkPolicy
fingerprints. Labels alone do not authorize adoption. Core does not yet persist an
independent expected Deployment UID across completed operations; the current
checkpoint binds it only within the original operation. Consequently this does
not close the broader distributed adoption/recovery gate. Public delivery, relay
configuration execution, production enablement and complete Mail parity remain
separate acceptance gates.

Credentialed management clients are constructed only after inspecting the current
Deployment, its ready Pod/ReplicaSet owner chain, and its fixed ClusterIP Service
profile. An invalid retained workload is rejected before resolving that credential
destination; a suspended retained workload receives no early probe through stale
endpoints. Execution fields omitted from the desired manifest are explicitly
rejected, including command/envFrom/lifecycle hooks, unsupported effective security
settings, mount changes, host ports, DNS/runtime-class changes and resource overrides.
The profile accepts only reviewed Kubernetes defaults and equivalent quantity
encodings. Ordinary Apply cannot erase an unknown field another manager owns.

## Native Rust configuration executor

Configuration is a separate opt-in on the existing enrolled UDS/mTLS channel.
Set the agent's `mailConfigurationEnabled` to `true` only with a reviewed private
`mailRuntime` configuration. It defaults to `false`; enabling it without a Mail
runtime fails validation. Core separately requires
`WEBDOCK_NATIVE_MAIL_CONFIGURATION_ENABLED=true`, the native Mail, Mail agent and
native mailbox flags, and the Mail feature gate. The optional configuration RPC
and its durable callback outbox do not block Hosting, lifecycle execution or
heartbeats when configuration is disabled or an older Core lacks that RPC.

The executor inspects an already normal owned instance, its current runtime ID,
fixed profile and stored/observed TLS pin before using management credentials.
It never runs lifecycle Ensure/bootstrap. Bound, per-resource Handoff and Applied
callbacks record custody before external mutations and after readback. The agent
journal contains exact nonsecret callbacks, not administrator or identity passwords.
A missing acknowledgement replays the same callback. An uncertain create can be
resolved only through its recorded handoff and exact ownership marker; an absent
or ambiguous resource is not blindly recreated. Existing identity credentials and
mail data are retained.

Apply configures domains, Group mailboxes, RSA DKIM and restricted User identities.
Interactive identity permissions exclude sending. Restrict uses only known provider
references, contracts actual memberships without adding groups, preserves current
permissions while memberships remain, and disables authentication after the last
membership is removed. It never configures domains/mailboxes, relay routes,
submission hooks, provider secrets or public networking. Core must serialize live
configuration work per customer and revalidate current authority; membership
intersection alone is not a concurrency mechanism for arbitrary external editors.

Runtime evidence is recorded in [configuration-runtime-evidence.json](configuration-runtime-evidence.json).
The actual local Docker test proves configuration checkpoint-loss recovery, partial
and full restriction, unchanged credentials and unchanged DKIM public keys. The
separate node-installed Kubernetes case (`run-kind-mail.py --case configuration`)
proves initial validated DKIM readback, Group/User access and restriction, callback
loss recovery, unchanged Deployment/Pod/PVC identities, an existing retained draft,
and refusal to configure a suspended instance. That Kind case does **not** compare
DKIM keys again after restriction; this assertion belongs to the Docker test.

The Kind runner selects one case explicitly, rechecks the known disposable cluster,
uses a single 10 GiB allocation, and deletes only its verified fixture resources.
Direct Core → enrolled daemon → Kubernetes configuration composition remains open.
These runtime checks do not establish public delivery, submission readiness,
production activation or complete Mail parity.
