# Managed PostgreSQL operations

The first internal production instance is ready for the Webdock tenant as of 2026-10-12. [Deployment and client evidence](../verification/2026-10-12-webdock-postgresql-production.json). The catalog remains operator-pilot only; existing application data has not been migrated.

Implemented v1 is single-primary PostgreSQL 18 with CloudNativePG, a separate bounded ext4 storage pool, and PgBouncer transaction pooling. Shared customers receive separate databases and roles; Dedicated receives its own CNPG installation. There is no HA claim or automatic cross-node volume migration.

**Production backups remain unconfigured.** The earlier owner deferral (2026-10-12) is superseded by the implementation scope in the [2026-10-25 storage handover](../handovers/2026-10-25-s3-managed-storage.md). No production S3 target, Barman, WAL archive, PITR, backup-based clone or restore is enabled by that authorization. UI, API and immutable existing offer terms must continue to describe the actual unconfigured state until recovery qualification and explicit activation. Retained local volumes and migration sources are not backups.

## Provisioning and accounting

Operator: Infrastructure → cluster → Managed PostgreSQL. Register only a qualified, already carved-out database pool. Provision a Shared backend explicitly. Create immutable tariff revisions; availability starts disabled. Pilot and public visibility are explicit operator actions, and public orders additionally require `WEBDOCK_DATABASE_PUBLIC_ENABLED=true` in Auth. No signup or project-creation hook provisions a database.

Customer: tenant → Managed PostgreSQL → project → review order. The price, resource scope and absence of backups are accepted before reservation/provisioning. Dedicated provisions directly after acceptance; Shared requires free slots and logical allowance on a ready operator pool. Repeated order acceptance reuses the accepted order. Billing starts only after the authenticated agent proves actual readiness and verified connections. Pause does not end billing. Cancellation requests are separate from retained deletion; invoice handling remains manual.

Physical promises include paused instances and retained volumes. Shared is reserved once per installation, with logical customer allowances separately bounded. Each installation reserves PostgreSQL plus 200m CPU, 512 MiB RAM and 512 MiB ephemeral overhead. Registered pool disk is reserved globally and cannot also be sold as App storage. Fresh node observations subtract known managed actual requests, then add all held promises plus other workloads.

## Installation sequence

1. Refresh `infra/postgres/inventory.py` and native control-plane reservations. Review deployed source hashes. This branch must not overwrite the separately deployed three-node App storage router or operator SSO changes. Reconcile that baseline first.
2. Use the explicit owner migration runner: `bun apps/auth/scripts/migrate-hosting-databases.ts /private/owner.env`. The file supplies `DATABASE_URL_UNPOOLED` (or `DATABASE_URL`) and optional `AUTH_DB_ROLE`. Run after existing Auth/Hosting/Mail schema migrations. It is transactional, adds tables and narrowly scoped runtime grants, and enables no offer. Keep owner credentials offline. Never push schema from HTTP or heartbeats.
3. Verify pinned images in `infra/postgres/versions.env` with upstream signatures. Install the pinned CNPG controller in `cnpg-system`; retain the existing cert-manager. Inspect controller health and CRDs before agent capability activation.
4. Carve out a separate pool without a capacity gap: lower approved native App storage allowance first, reduce the selected App helper ceiling while preserving all existing allocations, create the separate DB helper pool, observe it, then restore the approved aggregate ceiling and register the DB partition. Never increase either ceiling above measured disk minus host reserve. Preserve at least the existing production host reserve; pilot's smaller reserve is test-only.
5. Use a root-owned, mode-0600 `/etc/webdock-postgres-storage.json` in the existing storage-v1 configuration shape with its own `root`, `poolID`, node/cluster identity, capacity and reserve. Run `webdock-postgres-storage.service` on the selected DB node with the dedicated socket. Deploy `database_storage.py` alongside a compatible reviewed `storage.py`, `storage_service.py` and `storage_client.py`. The ordinary App service must not get `WEBDOCK_DATABASE_STORAGE=1`. Never substitute local-path requested capacity for an enforced filesystem.
6. The managed agent needs `databases.py` and `database_migrations.py` plus the updated daemon/observer/storage client. The BYOK installer is intentionally separate: this product runs on native managed infrastructure. Do not replace unrelated Mail modules or their keys. Agent configuration `/etc/webdock-postgres.json` must be root-owned mode 0600 and contain exactly `image`, `poolerImage`, `issuer`, `apiCIDRs`, `podCIDRs`, `storageSocket`, `caFile`. Images require version tag AND digest. The CA file contains public certificates only. The selected ClusterIssuer must issue the external hostname and internal service names; the local pilot CA is not a production issuer.
7. Render and inspect `python3 infra/postgres/boundary.py --kubeconfig /private/explicit-kubeconfig`; apply only with `--apply`. Restrictive DB policies are installed before resource-version-fenced App-policy exceptions. Verify CEL type checks and run `check_boundary.py`. Namespace labels are privileged: ordinary App identities may not create/modify namespaces, admission or RBAC. Preserve that existing RBAC boundary.
8. Configure real endpoint DNS and the reserved TCP ports 32000–32199 to the selected node, including host/provider firewall policy. Verify TLS hostname and CA from the external customer path. The current local `.example.invalid` DNS shim is solely test infrastructure. Auth carries management requests, never customer SQL.
9. Start with disabled tariffs and synthetic operator/customer identities. Run the end-to-end flow and the 24-hour bounded pilot before public availability. See qualification evidence; do not mistake local proof for production rollout.

## Connections and credentials

Customers reveal confirmed credentials in Studio only. Generic REST/MCP commands never return passwords. Use direct connections for schema migration, PgBouncer for application transactions, `sslmode=verify-full` and the downloaded CA. Application schema is `app`; Prisma must explicitly select it. Use the migration role for DDL and the application role for DML. Do not disable TLS checks or use the owner/superuser role in an application.

Credentials rotate behind a new revision and become visible only after proof. A failed/uncertain rotation requires reconciliation; do not guess which credential is active. CA changes fail closed against persisted identity. CA rotation requires an overlap trust bundle distributed to clients, a reviewed certificate rollout, verification of direct and pooled paths, and only then removal of old trust. Automated CA rotation is not implemented.

## Pause, restart, resize and migration

Shared pause disables only its two login roles and terminates their connections. Dedicated pause additionally hibernates the primary; storage remains. Resume refuses to replace missing data with an empty database. Restart is fenced by the observed Pod UID. CPU/RAM changes retain installation identity. Larger physical volumes and Shared→Dedicated use an explicit, interruption-accepted logical migration.

Migration reserves the target before fencing the source, uses a non-superuser schema migration role, streams dump/restore, verifies row contents/counts, then switches the customer endpoint and billing. The old source remains fenced for seven days. Neighbouring Shared databases stay available. After the retention deadline, the operator reviews a session-bound source cleanup preview and confirms the exact name; only the old source is purged. Referenced legacy roles are retained NOLOGIN instead of altering foreign grants.

If a job/lease is uncertain, stop and inspect operation ID, checkpoint, Cluster/Job UID and source/target state. Reconcile observes the existing job and never repeats an uncertain import automatically. Keep the source fenced until target integrity is established. If the transfer failed, keep both copies and reservations; this version has no automatic abort/rollback wizard. Do not manually enable both writable copies or edit accepted orders. Resolve under a reviewed operation-specific recovery procedure.

## Retention, purge and rollback

Customer cancellation alone deletes nothing. Operator retained deletion ends service and billing, fences access and starts seven-day retention. Purge requires a fresh five-minute session-bound preview, matching revision, elapsed retention and exact database name. Dedicated teardown checks namespace/PVC/PV ownership and resource versions before helper purge. Shared purge removes only the selected logical database and unreferenced roles. A project with PostgreSQL offer/order history must be archived, not permanently deleted.

For control-plane rollback, disable catalog availability, stop new DB claims and preserve schemas, keys, operation checkpoints, volumes and reservations. Existing SQL service can continue. Do not remove CNPG CRDs, namespaces, finalizers, PVs, helper journals or admission boundaries while any installation is held. Do not roll the App helper back over newer deployed router changes. Returning a catalog to disabled does not cancel accepted customer orders.

## Monitoring and remaining launch evidence

UI shows observation time, measured logical size, dedicated filesystem use, warning thresholds and unknown/stale state. Missing observations cannot start billing. A shared logical allowance is measured, not a hard per-customer filesystem cap. Review pool growth and connection demand; a full Shared filesystem affects all occupants.

Local evidence covers SQL/role isolation, direct/pooler TLS, pg/Prisma clients, ENOSPC/fail-closed storage, lifecycle and migration. Initial production DNS/firewall/TLS and external pg connections are now verified for the internal Webdock instance below. External Vercel/Prisma production clients, sustained load, 24-hour observation and external alert delivery remain broader launch checks.

## TODO: backups

- Select an existing external EU S3 provider, bucket and region through operator configuration; never paste keys into chat.
- Verify Barman/plugin/sidecar provenance and compatibility; reserve backup/restore resources.
- Configure encrypted credentials, base backups, WAL archive and retention.
- Prove independent restore, PITR, cross-customer isolation, RPO/RTO and alert delivery.
- Only then expose backup-dependent actions and recovery-window terms in new tariff revisions.

## Qualification observation

`infra/postgres/observe_pilot.py` is restricted to the disposable local container. It records installation UID, ready primary and restart count every five minutes, and emits a completed record only after elapsed wall time. The current 24-hour run started 2026-10-12T19:15:42Z; initial two installations were Ready with zero restarts. Logs remain in the ignored local progress directory until reviewed; this is availability sampling, not a continuous performance or failover test.

The installed BetterAuth limiter counts until an idle window; continuously polling multiple Studio views can exhaust its shared introspection bucket. The new DB view uses a 75-second interval. Review the broader authentication rate policy before multi-user production qualification; do not weaken authentication or treat an HTTP429 as success.

For the outstanding external Vercel client qualification, installation of the currently absent CLI is recommended (`bun add -g vercel`) to inspect deployments, logs and environment bindings. No Vercel deployment was performed by these local tests.

## First production instance: Webdock

Tenant `103216044377636864`, project `103216298636345344`, database `103217546714419200`, installation `103217546630533120`, on `wd-eu-02`. PostgreSQL has 1 GiB RAM, 500m CPU and a 10 GiB bounded filesystem; the separate pool is 32 GiB. The operator accepted the Starter catalog terms (14.90 EUR net/month, manual invoice handling). No public tariff release or external provider purchase occurred.

Prepared empty schemas: `webdock_auth` (Auth), `webdock_admin` (customer/registry), `webdock` (CMS), `webdock_mail` (Mail). The customer migration role owns them; the application role receives usage and default DML grants. These are destinations for a separately planned migration, not proof that the services already use this database. Existing `app` remains the default connection schema; clients using the prepared schemas must choose their schema explicitly.

Endpoint `db-103217546630533120.pg.webdock.dev`, direct TCP32000, pooler TCP32001. TLS uses the cluster's dedicated private CA; obtain its certificate and credentials from protected Studio. Only these two ports were added to the existing node02 guard; member-port and other-node rules remain unchanged. Use a client-side query timeout or transaction-local SQL timeout with PgBouncer; it rejects the `statement_timeout` startup parameter.

The production App router/helper were preserved by hash. PostgreSQL's helper is isolated in `/opt/webdock-postgres-storage` with `/etc/webdock-postgres-storage.json`. Agent daemon adds the database stage; the deployed observer adds only PostgreSQL capability; the existing routed storage client adds an optional socket argument without replacing its router. Original files and applied manifests are root-only under `/root/webdock-postgres-20261012` on node02. A future full agent rollout must preserve/reconcile this deployed baseline.

Both production app deployments report READY at merge `6b3e76e`. Direct and pooled Node pg logins succeeded for both roles, all four schemas were visible, and incorrect password/hostname tests failed as required. PostgreSQL and PgBouncer have zero restarts at verification; the existing App pod remains Ready with zero restarts. This initial check is not a 24-hour/HA/SLA qualification. No backups or existing-data migration were enabled.


## Native backup and recovery implementation — 2026-10-25

The dedicated implementation is in `crates/databases`, `DatabaseJobSpec` and
`NativeDatabaseExecutor`. It does not use the generic PostgreSQL plugin or a
45-second session-bound plugin job. [Local verification and remaining scope](../verification/2026-10-25-native-postgresql-backup.json).
The [fixture instructions](../../infra/postgres/backup/qualification_README.md)
distinguish real CNPG recovery from synthetic Core authority tests.

Apply the new forward migration `0048_database_backups.sql` through the existing
offline migrator. No old offer or order snapshot is rewritten. Existing no-backup
terms remain accurate. Shared backup/recovery and backup-based clone remain
unavailable. No production target, image trust, signer, rollout, data migration,
RPO/RTO or recovery-window entitlement is enabled by this code.

### Authority and operator controls

Only an authenticated operator with current project/customer authority can register
a destination or grant/update/revoke a policy. Destination registration stores writer
and separate read-only recovery credentials encrypted with the existing Core cipher.
The registered recovery-key reference must point to separately recoverable operator
key material; a reference alone is not proof of escrow. Preserve the Core encryption
master independently of PostgreSQL and the S3 archive.

Native managed commands are `databases.backupDestinations.register`,
`databases.backupPolicies.grant`, `databases.backupPolicies.update`,
`databases.backupPolicies.get`, `databases.backupPolicies.revoke`, and
`databases.backup`. Commands retain the existing idempotency, authorization, audit
and receipt mechanisms. Policy updates/revocation carry the current database and
policy revisions. The current schedule contract is daily six-field CNPG cron:
`0 <minute> <hour> * * *`.

A durable Core grant binds installation, customer, project, exact source CNPG UID,
immutable destination/archive identity, separate credential/recovery references and
versions, schedule/retention decisions and resource revision. The archive path is
`postgresql/<customer>/<project>/<installation>/<policy>`; the plugin `serverName`
is `wd-<installation>-<policy>`. Every native CNPG Cluster is named `postgres`, so
that name must never identify a shared archive. `serverName` belongs in the Cluster
plugin parameters, not ObjectStore configuration. See the
[Barman 0.15.1 usage contract](https://cloudnative-pg.io/plugin-barman-cloud/docs/usage/).

Installed CNPG RBAC owns recurrence after the initiating session expires. Core
continues to own the grant and records observed completions through native database
jobs. Observation-only refreshes use the real recorded approver and the durable
policy, with no fabricated user/session. Secrets remain ephemeral agent material;
the persisted spec/fingerprint contains references and revisions. The retained
legacy transport rejects backup/recovery packets; use the typed native transport.

Suspending recurrence, archiving WAL and granting retention deletion are independent
controls. Stopping WAL requires a suspended schedule. Revocation is reported as
pending until a current enrolled agent observes the applied suspended ScheduledBackup
UID/revision. Revoking the grant fences obsolete leases immediately; an already
started backup can finish and WAL/retention rights are not silently changed.
Revoke and observe suspension before disabling the database module or its tenant/project.
Automatic lifecycle-triggered cancellation is not qualified by this milestone.
A successful upload is insufficient: a base backup needs an observed CNPG
`completed` Backup, UID, base-backup ID and completion timestamp.

### Fresh recovery and resource boundaries

`databases.restore` requires an exact `restorePointID`, `targetTime`, numeric
`timeline`, explicit recovered `credentialRevision` and expected
`verificationSHA256`. It also requires an accepted, unused Dedicated target order,
matching name/project and current source revision. The current implementation
supports the same project and qualified source cluster. The target must still be
unclaimed with no CNPG UID; if its create job has already started, obtain a fresh
target order. This is deliberately fail closed; a convenient atomic restore-order
flow remains follow-up work.

Recovery uses a new installation, namespace, bounded volume and endpoint and the
source's read-only S3 credential. It never overwrites or releases the source. The
exact base/time/timeline go into CNPG recovery configuration; recovery does not
activate WAL writes into the source archive. Recovered native database/role names
are remapped to the new product identity while preserving ownership, grants and
SCRAM verifiers. Other restored roles remain intact. Core retains encrypted confirmed
password versions; selecting an incompatible historical version fails login/readiness.
Readiness also requires the expected data hash, owned physical resources, direct and
pooler endpoints, full TLS hostname/CA verification and actual SQL/product operations.

Operator-owned native runtime configuration has an optional `backup` section with
`pluginImage`, `sidecarImage`, approved HTTPS `endpoints` and explicit `egressCIDRs`.
Both images need immutable digests. The observed Barman deployment/sidecar and the
additional backup admission policy must match before the agent advertises
`databaseBackupVersion: 1`; Core requires that capability. The existing database
admission allowlist must also admit the sidecar. No config is automatically installed.
The provenance fixture signer is local-only and is not operator-approved production
trust.

Reservations and namespace quotas include CPU, memory and ephemeral storage for
the Barman restartable init sidecar and a conservative recovery-job peak, as well as
the existing PostgreSQL/pooler/init overhead. The source remains held while the
accepted fresh target reserves separate storage and recovery capacity. The database
PV remains a bounded local filesystem. There is no S3 mount or storage release based
on an uploaded object.

The digest verifier currently sorts/aggregates table rows inside PostgreSQL under a
bounded statement timeout. Its cost and memory behavior must be qualified for large
databases before selling this workflow. The connected Core/native-agent and independent fixture key/identity recovery
checks now pass locally. Production resource profiles, failure notifications,
external endpoint coverage, provider credential rotation and production admission/
trust installation remain acceptance work. The earlier component fixture used
private HTTP MinIO; the connected drill adds an owned, verified TLS bridge to that
service. This does not establish Wasabi or production-provider compatibility.

### First-party integration contract and local fixture API

The exact command contract is exported as
`webdock_databases::MANAGED_COMMAND_SCHEMA` and stored in
`crates/databases/src/managed-command.schema.json`. Parse requests with
`ManagedCommand::parse`; execute them through the existing authenticated
`DatabaseService::execute_managed(principal, stored_authority, command)` entrypoint.
No plugin authority or synthetic browser identity substitutes for that principal.
The connected fixture creates actual first-party password/TOTP operator sessions.

The existing browser entrypoints are `POST /api/console/databases` and
`POST /v1/databases`, with the command JSON as the body and the normal authenticated
actor flow. Responses wrap the result in `{ "data": ... }`. The existing route
checks the `databases` read/provision feature gate and forwards `databases.*`
commands through the native schema; no new Core plugin caller is required. This
drill exercises the domain plus real agent gRPC, not the later Console journey.

Core placement configuration uses `WEBDOCK_DATABASE_CLUSTER_ID`,
`WEBDOCK_DATABASE_DOMAIN`, `WEBDOCK_DATABASE_PUBLIC_ENABLED` (keep false for this
milestone) and optional `WEBDOCK_CONTROL_DATABASE_ID`. The effective database cipher
key is the existing `WEBDOCK_HOSTING_ENCRYPTION_KEY`; the current initializer falls
back to `BETTER_AUTH_SECRET` if it is absent. Preserve the actual effective key,
independently retained Auth/RLS identity keys and TLS issuer identity; a newly
generated replacement cannot open historical credential versions. The fixture uses
its own generated key bundle and does not select or replace any production key.
Point the native agent's `AgentConfig.databaseRuntime` at its private runtime JSON.

Operator setup uses the existing `ManagementConfig` (`cluster_id`,
`endpoint_domain`, `public_ordering: false`, optional `protected_database_id`, and
operator-supplied `lease_secret`). A new destination command has these exact fields;
credential values below denote private operator input, never public configuration:

```json
{
  "action": "databases.backupDestinations.register",
  "endpointURL": "https://approved-s3-endpoint.example",
  "region": "approved-region",
  "bucket": "approved-private-bucket",
  "keyRecoveryID": "110",
  "writeCredentials": {
    "accessKeyId": "PRIVATE_OPERATOR_INPUT",
    "secretAccessKey": "PRIVATE_OPERATOR_INPUT",
    "readOnly": false
  },
  "recoveryCredentials": {
    "accessKeyId": "SEPARATE_PRIVATE_OPERATOR_INPUT",
    "secretAccessKey": "SEPARATE_PRIVATE_OPERATOR_INPUT",
    "readOnly": true
  },
  "idempotencyKey": "operator-destination-request-001"
}
```

Use the returned `destinationID` with
`databases.backupPolicies.grant`: `databaseID`, current database `revision`,
`destinationID`, daily six-field `schedule`, `retentionDays`, `retentionDelete`,
`idempotencyKey`. The response contains `policyID`, policy `revision`, `operationID`
and pending `revocationEffective: false`. `databases.backupPolicies.update` requires
`databaseID`, database `revision`, `policyRevision`, `schedule`, `suspended`,
`walEnabled`, `retentionDays`, `retentionDelete`, `idempotencyKey`.
`databases.backupPolicies.revoke` requires both revisions and an idempotency key;
`databases.backup` requires `databaseID`, database `revision`, `idempotencyKey`.
`databases.backupPolicies.get` requires only `databaseID` and returns `policy` and
`backups`. Each restore point has its own `id`, `policyID`, exact `baseBackupID` and
`completedAt`; use its `id` as `restorePointID`.

The fresh target is an accepted, unclaimed Dedicated order. There is no atomic
restore-order action yet. With that `targetOrderID`, submit:

```json
{
  "action": "databases.restore",
  "sourceDatabaseID": "101",
  "restorePointID": "102",
  "targetProjectID": "103",
  "targetOrderID": "104",
  "revision": 2,
  "credentialRevision": 1,
  "targetTime": "2026-10-25T12:00:00Z",
  "timeline": 1,
  "verificationSHA256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  "name": "Exact accepted target name",
  "idempotencyKey": "operator-restore-request-001"
}
```

`revision` belongs to the current source database; `credentialRevision` explicitly
selects the historical version valid at the recovery time. A missing key, version,
capacity reservation, authority, exact base, UID, expected hash or SQL/TLS proof
prevents readiness. The sample IDs/time/hash are illustrative; they are not a
production request or a recovery commitment.

Public managed database views expose `backup` according to
`webdock_databases::BACKUP_STATUS_SCHEMA`
(`crates/databases/src/backup-status.schema.json`). Status is `not-configured`,
`configuration-pending`, `configured`, `suspension-pending` or `suspended`.
`observedControls` is null before agent evidence; afterward it separately reports
`schedule`, `walEnabled`, `retentionDelete`, `retentionDays`, the ScheduledBackup and
Cluster resource versions, and ObjectStore UID/resource version. Desired controls
alone never count as those observations. `publicRecoveryAvailable` remains false,
`disabledReason` is `production-qualification-required`, and `rpoSeconds`,
`rtoSeconds`, `recoverableUntil` and `lastRestoreTestAt` remain null. The Console
must preserve that disabled state and existing immutable offer terms.

The Console now consumes that strict current view, displays historical completion
timestamps separately from current observations, and reads the latest 100 recorded
completions through `databases.backupPolicies.get`. A newer desired policy revision,
a failed observation read or a pending state prevents old controls being presented
as current confirmation. Shared backup, restore and clone remain unavailable.

Current operators with write authority can register an HTTPS destination, grant or
update a policy, request a backup and revoke recurrence. Schedule suspension, WAL
archiving and retention deletion remain separate controls. Revocation stays pending
until effective observation; it does not imply all three controls stopped. Provider
credentials are request-only and immediately cleared from DOM fields; registration
does not use TanStack mutations or retain credentials in caches/storage. An uncertain
registration retains only its request identity/fingerprint and requires identical
credentials to be re-entered. Other uncertain mutations retry the frozen original
command/key. Conflicts require explicit refresh, and lost authority disables writes.

Public Restore remains disabled with `production-qualification-required`. The
Console also keeps operator Restore disabled until a separately reviewed atomic
restore-order admission exists; it does not race ordinary order acceptance against
the agent's Create claim. Historical offer snapshots, prices and backup exclusions
are unchanged. Bearer REST/MCP integration is a separately coordinated backend task;
session `/v1/databases` availability does not establish bearer parity.

The Part A evidence is
[`2026-10-25-pg-console-backup-ui.json`](../verification/2026-10-25-pg-console-backup-ui.json).
Its EN/DE browser checks at 375/1280 pixels use a controlled HTTP fixture and actual
Console components; they are not new Core/Native authentication or PostgreSQL
qualification. Fresh real Console-to-Core-to-Native acceptance remains held for
Root's migration 0054/compiler/pin release. No production activation or recovery
commitment follows from these UI checks.

The subsequent Console review fixes require a successful current permission read,
not cached flags after an error. A failed history read retires backup metadata and
credential forms, aborts pending transfers and stops source polling/focus/retry.
Explicit refresh first revalidates permissions and then obtains fresh history;
read-only reauthorization restores observations without restoring write controls.
Project and principal boundaries retire their drafts and pending deliveries.

Observation validity also checks the policy's actual observed revision, timestamp,
suspension and control/resource evidence. Invalidation of `observedAt` at the same
revision is pending, not configured. The client applies Core's strict 90-second
freshness interval conservatively using its local clock and rechecks manual backup
admission at dispatch. Clock skew can leave the UI pending; this is not a clock
synchronization or recovery guarantee, and Core remains authoritative.

An ambiguous registration outcome survives invalid local re-entry, refresh and
same-principal reauthorization with its original digest/key. Those actions cannot
convert uncertainty into a new request. Separately, the highest accepted policy
revision/revocation requirement survives unrelated destination/manual receipts
until matching fresh observation is available. The review-fix evidence and new
source hashes are recorded separately in
[`2026-10-25-pg-console-backup-ui-review1-fixes.json`](../verification/2026-10-25-pg-console-backup-ui-review1-fixes.json);
the earlier Part A checkpoint remains historical.

Review r2 moves the non-secret unresolved request identity and highest policy
observation requirement into the stable resource owner above the conditional
detail loading/error view. Detail 503/403, same-resource panel close/reopen and
read/write privilege changes abort transports and wipe forms while preserving the
original retry digest/key and policy target. Recovering a detail error requires a
fresh permission read; it does not make an ambiguous write a new operation. True
principal/session, platform, customer or resource retirement discards that scoped
owner. Private query views still use the stricter authority scope and are refreshed
when privileges or available project metadata change.

A forbidden mutation immediately unmounts the sensitive destination form, including
its detached input cleanup. Fresh reauthorization recreates empty fields; it does
not retain provider credentials to replay a request. Before the first current
history response, a cached configured view is labelled pending rather than observed.
The new checkpoint is
[`2026-10-25-pg-console-backup-ui-review2-fixes.json`](../verification/2026-10-25-pg-console-backup-ui-review2-fixes.json).
Its Core-shaped responses and mutation counters remain controlled HTTP fixtures;
real Core/native acceptance and Part B stay held for Root's 0054 freeze and a new
explicit release.

Review r3 places the scoped, non-secret intent provider above Bootstrap and the
real Console SessionGate. The gate and its private cache purge still unmount the
Shell and credential views. Same-principal/session role or transient view changes
reattach the intent only after fresh permission/history checks. A confirmed null
session/logout, changed identity/platform or actual selected resource/project
retirement clears the old scope; an unavailable session read (including 401/503)
hides private views without inventing a confirmed logout.

The product-owned transition machine fences each delivery and refresh by attempt
identity, delivery generation, authority epoch and refresh nonce. The latest
refresh blocks new dispatch, and stale or ignored-transport completions cannot
clear a newer attempt, overwrite its failure or restore denied authority. It is
not an authorization platform: actual permissions and Core remain authoritative.
No intent, provider key or source material uses browser storage or a module-global
map. The normal UI has one selected resource view; a same-resource duplicate-view
probe also stays denied through the shared cell epoch until fresh reauthorization.

Mutation receipts are decoded by the dispatched action before resolving an unknown
outcome. Registration requires only its strict destination receipt. Policy mutations
require the source-bound policy/operation receipt with the Native contract's exact
revision semantics and `revocationEffective: false`. Manual backup has no policy
revision input, so later revisions must be bounded by the same source's current
read rather than an unbounded wildcard. Wrong-action, malformed, foreign-policy or
unconfirmed-revision responses after send remain ambiguous under the original key.
The new checkpoint, actual SessionGate tests and held-refresh causal proofs are in
[`2026-10-25-pg-console-backup-ui-review3-fixes.json`](../verification/2026-10-25-pg-console-backup-ui-review3-fixes.json).
All browser observations and mutation counts in this checkpoint are controlled
HTTP fixtures; Core/native qualification and Part B remain held as above.

Review r4 distinguishes the retained resource's actual project from a dropdown's
transient default after SessionGate teardown. Selecting the same retained project
restores its retry identity and highest policy expectation; selecting a different
actual project/resource/identity still retires the old scope and its transports.

Manual backup preparation also rechecks eligibility immediately after asynchronous
digest calculation and before delivery begins. The callback comes from the latest
committed view, uses the current cell policy target and reads the current clock.
An invalidated observation, a 90,001ms-old observation or a newly accepted higher
target cannot dispatch merely because the pre-digest view was eligible. This adds
no authorization framework or API; Core remains authoritative.
The new scoped tests and controlled project-20 browser flow are recorded in
[`2026-10-25-pg-console-backup-ui-review4-fixes.json`](../verification/2026-10-25-pg-console-backup-ui-review4-fixes.json).

For an explicitly approved private HTTPS provider, the optional native runtime
`backup.endpointCACertificate` contains only the public PEM trust bundle.
The agent creates an immutable CA Secret and passes its key through ObjectStore
`configuration.endpointCA`; private key material in this configuration is rejected.
Default trust is retained when this field is absent. Both SQL endpoints continue to
require full CA and hostname verification.

Pending long operations renew only a currently authorized, unexpired Core lease
when polling within its final minute. The generation and token remain unchanged;
expired work still requires reconciliation. A complete Kubernetes list may carry
an empty continuation string; only a nonempty continuation or an over-limit list
fails the backup observation boundary.

The connected local fixture API, private recovery bundle lifecycle, exact cleanup
and commands are documented in
[`qualification_README.md`](../../infra/postgres/backup/qualification_README.md).
Migration 0048 is frozen for this milestone at SHA-256
`8c6a28a667069797c2f589730020b7914b2d5f5a0cd04fb4825981bb89750092`.
Any later schema change requires a new coordinated migration.


### Connected local qualification result — 2026-10-25

The real Core domain, enrolled authenticated Unix gRPC service and
`NativeDatabaseExecutor` completed backup and fresh-target recovery against
CNPG/Barman/MinIO. A daily scheduled backup ran after its initiating browser expired;
an on-demand backup, separate schedule/WAL/retention observations, pending/effective
revocation, stale-delivery retirement, capacity rejection and a real disabled-key
recovery failure were verified. Obsolete delivery acknowledgment drains the outbox;
it does not install a checkpoint or renew execution.

The source database stayed unavailable. The original Core container and TLS issuer
were destroyed, then a fresh Core and issuer were recovered from a protected backup
and separately retained fixture key/identity bundle. Historical credential revision
1 authenticated after the source had confirmed revision 2. Native recovery remapped
the product identity, verified A present/B absent and the expected data hash, and
passed direct/pooler TLS and transactional product SQL before Core recorded Ready.

The independent timed target took **78.221 seconds from the Core restore request to
native SQL/product readiness** (78.206 seconds after the accepted response). Fresh
order through readiness was 78.276 seconds. Additional independent sentinel/Core
assertions ended 78.298 seconds after the request; that is a later timer endpoint.
The workload had one app table, one recovered row and an 8,083,135-byte logical
database, on a bounded 6 CPU / 8 GiB node with source and fault-recovered target
reservations held. Each installation used a separate 1.5 GiB filesystem. Operators
and images were already available. This is a small local measurement, not an SLO.
The earlier unforced 30/30 crash sample remains separate RPO evidence; no final WAL
switch was forced in the connected drill.

The initial 128 MiB native sidecar OOM-killed during a real base backup. New policy
grants now reserve/limit 256 MiB, and both real base backups completed. Other fixes
cover empty Kubernetes continuation strings, renewal of live pending leases,
immutable backup evidence locks under RLS, and guarded configuration after expired
restore reconciliation. The Core logical-backup helper now inventories pgcrypto
extension-member ownership separately. The first diagnostic bundle was repaired
from the pinned bootstrap ownership invariant after source destruction.

Final verification passed 125 Rust tests, the live fresh restricted-role persistence
fixture, and strict Clippy including the connected caller. All four attempt custody
labels, owned S3 buckets/users/policies and the successful run's three native volumes
were removed; the private recovery bundle and fixture credentials were destroyed.
The existing MinIO identity remains running. Exact commands, failures, timer
boundaries and cleanup are in the dated verification JSON. No production signer,
S3 destination, IAM grant, customer activation, Shared backup or immutable old offer
was changed.

### Review fixes for queue, archival, resize and retained history

The earlier connected recovery drill is historical. The transition fixes and fresh
acceptance are recorded separately in
[`2026-10-25-native-postgresql-backup-transitions.json`](../verification/2026-10-25-native-postgresql-backup-transitions.json).

Manual backup requires an unrevoked, unsuspended policy with WAL enabled. Core
validates provisional claims before persisting/issuing a lease. A deterministically
invalid backup control request that has never issued an envelope is retired in a
committed transaction, keeping its installation capacity held and allowing the next
valid request through. Previously issued or uncertain work remains quarantined for
reconciliation; an invalid request is never acknowledged as an external success.

Forward migration `0051_database_backup_transitions.sql` prevents project and
customer archival until recurrence, WAL and retention deletion are separately
observed stopped at the current policy/resource revision. Both Registry status
updates and the Default-project cascade hit the SQL fence. Fresh agent/stop evidence
is required, and scope row locks serialize archival against new grants. A previously
completed, exact physical purge proof also establishes that its bound installation
is gone. These guards do not silently revoke any separate authority or fabricate a
session/controller principal. Registry only maps the database guard to a clear
conflict. Existing 0048/0050 are unchanged.

Resize preserves the backup/restore reservations already held. A narrow ordering-
worker function advances only the admitted policy/resource revision without granting
that worker general policy-write access. Issued conflicting work must finish or be
reconciled; unissued queued control work can be explicitly superseded. After native
resize readiness, Core automatically reconfigures/reobserves the same policy and
archive identity. Namespace quota follows the main PostgreSQL delta, preserving
sidecar/replay/ephemeral headroom. Allocation markers make retry after quota commit
safe if the Cluster resource patch has not yet completed.

Backup inventory uses 100-object pages at one provider resource version and keeps
only a single completed scheduled candidate in memory. Failed/manual entries cannot
replace it, historical provider/Core records are preserved, and callback payloads
stay bounded. A known observation failure immediately invalidates the current
observed timestamp; public configured/suspended status additionally needs a fresh
observation. The historical `lastSuccessAt` is retained and no recovery window/SLO
is inferred from it. Obsolete callbacks cannot invalidate a newer policy revision.

The fresh connected review retained 104 provider history objects, changed known
failure to `configuration-pending`, and completed actual 500m→1000m resize plus
backup re-observation in 25.314 seconds. Native quota and Core demand matched at
1800m / 2.25 GiB / 1.25 GiB ephemeral, with the source UID unchanged. A fresh
historical readonly-role restore took 72.512 seconds on the small local fixture;
the source was available, so this is not a repeat of the independent unavailable-
source proof or a production RTO. The actual export source used the existing
confirmed application credential through shared HMAC derivation, verified TLS and
an app-only PG18 dump; stale references were refused after real Native rotation and
pause. SQL plaintext never carries a password, and no second credential truth is
stored. Dedicated physical purge removes the entire owned installation, including
its SQL roles; imported old export identities are explicitly disabled during fresh
restore.

Actual Registry project/customer archival was refused before stop, then succeeded
with its empty Default-project cascade after observed stop. This qualifies these
local transitions only. Production signer/provider/key custody, external S3/Wasabi,
large workloads, aged retention deletion, provider rotation and alerts remain
separate acceptance. Shared backup/restore, immutable old offer terms and public
recovery-disabled state are preserved.
