# Native managed Mail installation

## Intent

Make the existing Mail product executable work through the native signed Console
installer and generation-based update/rollback. Reuse the generic installer, private
metadata storage, enrolled agent, transport and retained-material contracts. Mail
product decisions remain in the plugin; no product-specific Core installer branch.

The user approved Mail first after the PostgreSQL installer and durable-jobs milestones.
The implementation choices below are the builder's calls under that execution request.

## Binding product boundary

- Stalwart Community Edition: one engine per customer with explicitly activated incoming
  mail, shared across that customer's projects. First `mailboxes.create` is an explicit
  incoming activation; a separate mandatory `incoming.activate` is not introduced.
- Installing/enabling the Mail plugin, creating a customer/project/domain, connecting a
  relay or outgoing-only use allocates no customer engine or mailbox volume.
- Keep metadata database/role/password, sealing key, engine UUID/admin credentials,
  mailbox credentials, workload unit/namespace, PVC and account identity through lifecycle.
- Preserve legacy project-engine quarantine and existing send fail-closed behavior for
  customers with incoming reservations, including failed/suspended ones.
- No public reception, engine-to-Turbo relay, Webmail/outbox completion, real delivery,
  backup, system job or schedule work in this milestone. Live Spitzli/Contabo is untouched.

## Runtime and storage

Mail configuration gains optional `listen`, `core`, `listenTls`, `coreTls`, `storageCa`
with the same strict camelCase shapes as PostgreSQL. Use shared
`plugin_kit::transport::RuntimeTransport` to require exactly one listener and callback.
Existing absolute private Unix configurations continue working.

Add Mail `Store::connect_tls(url, ca)` through `plugin_kit::storage_tls`; enforce
loopback inside plaintext `Store::connect`, not only in the executable. TLS verifies
the driver's actual hostname and CA with no downgrade. No new dependency is needed.
Keep all three physical metadata migrations unchanged; logical schema remains1/readable[1,1].

Use the same authenticated Host client for invocation checks and Workloads. Startup
errors emit only `mail_plugin_start_failed`, not arbitrary DSNs/provider diagnostics.
Preserve private-file ownership/mode checks and loopback-only SMTP/dev-domain trust.

## Candidate reconciliation fence

Current Mail startup calls `resume()` before listening, which schedules retained customer
engine work. That work may contact Stalwart before its eventual Host workload apply.
The replacement must remain incapable of customer reconciliation before promotion.

- Start Describe/Health listening independently of retained reconciliation.
  This includes initial callback unavailability; do not require an initial network
  connection before binding discovery. Static invalid trust/private-file configuration
  still fails closed. Existing eager shared transport APIs remain compatible for other products.
- Add `HostClient::authorize_runtime(plugin, generation)` as a bounded wrapper over the
  existing authenticated `AuthorizePeer` RPC; no new RPC, permission or fake-unit probe.
  Reuse it in `RemoteAuthority` where that avoids duplicate callback logic.
- A cancellable startup task waits with bounded backoff for ordinary current-runtime
  authority, then calls retained `Plugin::resume`. A pre-promotion denial or temporary
  callback failure must not write engine/mailbox states, rotate credentials, issue JMAP
  requests or mutate workloads. Listener discovery remains available to the pinned Core.
- Recheck authority before subsequent startup retries. The shutdown signal must cancel
  the waiting authorization/backoff/resume immediately, independently of graceful server drain.
  Replacement startup retries durable saved state. Do not add a fixed startup sleep.
- The plugin's own metadata connection/migrations may initialize before promotion under
  the existing signed logical-schema high-water contract; this is distinct from resuming
  customer engines. Do not weaken Core's candidate Invoke/Workloads denial.
- Existing old-runtime suspension proof (zero replicas and no Pods) remains mandatory.
  New runtime identity/namespace is generation-specific; customer workload identity is not.

The accepted background product intent remains in Mail's retained store. This work does
not convert it into the new session-bound generic job API or invent autonomous job authority.

## Installer contract

Use existing recipe fields `storage:true`, `sealingKey:true`, `routeDomains` and
`privateConfiguration`. The latter holds `platformTransport`, optional `mailboxes` and
optional `trustUnverifiedDomains`; runtime transport/storage/master fields remain reserved.
No browser-provided secrets, endpoints, manifests or Kubernetes objects.

Keep actual separate PostgreSQL cluster-ID checks and read-only retained-storage checks
on update/rollback. Keep one kernel CA/Core leaf trust profile and independent metadata CA.
Catalog/release provenance, manifest/image digests, feature ceilings, revision/idempotency
checks, private endpoints and current-generation bindings remain the generic contracts.
No new native migration is expected; 0042/0043/0044 remain unchanged.

Incoming requires explicitly approved Workloads, offered eligible cluster/capacity,
reviewed route domains, reachable private management, pinned CE image and storage/ingress
prerequisites. Supported features do not imply public mail delivery readiness.

## Acceptance and custody

1. Focused product tests: strict TLS/Unix config, ambiguous/missing transport denial,
   plaintext remote-storage denial, verified TLS storage and retained ciphertext/rows.
2. Actual Mail executable: owned TLS PostgreSQL and authenticated callback/listener,
   right/wrong CA/name/pin/peer cases, private config files and redacted startup failure.
   Stage retained engine/mailbox data and a counted JMAP stand-in; while the runtime is
   a candidate, Describe/Health works and reconciliation has zero customer side effects.
   After ordinary authorization, reconciliation runs automatically. Shutdown cancels wait.
3. Fresh supported native bootstrap, reviewed signed Mail OCI releases, real Console
   install/disable/update/rollback and enrolled agent on an explicitly owned local fixture.
   No direct installation/runtime rows, fabricated customers or shared operator reset.
   Installing and outgoing-only/domain/relay operations prove zero engine allocation.
4. Explicitly create a mailbox for a genuinely created customer/project, observe its
   real CE workload, then verify gen1→gen2→gen3, predecessor Pods absent, retained
   metadata identity/password/sealing key, engine/PVC UID and mailbox credentials. Store
   and read a local mailbox sentinel if the supported protocol allows it; do not send mail.
   Exercise retained suspend/resume, same-customer reuse and foreign-resource denial.
5. Use loopback SMTP dev configuration without real Turbo credentials. No successful
   outgoing delivery is claimed unless a strictly fixture-owned loopback catcher is used.
   Private reception/relay/Webmail limits remain explicit.

Owned platform/metadata PostgreSQL, callback and artifact registry must be separately
labelled. Use an owned callback container on the dev k3s Docker network with minimal
read-only mounts, never host firewall changes. Never print kubeconfigs, key PEMs, DSNs,
passwords, cookies or mail credentials. Record only sanitized IDs/hashes/statuses.
Keep every ownership UID/process identity/image alias before mutation and cleanup.
Restore initially stopped retained k3s and remove only verified owned assets after proof.
Protect both untracked handovers and `.dev-local/demo-owner.json`; no production actions.

Run owned database/cluster acceptance to completion before full browser quality to avoid
fixture topology churn during browser startup. Finish with full quality, fresh independent
review, targeted fixes, cleanup and signed non-main branch integration.
