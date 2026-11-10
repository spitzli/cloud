# Hosting runtime boundary

Source checkpoint: 2026-10-15. This extraction preserves existing Kubernetes, GitHub, Vercel, Database and Mail behavior; it does not deploy a cluster or introduce another cloud provider.

## Ownership

`modules/hosting/src/server` owns the full Hosting service graph: the shared bridge dispatcher, authorization, app/project operations, cluster enrollment and observations, reservations, retained capacity, environment handling, service bindings, Database protocol routing, BYOK bindings, Git connections/builds/releases/publication/reconciliation, and project setup admission/preview/execution/recovery. Managed Database offers, orders, credentials, maintenance, agent jobs and observations are owned by the Database module. The existing Hosting SQL definitions and migration helper also live in the module. Managed Database service implementations and their schema now belong to `modules/databases/src/server`, reached through the independent `DatabaseServices` contract in `@spitzli/database-contracts/runtime-ports`. Compatibility paths under `apps/control-plane/src/modules/hosting` contain typed composition or re-exports.

The control plane injects pools, transaction context, the selected local identity directory, installation admission checks, environment settings, secret codecs, OAuth introspection, provider transports and cross-product capabilities. Factory dependencies are immutable object bindings. Lazy property getters defer connected service lookup until invocation, avoiding module initialization cycles without a mutable service locator. The existing `executeHostingBridgeCommand` remains the single shared command dispatch path for both legacy delegated and trusted Clerk browser requests.

The independent Kubernetes agent still owns cluster execution. Agent registration, credentials, generations, claims, checkpoints and completion fences remain the existing protocol; moving application code does not change resource placement or give a worker bootstrap credentials.

## Public contracts and provider adapters

`HostingPorts` defines host-owned resources and capabilities. Neutral shared authorization, transaction, agent and operation types live in `@spitzli/hosting-contracts/runtime-ports`; neither product imports the other product’s server implementation. Existing `@spitzli/hosting-contracts` command validation remains authoritative. The new `@spitzli/hosting-provider-contracts` package owns pure GitHub/Vercel provider DTOs, ports and provider errors, so adapters do not depend on a product runtime. `modules/hosting` keeps compatibility exports for its earlier provider types.

`adapters/hosting/github` now owns the existing GitHub transport and webhook parser. Configuration and optional test-loopback permission are explicit factory arguments; the API compatibility wrapper supplies its existing settings. Existing account, repository, installation, source SHA, workflow attempt and artifact checks remain intact. The transport now explicitly requires a string workflow creation timestamp before returning it in the typed provider contract.

The Vercel transport and project readers remain in `adapters/hosting/vercel`. The Hosting module receives those readers as typed callbacks rather than importing the old `hosting-contracts/vercel` compatibility facade. Existing facade callers remain supported, but the runtime no longer imports that provider implementation path. The independent provider-contract package removes the former adapter-to-module dependency cycle.

Mail integration uses public `mail-contracts` provider and agent ports, including explicit lease DTOs. Cross-product capacity queries are injected capabilities; Hosting does not import Mail server implementation or the old Mail-core implementation facade. Capacity still counts retained native Mail and database reservations even when product switches are off. Mail domain normalization and its public error now live in the Mail contract subpath with existing compatibility exports preserved.

## Authorization and admission

Hosting authorization consumes the selected `IdentityDirectory` with locked session/subject/membership reads. Automatic Git publication separately locks and checks the exact durable source policy, revision, project, enabled state and auto-publish permission. Preview restrictions, customer/project status, management mode, operator security setup and scope checks remain in the module. No provider email address becomes a local identity.

Setup options omit Mail/Database rows and service calls when those products are disabled or not owned by this API process, while retaining Hosting-only options. Setup is cross-product admission. Saving, previewing or confirming a new database setup requires Database provisioning authority; binding an existing database requires Database access. Mail setup and creation of a Mail service binding require Mail access. An already confirmed interrupted setup can resume with access authority while Database drains. New previews or confirmations cannot disguise fresh provisioning as recovery. Workers retain the ability to finish accepted work; there is no blanket provisioning gate inside database execution. Disabled-module retained-resource preflight remains responsible for preventing shutdown with unfinished setup work.

## Preserved data and operations

Resource IDs, bound provider accounts, customer ownership, row/advisory locks, idempotency records, operation generations, lease checks, revision comparisons, deletion confirmation, retained volumes and secret encryption purposes are unchanged. SQL literal comparison against the pre-extraction tree matched every moved file except authorization, whose provider-specific identity reads now use the identity port and whose durable Git-policy query remains local.

No global provider initialization, account creation or migration executes when importing the module. Credential values are still returned only through the existing explicitly authorized credential-display or agent lease paths. Legacy PostgreSQL row shapes and setup dispatch internals remain compatibility implementation details; this extraction does not claim every stored JSON row has been replaced by a new normalized DTO schema.

## Verification and remaining scope

Module tests cover alternate identity projection, lock requests, membership revocation, preview denial, durable Git policy, shared bridge admission order, deferred composition, cross-product setup admission, and confirmed setup recovery while draining. Boundary tests prohibit application/adapter/foreign-module implementation imports in Hosting server code, require environment injection, and keep SQL service logic out of the API wrappers.

The extraction worker ran API type checking, the API unit suite, and the focused Hosting module tests. Central disposable PostgreSQL/RLS regression execution is coordinated by the parent task; this note does not claim those results before execution. No live GitHub/Vercel call, cluster mutation, provider account creation or production migration was performed.

Hosting UI ownership is handled by the separate concurrent UI extraction. Application route mounting, installation lifecycle preflight, generic identity implementations, the deployment distribution and the physical Kubernetes agent remain their respective host/core/adapter responsibilities. Existing-provider completion is the selected scope; AWS/GCP services are not prerequisites for this extraction.
