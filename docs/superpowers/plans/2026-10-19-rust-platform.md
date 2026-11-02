# Rust platform implementation plan

> **For agentic workers:** Use superpowers:executing-plans or superpowers:subagent-driven-development. Independent contract investigations and isolated subsystems may run in parallel under superpowers:dispatching-parallel-agents. The owner explicitly authorized execution without further approval rounds.

**Goal:** Replace first-party backend runtimes with tested Rust services while preserving existing customer behavior and introducing the approved Console and plugin execution model.

**Architecture:** Loco and SeaORM compose a PostgreSQL-backed API with Webdock-owned identity. Shared Rust contracts bind independent supervisor/agent/plugin executables; React/Tailwind is the browser runtime. Kubernetes/Helm and Docker implement the same authorized runtime operations.

**Tech stack:** Rust edition 2024 (minimum 1.94), Loco 1.2, SeaORM 2, Tokio, Tonic/Prost, React, Tailwind, PostgreSQL.

**Spec:** `docs/superpowers/specs/2026-10-19-rust-platform-design.md`.

## Global constraints

- No external inspiration names, URLs or assets in repository files, commits, branches or PRs. Independently authored implementation.
- No production mutations, customer data tests, automatic migrations, arbitrary shell execution or exposure of provider credentials.
- Existing string Snowflake IDs, RLS/signed scope, canonical protocol hosts, leases, encrypted state and operation semantics remain compatible.
- Own auth; Clerk must not be required by the finished runtime. Local account migration is explicit and audited.
- Provider integrations are independently distributed plugins. Core owns provider-neutral services only.
- Kubernetes/Helm is primary; Docker is fully supported. Managed production Mail retains the repository's Kubernetes/Stalwart CE per-enabled-customer decision.
- Original checkout and its WIP are preserved. Current isolated baseline: `15930ae`.
- Existing runtimes are retired only when their complete parity rows pass. New endpoints or an empty shell are not parity.

## Review focus

1. A revoked/suspended identity or tenant must not regain access through background work, cached UI, plugin RPC or a pooled DB connection.
2. An interrupted deployment or timed-out provider action must not duplicate resources on reconnect/retry.
3. Existing encrypted secrets, IDs, permissions and retained volumes must survive migration and rollback.
4. Local socket placement and remote network transport must enforce the same installation/generation/action scope.
5. Missing/disabled/stale runtime capabilities must produce honest unavailable states rather than fabricated dashboard data.

## Execution ledger

Track commands, results and remaining parity in `docs/verification/2026-10-19-rust-platform.md`. The old source is a behavior oracle, not a permanent backend proxy. Record deviations and reasons there. Tests appropriate to each task run before commit; integration acceptance runs on owned disposable databases/containers only.

## Tasks

### 1. Workspace and core contracts

Files: root `Cargo.toml`, `rust-toolchain.toml`, `.gitignore`; `crates/core/{Cargo.toml,src/lib.rs,src/id.rs,src/features.rs,src/config.rs,tests/contracts.rs}`.

Interfaces: `RecordId::parse(&str) -> Result<RecordId, ContractError>` with JSON string-only serialization; `FeatureState::{Enabled,Draining,Disabled}`; `FeatureSet::require(feature, mutation_kind) -> Result<(), ContractError>`; `PlatformConfig::from_map(&HashMap<String,String>) -> Result<PlatformConfig, ContractError>`.

- [ ] Write negative/positive tests for Snowflake overflow/numeric JSON, unknown features, draining admission and canonical HTTPS/loopback origins.
- [ ] Run tests against missing/stub behavior, implement the contracts, run `cargo test -p webdock-core` and format/lint.
- [ ] Commit the tested contracts and workspace. Resolve dependency versions into the shared lockfile.

### 2. Loco API runtime and persistence boundary

Files: `crates/control-plane/{Cargo.toml,src/main.rs,src/lib.rs,src/app.rs,src/config.rs,src/http.rs,src/db.rs,tests/http.rs}`, `config/rust/`, `tools/rust/`.

Interfaces: Loco `Hooks` implementation; `build_app(config, db) -> Router`; `GET /healthz`, `GET /readyz`; compatible host/CSRF/error middleware. `scope_transaction` supplies an explicitly verified principal on a restricted connection for domain operations.

- [ ] Pin live Loco/SeaORM APIs from installed source; test canonical host rejection, origin rules, unavailable DB readiness and absent forwarding trust.
- [ ] Implement real Loco boot, injected SeaORM connection and offline-only migration entrypoint. Verify health and readiness against disposable PostgreSQL.
- [ ] Confirm runtime cannot migrate/drop tables and fails closed on missing role/schema prerequisites.

### 3. Identity and account lifecycle

Files: `crates/identity/` and explicit SQL migration files under `crates/identity/migrations/`; Control Plane auth routes in `crates/control-plane/src/auth.rs`.

Interfaces: `IdentityService` issues/revokes host-bound server sessions; verified `Principal` carries canonical local user ID and freshly checked permissions. Preserve existing auth endpoints and OAuth/OIDC/MCP issuers/audiences.

- [ ] Inventory and capture existing credential/hash, cookie, MFA/passkey, grants and token contracts.
- [ ] Port local login/logout, setup/invites/reset, MFA/passkeys, revocation and existing signed/encrypted formats using maintained crates. Audit migration of external identity mappings without email linking.
- [ ] Port OAuth/OIDC, consent, PKCE, introspection and MCP auth. Test expiry, cross-origin/session theft, stale roles, recovery and old/new credential fixtures.

### 4. Registry and tenant access

Files: `crates/control-plane/src/registry/`, `crates/control-plane/src/access/`, `crates/control-plane/tests/registry.rs`.

Interfaces: existing `/v1/operator/registry/:collection[/:id]` envelopes and field names; database tables `webdock_admin.customers/projects/audit_events`; `webdock_admin.next_snowflake()` allocation; existing tenant/project access contracts.

- [ ] Port CRUD/search/pagination/archive with immutable project ownership, atomic audit and RLS.
- [ ] Compare old/new responses using disposable fixtures; test denial and rollback through real HTTP and DB roles.

### 5. Plugin contracts, runtime and admission

Files: `crates/plugin-protocol/`, `crates/plugin-runtime/`, `proto/webdock/plugin/v1/plugin.proto`, `crates/control-plane/src/plugins/`.

Interfaces: `PluginRuntime.Describe/Health/Invoke/WatchOperation`; versioned manifest declarations/actions; existing signed invocation context and host service envelopes. Typed standard capabilities plus schema-validated provider-specific payloads.

- [ ] Capture manifest/signature/generation/grant contracts and encode compatible validation with round-trip fixtures.
- [ ] Implement actual Tonic servers/clients over owned Unix sockets and mTLS, with deadlines/limits/cancellation and signed action scope. Run real local socket tests plus certificate/generation denial.
- [ ] Port registry/source-install lifecycle, host services, API/MCP exposure, jobs/events and isolated UI launch contracts; retain bounded HTTP compatibility for old installed plugins.

### 6. Supervisor, agents and execution adapters

Files: `crates/agent/`, `crates/supervisor/`, `deploy/helm/webdock-plugin/`, `deploy/compose/`, Rust integration tests under each owning crate.

Interfaces: typed desired/observed runtime specifications and durable command IDs/generations; `RuntimeAdapter` for inspect/apply/stop/logs; enrolled agent connection with scoped execution leases. No arbitrary command field.

- [ ] Port existing agent ownership/revision/fencing policy and separately privileged helpers.
- [ ] Implement Docker Engine and scoped Kubernetes/Helm adapters; preserve resources on disable and respect Kubernetes restart ownership.
- [ ] Test real disposable containers and Kubernetes workloads, retained storage, idempotent reconnect, revoked leases, remote enrollment and local-only operation.

### 7. Hosting, builds and durable work

Files: `crates/control-plane/src/hosting/`, `crates/build-worker/`, supervisor persistence/operation modules.

- [ ] Port existing Git/image/source publication, reservations, quotas, environment, domains, deploy/restart/rollback, operation/outbox and log/metric contracts.
- [ ] Port build isolation/publication and trusted image binding; keep builds outside customer runtime trust boundaries.
- [ ] Verify create → build → publish → health-gated rollout → observe → rollback with interruption/retry and stale authority tests.

### 8. Mail, database gateway, CMS, storage and provider plugins

Files: `crates/mail/`, `crates/database-gateway/`, Control Plane product modules, separate `plugins/` packages.

- [ ] Port exact Mail/outbox/JMAP/relay, database access/revocation, CMS bridge and storage contracts with preserved data/encryption.
- [ ] Isolate existing provider-specific implementations outside Core, add Lumaserv as a separate generic-contract acceptance plugin without live provisioning.
- [ ] Verify product-specific integration tests and platform feature enable/drain/disable behavior across API/MCP/worker.

### 9. React Console and plugin UI

Files: `apps/console-ui/` during migration, shared browser-only components/contracts; replace Console runtime after route/flow parity.

- [ ] Implement React/Tailwind shell, workspace/app context, accessible navigation/command palette and real generated API queries.
- [ ] Port existing business screens, own-account/security screens, deployments/logs/metrics with real loading/empty/stale/error states.
- [ ] Implement versioned declarative tables/forms/actions and isolated custom UI bridge; test malformed messages, origins, revocation, accessibility and light/dark/mobile layouts.

### 10. Data migration, distribution and retirement

Files: `deploy/`, `tools/rust/`, release workflows, public developer guides and parity ledger.

- [ ] Adopt existing SQL migration history and restricted-role bootstrap without replaying or dropping data. Rehearse retained-data upgrade and rollback on sanitized fixtures.
- [ ] Build Rust binaries/images and React assets; verify Helm/Compose/local Docker paths, offline upgrades and protocol-version overlap.
- [ ] Retire old first-party backend entrypoints only after each contract/flow passes; retain external CMS/database/mail engines as dependencies.
- [ ] Run full regression, security-boundary, real browser, runtime and release checks, then fresh independent review. Close all important findings before declaring completion.

### 11. Static quality requirements added by the owner on 2026-10-20

Files: root Cargo/Clippy policy, `tools/quality/`, Console ESLint/TypeScript configuration, `.github/workflows/quality.yml`.

- [x] Check Claude's handover against actual source and distinguish completed migration primitives from missing production composition.
- [x] Enforce reasoned Rust lint policy, formatting, adapter feature isolation and standalone provider parity.
- [x] Enforce strict typed frontend checks, exhaustive control flow and bounded nesting; prefer guard clauses and small functions.
- [x] Add tested project-specific architecture and strict-deserialization rules using maintained parsers.
- [x] Run the complete local quality gate and independent review; record limits in the verification ledger.
- [ ] Keep subsequent implementation green under the same gate. Replace remaining generic domain commands/results with concrete types as each domain is completed; preserve intentional schema-validated plugin payloads.

The quality gate does not replace the database, browser, runtime, migration and product parity acceptance above.

### Incoming Mail executor follow-on (2026-10-20)

The native incoming administration slice records desired state and configuration work. The next
slice executes only instance activation/resume and retained suspension. Domain/mailbox/DKIM/grant
configuration execution remains a separate acceptance gate; queued configuration is not readiness.

1. Add strict bounded Mail DTOs alongside the existing agent protocol and Mail RPC methods on the
   same authenticated channel. Every request reuses the current enrollment, cluster generation and
   transport peer. Core retains no Docker socket, Kubernetes credentials or arbitrary manifest API.
2. Native migration `0023` records per-operation immutable request provenance, the enrolled generation,
   private claim token, encrypted material and exact accepted checkpoint/completion fingerprints.
   Existing migrations remain frozen; `0022` remains reserved for database bindings. Existing running
   operations are not adopted. Expired work becomes `needs_review`; retries require explicit operator
   reconciliation and the same retained customer instance.
3. Claim and guard use Hosting's authenticated signed agent transaction. Initial activation creates
   one encrypted bootstrap credential; an uncertain checkpoint never regenerates it. Permanent
   instance credentials are checkpointed before recovery mode is removed and remain immutable across
   resume. No plaintext credential enters public status, logs or general command journals.
4. Checkpoint and completion callbacks bind the exact request fingerprint, operation, customer,
   desired revision, lease and agent generation. Exact previously accepted receipts may be
   acknowledged after response loss. Stale callbacks cannot install credentials, mark readiness or
   renew permission; failure reconciliation changes only the still-matching revision.
5. The daemon uses its existing private state directory and a separate durable Mail callback outbox.
   Unknown/revoked Mail delivery never blocks Hosting dispatch or heartbeats. Runtime mutations
   require a live guard before each step; callbacks replay exact bytes without rerunning mutations.
6. Reuse maintained Bollard/Kube clients and existing ownership/storage protections. Each customer
   has an explicitly owned instance/network/storage boundary. Kubernetes uses the restricted
   high-port Community Edition profile. Docker is an explicitly configured local integration
   runtime, not an implicit production Mail placement. Suspension stops execution and retains
   storage/configuration; deletion is never inferred.
7. Acceptance uses a fresh restricted database and owned internal Docker network with the pinned
   real Community Edition engine: Core → enrolled daemon → bootstrap checkpoint → recovery disabled
   → verified readiness, exact callback replay after lost acknowledgement, resume without credential
   replacement, retained suspension and enrollment revocation/ownership denial. Run restricted
   Kubernetes manifest checks and actual owned Kind acceptance separately. No production resources
   or default Kubernetes context are used.

### Native Mail configuration execution (2026-10-20)

The reviewed lifecycle checkpoint is complete for the isolated Docker composition and bounded
node-installed Kubernetes runtime. Configuration is the next implementation slice, not a claim
that public Mail delivery or the whole product is complete. The owner already authorized execution.

**Source contracts:** `modules/mail/src/server/native-configuration.ts`,
`native-submission-configuration.ts`, the managed agent's Mail configuration implementation,
`crates/mail/src/incoming/configuration.rs`, and the native Webmail/grant checks.

**Scope:** domains, mailboxes, DKIM, user identities and mailbox grants. Preserve the existing
Community Edition Group/User model and JMAP permission allowlist. Do not enable sending, relay
routes, submission hooks, provider credentials or public network access as a side effect.
Submission configuration is a separate workflow with provider-binding dependencies.

1. **Core provenance and typed contracts.**
   - Reserve additive native migration`0024` for configuration admission and callback custody;
     `0022` remains reserved for database bindings and frozen`0023` must not change.
   - Add `native_configuration_operation` and bounded per-resource handoff/checkpoint records.
     Persist immutable customer/cluster, original subject/session/`StoredAuthority`, exact
     configuration/service revisions, request/material hashes, encrypted material and runtime UID.
   - Human admission requires current owner/admin/operator management permission. Historical
     pending/running operations without native provenance remain unclaimed; explicit operator
     retry records fresh authority and current verified resource references.
   - The membership-detach trigger also creates null-subject jobs. Add restriction-only provenance
     for new such jobs; it may remove existing access, never create identities/resources or grant
     permissions. Do not fabricate a user session or adopt arbitrary historical desired state.
   - Add strict DTOs in `agent-protocol/src/mail_configuration.rs`: bounded nonsecret snapshot,
     separate credential material, claim/lease, per-resource handoff/checkpoint and completion.
     Preserve string IDs/revisions and the retained limits:32 domains,64 mailboxes,128 identities,
     256 enabled grants,1MiB–8GiB mailbox quotas and8GiB total. Validate DKIM using maintained
     parsing code; preserve existing keys rather than regenerating them.

2. **Core queue and authorization.**
   - Add `mail/incoming/configuration_agent.rs` with focused authorization/snapshot modules.
     Reuse the existing enrolled-agent worker transaction, customer-before-service lock order,
     signed scope and current membership reads; no broad new runtime-role privilege.
   - Claim only current, ready, enabled service/configuration revisions. Revalidate original
     human authority or explicit contraction-only provenance before every authorized step.
   - Bound leases by original authority. Check the database clock after blocking prerequisites
     and before committing outcomes; callback replay never renews a lease. Storage/auth failures
     remain unavailable and do not become durable permission-denial acknowledgements.
   - Generate identity credentials once, encrypted by existing `MailCipher` with retained
     associated-data purposes. Persist external handoff before mutation and provider references
     only after acknowledgement/readback. Unknown creates/password changes cannot be blindly
     repeated; ambiguous unmarked domains require explicit review, not adoption by name.
   - Final completion alone advances applied configuration/grant revisions. Confirmed intermediate
     references may be retained while resources remain pending. Superseded revisions cannot
     publish readiness. Gateway grant revocation remains immediate during drain.

3. **Agent execution and transport.**
   - Extend the existing Mail control boundary on the same UDS/mTLS enrollment and generation.
     Coordinate `proto/webdock/agent/v1/agent.proto`, agent-control dispatch, management and outbox
     changes with the Core owner before editing shared files.
   - Use a separate durable configuration callback outbox so failed delivery does not block
     heartbeat or other domains. Advertise configuration capability only when executable.
   - Extend the existing bounded Reqwest management client and typed CE methods. Do not add a
     second client stack or import the Mail database domain into the agent.
   - Inspect and bind the actual current normal owned runtime/profile before credential use.
     Never invoke lifecycle Ensure/bootstrap from configuration. A lifecycle checkpoint may name
     a replaced recovery container and is not a universal current-runtime binding.
   - Apply conservative Group/User permissions, quota and membership changes with readback.
     Preserve messages, stable credentials and DKIM keys. Sending permission remains excluded.

4. **Acceptance and review.**
   - Begin with failing protocol/domain tests, then real restricted PostgreSQL tests for atomic
     original authority, cross-customer/preview denial, historical-job refusal, contraction-only
     membership cleanup and role/session/domain-ownership revocation.
   - Exercise suspension/revision races, lease expiry behind locks, infrastructure failures,
     changed callback/runtime/provider references and exact checkpoint/receipt replay.
   - Compose actual Core, enrolled daemon and the reviewed isolated CE engine: create a domain,
     mailbox and identity; validate DKIM/readback; prove granted JMAP access and immediate denial
     after revocation. Drop engine replies and callbacks, restart the daemon, and prove no duplicate
     resources or credential rotation. No external email or production resources.
   - Assert that configuration does not mark submission ready or configure relay/hook/provider
     state. Test additive migration upgrade/repeat and runtime provenance mutation denial.
   - Run the existing strict quality gate and fresh independent reviews for Core, agent and the
     composed flow. Apply/freeze`0024` on the owned shared fixture only after these checks.

Implementation ownership is split into Core/migration, agent CE executor and composed acceptance.
Shared protocol changes are owned by the Core implementer until the first contract checkpoint;
the runtime implementer can independently inspect and test CE behavior meanwhile.

### Next Mail recovery and Webmail slices (2026-10-20)

The reviewed configuration checkpoint is accepted locally. Native migrations through0024 are frozen; subsequent schema changes use a new migration. The broader Mail gate remains open.

1. Inspect the current initial-Ensure uncertainty path and the retained reconciliation contract. Define a typed, explicitly authorized inspection/recovery operation for an initially unverified instance. Persist Core runtime identity across operations independently of expiring checkpoints. Never convert an unknown result into readiness, recreate an uncertain instance, rotate retained credentials or adopt an unbound runtime automatically.
2. Implement that contract in the Core and private agent protocol with original authority, customer/service revisions, exact runtime/material binding, bounded leases and durable callback custody. Validate expired initial admission, missing/foreign/partial/normal instances, revoked original sessions, runtime replacement and lost responses against fresh restricted PostgreSQL.
3. Implement corresponding Docker and node-installed Kubernetes inspection/recovery in the agent using the existing runtime profile, retained UID, permanent authentication and TLS checks. Inspect before mutation; any continuation requires fresh exact authorization. Verify retained data and no blind bootstrap/recreation on owned local fixtures.
4. Inspect native Webmail launch/exchange, gateway and retained React contracts. Wire the Console to the native browser-safe launch flow with explicit grants, preview restrictions, session/customer isolation and configured-origin/CSP enforcement. Keep secrets and management credentials out of URLs and JavaScript persistence. Implement only real native behavior, including visible denial/unavailable states.
5. Exercise actual Console/Core/daemon/engine recovery and Webmail launch where supported, and separately compose Core with the verified Kubernetes runtime. Review new authority and browser boundaries, run the strict integrated gate and update the parity ledger without claiming public delivery or production cutover.
