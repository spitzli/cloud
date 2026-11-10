# Webdock managed hosting — implementation and live verification, 2026-10-08

## 2026-10-09: persistent storage live and Lunares deployed

The later explicit deployment request supersedes the local-only storage status below. The root-only storage helper and sandboxed agent are live on Contabo; verified platform capacity includes a 20 GB fixed-filesystem pool. Auth/Studio were built locally and deployed prebuilt with READY/fra1 Functions. Lunares app `102047516467400704` has 1 GB `/data`, the supplied SQLite snapshot and its signed API at `https://api.schattenclan.de`. Its existing panel remains on Vercel at `https://admin.schattenclan.de`. No Discord token was installed; API standby is healthy. Exact proof: `docs/verification/2026-10-09-lunares-production.json`. Networking/TLS setup is targeted to this bot, not generic selfservice capability.

## 2026-10-09: persistent storage prepared locally

Fixed-size `/data` filesystems, retained-data accounting and separate administrator purge are implemented and locally verified across Studio/API/MCP, PostgreSQL, the sandboxed executor and a private root storage helper. They are **not yet deployed/activated on Contabo**; production remains at the environment-variable milestone. See [storage architecture and rollout boundary](persistent-app-storage.md). No production allowance or Lunares data changed.

## 2026-10-09: tenant-owned infrastructure and simpler hosting UI

This increment is deployed. Operator customer hosting settings now enable own Kubernetes/k3s clusters and own Vercel teams independently. Kubernetes registration count, approved namespaces and existing-application management are enforced in Auth. Defaults remain disabled; no production customer's entitlement was changed automatically.

Customer-owned clusters use a portable outbound Python/kubectl agent and an explicit installation bundle at `/hosting-agent.zip`. Local kubeconfig stays with the customer. A scoped RBAC generator is included; renewing authentication remains the cluster owner's responsibility. The agent publishes bounded allowlisted inventories and optional CPU/RAM metrics. Existing Deployment/StatefulSet scale, stop/start, restart and logs; DaemonSet restart/logs; CronJob suspend/resume; Job/Pod inspection/logs use UID/version-fenced commands. They do not adopt namespaces or apply the Webdock-managed safety baseline to pre-existing workloads. Provider/GitOps conflicts, paused rollouts and scale-down that would delete StatefulSet PVCs are blocked. Uncertain writes require operator reconciliation against fresh inventory; queued intent expires after three minutes.

New REST commands use authenticated `POST /api/hosting/commands`; MCP tools use the corresponding `byok_*` names through the existing scoped hosting server. Enrollment secrets are available only in the owning authorized Studio session. Credentials, kubeconfig, arbitrary YAML and shell commands are never MCP command arguments.

Vercel reuses the existing Studio integration configuration and callback, with separate tenant-bound encrypted connection records and selected project IDs. The existing confidential Studio→Auth channel passes the runtime integration configuration only for OAuth begin/finish, without adding it to public command schemas, responses or persistent Auth configuration. State binds the customer and current session; a matching HttpOnly nonce routes the callback. Platform-team reuse and cross-tenant team ownership are rejected. Project/deployment/domain read scopes are required; cancel and Frankfurt-default actions depend on the installation's write scopes. Provider billing stays with the customer. Redeploy/build actions and spend enforcement are not offered; no non-EU remote build is started. The platform project-created webhook remains platform-team-scoped.

Studio and Auth use exact shared conversions behind readable CPU cores and MB/GB/TB fields. Switching units preserves stored values, including legacy non-round values and German decimals. Applications come first; resource scans, advanced settings and mobile table layouts are simplified. Project/customer/cluster assignments use names rather than requiring copied IDs.

Verification: 83 Auth, 95 Studio, 8 contracts/resource UI and 18 Python tests passed; both typechecks and local production builds passed. A local disposable Kubernetes cluster completed scan, scale, stop/start, restart and logs on an existing application, including a real browser-triggered action and persistent daemon HTTP flow. Production operator pages were rendered and checked through the real session; final deployments and bundle download were independently inspected. The first real customer-owned Vercel OAuth installation is still unverified and requires that customer's authorization. No customer resources were migrated or changed as test fixtures.

The existing Contabo daemon had stopped after a 401/403 response before this release. Its unchanged credential was accepted during diagnosis. It was restarted and now stops permanently only on Webdock's explicit credential-rejection response; generic proxy/provider failures and temporarily disabled BYOK policy are retried. The original rejection body was unavailable, so its exact source is not claimed. Its previous daemon source was backed up under `/root/setup-backup/daemon-before-byok-20261009.py`.

Revoked registrations continue to count toward the configured cluster maximum; there is no connection-archive UI in this increment. External cluster quotas cannot be guaranteed against changes made outside Webdock. Existing persistent-storage, registry/build, domains/TLS, backup and OS/k3s-maintenance limits below still apply to Webdock-hosted apps.

Evidence: [BYOK verification](../verification/2026-10-09-byok-hosting.json).

## Current state: active managed execution

The owner explicitly corrected the read-only stopping point. **Active application management is now implemented and tested on the real Contabo node.** Studio/Auth are now deployed to production on Vercel with verified `fra1` Functions. The permanent node-local agent is enabled and active; the local preview and SSH bridge have been stopped. Production cluster: `101775802667896832`. See [production verification](../verification/2026-10-08-hosting-production.json).

The operator can enable finite cluster capacity with a documented security verification, assign a customer project, create an HTTP application, change its configuration, scale it, stop/start/restart, restore the previous configuration, read bounded logs and permanently remove the application with a fresh deletion preview and exact-name confirmation. These actions are available through Studio, REST and MCP and execute through the same authorization/quota/journal service. Selfservice uses the project mode, active membership and explicit image policy; platform-only actions and deletion remain operator-only. Customer preview cannot enqueue operations, including log retrieval.

### Real execution path

`webdock-hosting-agent.service` runs `daemon.py` permanently on Contabo. It retrieves typed, leased Webdock jobs over HTTPS from production Auth and invokes the fixed local `executor.py` implementation. Root-only credentials live at `/var/lib/webdock-hosting-agent/connection.json`; installed code is under `/opt/webdock-hosting-agent/`. It never accepts arbitrary manifests or host shell commands from a tenant. The Kubernetes API remains private. The service is enabled at boot, retries transient failures and does not depend on the workstation or SSH bridge. Enrollment used the real production platform administrator session.

The executor creates **new owned project namespaces** with Pod Security, dynamically assigned quota, default-deny networks, project-internal communication, DNS and controlled Traefik access. It does not adopt or overwrite the pre-existing default/system/security-check namespaces. An empty, equally restricted `wdv-<projectID>` namespace validates the exact Pod server-side without charging a speculative extra Pod against a full running application quota. It also dry-runs the Service/Deployment before applying.

Applications have a read-only root filesystem, non-root identity, no API token, positive CPU/RAM/ephemeral requests equal to limits, explicit health probes and a bounded temporary `/tmp` volume. Recreate rollout avoids hidden surge allocation. Updates reserve resource deltas while retaining uncertain previous allocations; completion releases reservations only after the agent has observed the requested state. Prior successful configurations are stored separately from failed desired configurations for rollback.

Deletion is invoked only through the authorized Webdock service. The executor checks namespace/resource ownership and sends Kubernetes DeleteOptions with UID/resourceVersion preconditions; then verifies Deployment, Service and Pods are absent before reporting completion. Project namespaces are retained as stated in the preview. Their application resource quota is reduced to the remaining allocation, including zero after the last app. A failed/expired write retains allocation and requires operator reconciliation; failed log reads do not deadlock project changes, and diagnostic reads remain possible after a failed deployment.

### Real-node lifecycle result (local control plane, before production rollout)

[Machine-readable verification](../verification/2026-10-08-managed-hosting.json) records the real operation chain. The test application `101749052365869056` in project `101748680347877376` was:

1. Created on Contabo and observed ready.
2. Scaled from one to two replicas.
3. Stopped, started, restarted and rolled back through Webdock.
4. Read through the bounded log operation.
5. Updated from 100 to 125 millicores through the actual Studio form, reaching revision 7/7.
6. Reached from Traefik at its internal service `/ping`, returning `OK`.
7. Deleted through the actual Studio confirmation form, reaching deleted revision 8/8.

After deletion, direct read-only inspection found no Deployment, Pod or Service in the project namespace; application CPU/RAM/ephemeral hard quotas were zero and no reservation for the app remained. `wd-101748680347877376` and `wdv-101748680347877376` remain as managed empty namespaces. No customer application was migrated, no existing namespace baseline was overwritten, no new image was pulled and no public ingress was created.

### Supported profile and remaining work

This is active management of **HTTP containers using already cached immutable images and temporary storage**. Custom containers require operator access or explicit per-project own-image permission. On shared clusters, platform operators must additionally confirm `confirmSharedImages` when granting/updating this permission; it does not enable images for other projects. Dedicated clusters remain restricted to their assigned customer. The approved HTTP healthcheck template uses the already-present immutable Traefik image. Cached-only image use avoids silently introducing unverified/non-EU registry traffic.

Persistent volumes, verified EU image registry/build pipelines, public domain/TLS provisioning, fine-grained external egress rules, backup/restore and OS/k3s maintenance are not supplied by this execution increment. Local Path still has no hard tenant written-byte quota; persistent allocation is therefore disabled. Vercel provider metering/enforcement remains separate; the new Webdock interface itself is deployed. Do not call the whole hosting product production-ready solely because its active lifecycle works.

### Added REST/MCP operations

- `POST /api/hosting/clusters/{id}/activate` / `activate_hosting_cluster`.
- `GET/POST /api/hosting/projects/{id}/apps` / `list_hosting_apps`, `create_hosting_app`.
- `GET/PATCH /api/hosting/apps/{id}` / `get_hosting_app`, `update_hosting_app`.
- `POST /api/hosting/apps/{id}/{scale,start,stop,restart,rollback,logs,reconcile}` with corresponding `*_hosting_app` tools; logs use `get_hosting_logs`.
- `GET /api/hosting/apps/{id}/deletion`, `DELETE /api/hosting/apps/{id}` / `preview_hosting_app_deletion`, `delete_hosting_app`.

Async requests return a durable operation ID. Observe it with `get_hosting_operation`, then read the application to inspect desired/observed revision and cached logs. Readiness is an observed result, not the acceptance of a Deployment manifest.

Verification before production rollout: full Auth suite 80 passed; full Admin suite 95 passed; contracts 4 passed; Python observer/executor/daemon tests 9 passed; both TypeScript checks passed. This includes the limited-runtime-role regression test. Additional tests cover customer selfservice/foreign tenant rejection, preview job denial, diagnostic reads after failed deployment, non-blocking log failures, idempotent app creation and observed-only allocation release. Both production builds completed locally under Node 24 and were uploaded with `--prebuilt`; final READY deployments and `fra1` regions were inspected. No independent reviewer is claimed.

## Login recovery correction

A missing local Studio session previously escaped as a generic page-load error. Hosting page reads now redirect HTTP 401 to native SSO with the original safe internal destination; 403/404 uses not-found, and service outages remain genuine errors. API responses and mutation forms retain their original status/draft behavior. Verified with four regression tests, fresh unauthenticated HTTP reads and the authenticated T3 preview. The localhost environment uses separate local test identities, not production Webdock sessions.

## Historical foundation snapshot (superseded where it says read-only)

The following records the earlier foundation delivery and its connection/security contracts. Its statements about read-only observation or absent execution are superseded by the current section above.

## Delivered state

Implemented locally on `feat/hosting-foundation`; not committed and not deployed. Studio and Auth remain at Vercel by explicit user instruction. No production schema, Vercel configuration, remote server file, customer workload or namespace was changed during this implementation.

The operator Studio now has **Infrastructure**, cluster registration, protected one-time enrollment, agent revocation, and customer hosting pages. Existing package templates, assignments, offers and extras carry hosting allowances. Old packages default to no hosting allowance; legacy edits preserve existing hosting fields. Customer pages show their own project assignment, mode and limits. Operators can assign existing projects to k3s or Vercel inventory and configure global/customer, provider and project caps.

REST `/api/hosting` and hosting tools on the existing `/api/mcp` share the native Auth hosting service. Hosting OAuth clients are a separate selectable capability in Auth Connections. They use `hosting:read` / `hosting:write`; existing Registry clients and `webdock:*` operator checks remain separate. Customer tokens cannot register clusters, change limits, change managed project configuration, see another customer or obtain Registry tools. Customer preview is read-only and restricted to its original, unexpired tenant mapping.

Allocation code uses the same PostgreSQL transaction advisory lock as subscription edits/offer acceptance, plus a cluster lock. Reservations count across projects and clusters. Quantities use exact integer units; IDs remain Snowflake strings. Per-app replica limits are maxima, not customer-wide replica sums. Lowering a cap preserves existing allocations and reports over-allocation. Ambiguous/expired operations retain reservations for reconciliation. No public arbitrary operation enqueue endpoint is present.

## Real Contabo connection

The owner supplied `root@213.136.65.22`. Existing public-key SSH with strict known-host verification worked. Read-only observation confirmed:

- Host/node `vmd208517`, Ready, k3s `v1.36.5+k3s1`, amd64.
- Kubernetes allocatable CPU: 6000 millicores.
- Kubernetes allocatable memory: 12541489152 bytes.
- Local preview cluster ID: `101741004872224768`.
- Location: EU confirmed by owner; exact country/provider region remains unverified. Country is stored as unknown, not guessed Germany.

`apps/hosting-agent/observer.py` reads only node objects and emits allowlisted fields. Node annotations, kubeconfig, tokens and arbitrary command output never enter observations. `connect.py` executes that reader through pinned SSH, then sends heartbeats to the **local** Auth service. It does not install anything on the server or change Kubernetes resources. Connection credentials stay in a private ignored file; HTTP redirects never receive them. The local loop stops on errors and the UI marks observations stale on the next read after 90 seconds.

Local preview URLs:

- Studio: `http://localhost:3120/infrastructure/101741004872224768`
- Auth: `http://localhost:3125`

The local preview uses `webdock_hosting_preview_20261008`, cloned from the explicitly disposable local test database. Test suites continue using `webdock_admin_test`. Both run on the existing local PostgreSQL test container, never the Contabo node. Browser identities and agent credentials are synthetic/local and stored privately under the git-excluded `.superpowers/sdd/2026-10-08-hosting-foundation/` directory.

The earlier `Local browser cluster` record is a clearly named simulated fixture, separate from the real Contabo inventory record. A real observation is **not** a verified provider location, a workload isolation attestation or a deployable capacity calculation. Provisionable capacity remains zero and workload readiness remains unverified.

## Baseline compatibility and honest limits

The authoritative server baseline is the workspace's `infrastructure/k3s/README.md` and its admission/namespace files. It requires Restricted Pod Security, read-only root filesystems, positive CPU/RAM/ephemeral requests and limits, service probes, no mounted API tokens, ClusterIP-only services and default-deny networking. The future executor must prevalidate generated pod templates and surface admission failures from ReplicaSets/Pods even when Deployment creation succeeds.

Do not rerun `prepare-namespace.sh` over allocated customer namespaces: its provisional baseline would overwrite future customer quotas. No current customer namespace was provisioned in this increment.

Local Path PVC size requests do not enforce hard per-customer written-byte limits. Ephemeral limits are measured/eviction-based. Shared-kernel namespaces do not provide VM isolation against hostile code. The initial dedicated-cluster-only rule was superseded on 2026-10-09 by explicit per-project operator approval for shared clusters (`confirmSharedImages`); shared-kernel limitations remain unchanged. A template/image executor, verified writable mounts, healthchecks, per-app egress, ingress/domain ownership and suitable storage still need implementation and verification before customer apps can run.

## Explicit deployment prerequisites

Run `apps/auth/scripts/migrate-hosting.ts` only with the intended Auth migration environment. It creates additive `webdock_auth.hosting_*` tables and extends the persisted Better Auth MCP resource with hosting scopes; config changes alone do not update a previously persisted OAuth resource. No request handler applies migrations. No new cross-schema write privilege is granted to Studio.

The existing Studio resource-server OAuth client must be linked to the MCP resource through the native operator provisioning flow, as for existing MCP introspection. Hosting clients require explicit native registration/consent; no dynamic anonymous client registration was enabled.

Hosting bridge/agent routes fail closed in production unless `SPITZLI_HOSTING_EU_VERIFIED=true`. This is an operator release gate, **not** automatic proof of location. Set it only after the complete operating path is verified. Current Vercel Frankfurt functions do not establish an EU-only control/storage/logging path; the gate has not been enabled or deployed here. The user chose to keep Studio/Auth at Vercel and verify the node locally for now.

Existing applications keep running independently of the hosting control plane. Rollback means disabling the new routes/observer or revoking the agent; do not drop hosting records, delete namespaces or free uncertain reservations.

## API surface in this increment

| Operation | REST | MCP |
| --- | --- | --- |
| Clusters | GET/POST `/api/hosting/clusters`, GET `/{id}` | `list_hosting_clusters`, `register_hosting_cluster`, `get_hosting_cluster` |
| Enrollment reference/revocation | POST `/clusters/{id}/enrollment`, `/revoke` | `create_cluster_enrollment`, `revoke_hosting_agent` |
| Limits | GET/PUT `/customers/{id}/limits` | `get_hosting_limits`, `set_hosting_limits` |
| Usage | GET `/customers/{id}/usage` | `get_hosting_usage` |
| Project assignment | GET `/customers/{id}/projects`, POST/PATCH `/projects/{id}` | `list_hosting_projects`, `create_hosting_project`, `update_hosting_project` |
| Recorded operation | GET `/operations/{id}` | `get_hosting_operation` |

Paths in the final five rows are relative to `/api/hosting`. Requests use bearer authorization; browser server actions use the existing delegated SSO session. Enrollment MCP returns only a protected Studio setup reference. Raw enrollment tokens appear once after an authenticated operator browser action; agent credentials are returned only by successful one-time enrollment and never by inventory/read tools.

Cluster create and hosting-project assignment take an idempotency key. Limits/project/revocation updates use revision comparison. Lists are bounded/paginated and reuse Studio list controls. Provider/project caps only restrict package allocation, never increase it. Scope descriptions and enforcement labels intentionally distinguish Webdock action limits from provider enforcement and measured usage.

Vercel inventory and cap configuration exist, but actual Vercel deployment/traffic/cost metering and hard provider limits are **not implemented** by this foundation. There is no CPU-unit equivalence between Vercel billed execution and reserved k3s CPU. External Dashboard/Git actions are not falsely represented as covered by a Webdock-local budget gate.

## Verification

- Full Auth suite: 77 passed.
- Full Studio/Admin suite: 90 passed.
- Hosting contracts: 3 passed; Python observer/redirect tests: 3 passed.
- Auth and Studio TypeScript checks passed.
- German catalogs extracted, translated and compiled; all 11 i18n tests passed and the generated catalog is current.
- Real PostgreSQL tests: concurrent reservations, cross-cluster customer budget, stale plan revisions, foreign customers, read-only scopes, one-time enrollment, stale/foreign-generation reports, expired/fenced operations, revocation, lower caps and read-only preview.
- Real native OAuth: authorization/consent/PKCE/token/introspection, Registry scope escalation denied, customer membership and session revocation observed.
- Actual local HTTP: customer REST read 200; limit mutation 403; Registry read 401; hosting MCP read 200 with exactly the same result as REST.
- T3 browser: real local operator and separate customer SSO; UI cluster registration, enrollment, project assignment and CPU cap change; customer preview checked and exited; foreign customer returned no data; German customer page visually checked.
- Contabo observation checked via read-only SSH and delivered through the local heartbeat endpoint. No live admission/network tests or provider mutations were rerun.

No production build/deployment is claimed. The approved plan's foundation is the delivered increment; container execution, provisioning, secrets/domains, backup/restore, maintenance and Vercel provider enforcement remain subsequent increments.

See [approved specification](../superpowers/specs/2026-10-08-hosting-control-plane.md) and [foundation plan](../superpowers/plans/2026-10-08-hosting-foundation.md).

## 2026-10-09: customer-owned turboSMTP

Own turboSMTP is an independent, default-disabled tenant entitlement in the existing BYOK policy. Operators enable it and a finite sender-domain allowance under customer Hosting. The tenant's owner/admin enters a Consumer Key/Secret with API permission under Mail. Credentials are verified through the existing bounded sender-domain adapter, encrypted with a tenant-specific purpose, and never returned to UI/API/MCP; duplicate credential fingerprints and reuse of the platform master key are rejected. This detects the same key, not different keys for the same provider account. The UI explicitly confirms account-wide domain visibility and asks for a dedicated customer account.

The connection table is separate from platform-managed subaccounts, domains, keys and tracking. Connecting does not migrate them, change application SMTP settings, or send email. Cached domain status, explicit refresh and registration are available through Studio and the same scoped REST/MCP commands. Credential entry is Studio-only and its command is excluded from MCP tool schemas. Disconnect clears Webdock's stored credential, preserving provider resources. Provider sending-key creation/revocation and billing stay in turboSMTP because those administrative endpoints require a different authorization method.

Domain registration commits a reservation before the external write. An uncertain result blocks blind retries, credential replacement and disconnect; refresh can reconcile an observed domain. Operator review can clear an unresolved registration after five minutes with exact-domain confirmation. Failed mutations refresh the tenant layout so the visible recovery action carries the new revision. Domain snapshots are bounded to 1,000 entries; the UI supports search/pagination.

The owner explicitly approved the separate turboSMTP management API as an exception to the EU-only requirement. SMTP/API sending configuration remains on EU endpoints. This is not a guarantee that the provider's management infrastructure is EU-only, and the Webdock domain allowance is not a provider email/spend cap.

Source contract: https://serversmtp.com/turbo-api/turbo-smtp.yaml (V2). Management: `https://pro.api.serversmtp.com/api/v2`; sending configuration: `pro.eu.turbo-smtp.com`, `https://api.eu.turbo-smtp.com/api/v2/mail/send`.

Verification: 85 Auth + 95 Studio + 8 shared tests passed; typechecks, gettext and local production builds passed. Production Mail and customer-permission pages were rendered with a real operator session. No actual customer-owned turboSMTP credential was supplied, so real provider connection verification remains open. No email was sent. Evidence: [turboSMTP BYOK](../verification/2026-10-09-turbosmtp-byok.json).

## Application environment variables — live 2026-10-09

App creation and update accept optional `environment` patches: `[{"name":"EXAMPLE","value":"sample"}]` sets/replaces a value, `value:null` removes the named variable, omission preserves all existing values. Empty strings are valid values. Names use ASCII environment identifiers; duplicate and prototype-related names, NUL bytes, more than 32 variables, values over 4096 UTF-8 bytes or more than 12000 total name/value bytes are rejected. A 16000-byte serialized-value cap bounds encrypted/agent transport.

The Studio editor works on create/update forms, hides values after saving, preserves unchanged keys, and disables edits while submission is pending. REST and MCP reuse the same command and access checks. `environmentNames` is the only environment metadata exposed in app reads. Values remain outside `spec` and are encrypted with app-bound authenticated encryption in current, observed, previous and queued-operation snapshots. Idempotency fingerprints for environment commands are keyed. Restart/scale preserve values; rollback restores the observed configuration snapshot, including its environment.

The managed agent must advertise `capabilities.environment=true`. Jobs requiring this feature remain gated against old agents. The agent materializes immutable app/revision-owned Secrets and explicit `secretKeyRef` entries. Runtime pods still have no service-account token, root access or writable root filesystem. Secret quotas include rotation headroom; cleanup retains live references while pruning abandoned versions, then retains only the current revision after readiness. App deletion removes owned Secrets and clears stored environment snapshots.

Application logs travel in a bounded Base64 transport field, then the control plane masks current/historical configured values and truncated boundary fragments before storing/displaying logs. This cannot protect transformed, encoded or otherwise deliberately disclosed values; applications must not log secrets. Secret names/values are never added to general search indexes.

Deployment and test evidence: `docs/verification/2026-10-09-app-environment.json`. Persistent volumes, targeted external egress, image delivery and public ingress remain separate runtime work; this feature does not start or migrate Lunares.
