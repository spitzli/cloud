# Webdock Hosting Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Work natively in this session; no subagents, commits or PRs without a separate instruction.

**Goal:** Deliver the first independently testable hosting increment: customer hosting allowances, transactional reservations, tenant authorization, a durable operation ledger, cluster enrollment/heartbeat, and matching Studio/REST/MCP access.

**Architecture:** Keep the hosting authority beside the existing Auth customer membership and subscription services, using additive `webdock_auth.hosting_*` tables and the same database transaction for subscription revision checks and reservations. Studio exposes the product UI and public REST/MCP facades through a fixed authenticated bridge; it does not acquire Auth database credentials. A cluster agent will consume bounded durable operations in the subsequent application-delivery increment.

**Tech Stack:** Node 24, TypeScript, Next.js 16.3.8, PostgreSQL/pg, Better Auth, the installed MCP SDK, existing gettext package. No infrastructure dependency is needed for this foundation.

**Spec:** [Approved hosting architecture](../specs/2026-10-08-hosting-control-plane.md). User approved the written spec on 2026-10-08. This implementation plan still requires review before product changes.

## Global Constraints

- All platform IDs are decimal Snowflake strings; never convert IDs to JavaScript numbers.
- Everything must run in the EU, preferably Germany, including builds, registry, storage, backups and control services. Existing Vercel Frankfurt functions do not prove the whole control plane EU-only.
- Production credentials and customer data must not enter an unverified hosting control path. Local development remains possible before the production location gate is satisfied.
- Node 24. Read installed Next documentation and applicable AGENTS.md before framework edits.
- Preserve current uncommitted changes; no automatic worktree checkout from HEAD that drops the working baseline.
- Existing Registry and `currentMCPClaims` remain operator-only. Existing CMS-instanceID and Auth-bindingID remain distinct from hosting IDs.
- Customer-view preview remains read-only and expires after 15 minutes; exit it after browser verification.
- Destructive operations remain operator-only through Webdock, API and MCP. No direct provider/SQL deletion fallback.
- English source/fallback, German gettext catalogs; do not translate customer data.
- Only local disposable test databases. No production reset, schema push, remote build, commit or PR.
- No unsupported provider limit may be labeled hard-enforced. Unknown usage is not zero.

## Scope boundary and delivery map

The approved architecture has four independently testable increments. This plan implements increment 1 in full. It does not claim to deliver a running customer container before increment 2.

| Increment | Result | Prerequisite for the next detailed plan |
| --- | --- | --- |
| 1, this plan | Actual persisted cluster inventory, enrollment/heartbeat, customer limits, atomic reservation service, audited authorization, UI/API/MCP | Tests and interface contracts below pass |
| 2 | Cluster agent plus real image/template deployment, scale/restart/rollback, logs, secrets, domain/TLS and persistent volume | Observe the installed k3s version, network and storage capabilities; use increment 1 services |
| 3 | Vercel usage/capability integration and enforced Webdock action limits, drift detection | Verify actual provider APIs, permission scopes and billing-unit semantics |
| 4 | Backups/restores, maintenance, additional/dedicated clusters, EU build/registry pipeline | Choose and verify concrete EU infrastructure; test recovery |

Do not expose deploy/scale controls as working in increment 1. An enrolled cluster can be connected without being ready for customer workloads. A heartbeat alone is never a successful isolation/storage/TLS test.

## Review Focus

1. Two concurrent reservations or a simultaneous plan revision must never spend the same allowance twice (Task 3).
2. A token can be valid while membership is revoked, preview is active or the customer is archived; every entry point must deny the relevant action (Tasks 2 and 5).
3. An ambiguous worker result or expired lease must retain reservations until reconciliation, and a stale report cannot overwrite newer state (Task 4).
4. Missing, zero and explicitly unlimited allowances have different meanings; old plans must not implicitly enable hosting (Tasks 1 and 3).
5. Cluster observations can be stale, fabricated or belong to another cluster; neither capacity nor readiness may be inferred from them (Tasks 4 and 6).

## File and responsibility map

Create `packages/hosting-contracts/{package.json,src/index.ts,src/index.test.ts}` for pure JSON contracts and validation shared by Auth, Studio and the future agent. Use the installed Zod version; no server imports.

Create Auth modules under `apps/auth/src/lib/hosting/`:

- `schema.ts`: additive SQL, indexes and database constraints.
- `authorization.ts`: live subject/session/membership/project checks.
- `allowances.ts`: hosting allowance normalization and inherited caps.
- `reservations.ts`: locked budget/capacity allocation and release proofs.
- `clusters.ts`: operator inventory, enrollment and bounded observations.
- `operations.ts`: durable lifecycle, leases and idempotency.
- `service.ts`: fixed dispatcher composing the above; no caller-controlled SQL or executable operation name.
- `bridge.ts`: native verification for Studio delegation and MCP tokens.
- `agent-api.ts`: enrollment and authenticated cluster observation transport.

Modify Auth `plans.ts`, `studio-api.ts`, `mcp.ts`, `auth.ts`, `app/api/mcp/introspect/route.ts`, existing consent/client registration code and metadata as required by the call-site inventory. Add `app/api/hosting/bridge/route.ts` and `app/api/hosting/agent/[...path]/route.ts` as thin adapters.

Create Studio `lib/hosting-client.ts`, `lib/hosting-api.ts`, `lib/hosting-mcp.ts`, `lib/hosting-actions.ts`, and `app/api/hosting/[[...path]]/route.ts`. Compose hosting tools in the existing `/api/mcp` transport; customer tokens must not instantiate registry tools.

Create operator pages `(console)/(workspace)/infrastructure/page.tsx`, `/infrastructure/[clusterID]/page.tsx`, `/customers/[id]/hosting/page.tsx`; customer page `(console)/(portal)/tenants/[customerID]/hosting/page.tsx`. Extend existing navigation and plan forms. Share forms, status rendering and list primitives under `components/hosting/`; use existing `ListControls`/`Pagination` with backward-compatible status-option support.

Create tests `apps/auth/tests/hosting-{allowances,authorization,reservations,operations,clusters,bridge}.test.ts`, `apps/admin/tests/hosting-{api,mcp,ui}.test.ts`. Add regressions to existing Auth MCP/plans/preview tests and Studio registry/MCP tests. Migration script: `apps/auth/scripts/migrate-hosting.ts`.

## Task 1: Exact contracts, compatible plan extension and schema

**Produces:** `HostingDimension`, `HostingAllowances`, `HostingActor`, `HostingCommand`, `HostingResult`, input schemas; pure `normalizeHostingAllowances(value)` and `effectiveHostingAllowances(base, extras)`; `hostingSchemaSQL`.

- [ ] Read local Next route-handler, server-action and data-security docs. Record current git diff and installed Node version. Inspect runtime database grants through local metadata only; use Auth-owned hosting tables rather than new cross-schema runtime writes.
- [ ] Define the shared contract with these exact units and status values:

```ts
export const hostingDimensions = [
  'apps', 'cpuMillicores', 'memoryBytes', 'volumeBytes',
  'ephemeralBytes', 'replicasPerApp', 'concurrentDeployments',
] as const;
export type HostingDimension = typeof hostingDimensions[number];
export type HostingAllowances = Record<HostingDimension, number | null>;
export type Enforcement = 'provider-enforced' | 'webdock-enforced'
  | 'observed-only' | 'unsupported';
export type HostingActor = {
  subject: string; sessionID: string;
  source: 'studio' | 'oauth'; scopes: readonly string[];
}; // Constructed only by the native authenticated bridge.
export type HostingOperationStatus = 'queued' | 'running' | 'succeeded'
  | 'failed' | 'needs-reconciliation';
```

- [ ] Write contract tests before implementation:

```ts
assert.equal(normalizeHostingAllowances(undefined).apps, 0);
assert.equal(normalizeHostingAllowances({ apps: 0 }).apps, 0);
assert.equal(normalizeHostingAllowances({ apps: null }).apps, null);
assert.throws(() => normalizeHostingAllowances({ apps: -1 }));
assert.throws(() => normalizeHostingAllowances({ apps: '10' }));
assert.throws(() => normalizeHostingAllowances({ apps: 1.5 }));
assert.throws(() => normalizeHostingAllowances({ apps: Number.MAX_SAFE_INTEGER + 1 }));
```

- [ ] Run them with `node --import tsx --test packages/hosting-contracts/src/index.test.ts`; verify missing-contract failure, then implement strict schemas and checked addition. IDs must match the existing signed-bigint Snowflake range.
- [ ] Add optional `hosting` to plan-template/subscription/history/offer JSON payloads, keeping existing five allowance keys and their semantics intact. Old payloads normalize hosting to zero. Existing clients that omit hosting on unrelated updates preserve current hosting values; an explicit reset requires an explicit hosting object. Include hosting in offer revision checks and snapshots.
- [ ] Create tables `hosting_cluster`, `hosting_enrollment`, `hosting_agent`, `hosting_project`, `hosting_limit`, `hosting_reservation`, `hosting_operation`, `hosting_observation`, `hosting_audit`. Customer/project relations reference existing canonical records with delete RESTRICT. No cascade that silently removes infrastructure inventory. Unique provider/cluster/namespace mapping prevents duplicate ownership.
- [ ] Constraints: positive revisions/generations, bounded nonnegative quantities, valid status/provider/action enums, globally unique operation IDs, unique `(subject,idempotency_key)` with payload hash, unique cluster enrollment token hash, immutable customer/project ownership. Nullable dimensions explicitly represent unlimited; absent record is denied.
- [ ] Migration script follows existing explicit scripts, runs in a transaction and never at server startup. Local tests enforce both loopback host and exact disposable DB name before schema changes. Production execution is not part of this plan run.
- [ ] Test old plan creation/edit/offer acceptance, new hosting allowance snapshots, invalid extras overflow and migration rerun against disposable DB. Run existing plans tests to prove compatibility.

## Task 2: Native authorization and hosting OAuth without opening Registry

**Consumes:** shared `HostingActor` and native session identity.
**Produces:** `authorizeHosting(actor, {customerID?, projectID?, action}, connection)` returning server-resolved owner/mode/rights; `currentHostingMCPClaims(subject)` separate from `currentMCPClaims`.

- [ ] Write authorization tests for every role/action boundary using local fixtures for two customers and one operator:

```ts
await assert.rejects(authorizeHosting(customerAActor,
  { projectID: customerBProjectID, action: 'read' }, connection));
await assert.rejects(authorizeHosting(customerAActor,
  { customerID: customerAID, action: 'limits.write' }, connection));
await assert.rejects(authorizeHosting(previewActor,
  { projectID: customerAProjectID, action: 'app.write' }, connection));
await assert.rejects(authorizeHosting(customerAActor,
  { projectID: managedProjectID, action: 'app.write' }, connection));
```

Fixture actors are constructed through the test authentication boundary, never public JSON parameters. Include disabled users, incomplete setup, expired session, revoked membership and archived customer. Return indistinguishable not-found/denied responses for foreign targets.
- [ ] Implement live lookups using the existing user/member/tenant_customer/project relationships. Operator requires current verified MFA setup; customers require active membership. Mutation checks happen within the reservation/write transaction where relevant.
- [ ] Extend OAuth with `hosting:read` and `hosting:write` using an explicitly provisioned hosting-client metadata flag. Keep existing `webdock_mcp` operator-only token issuance and `currentMCPClaims` semantics. Hosting-client refresh tokens retain the same live-session binding rule as existing MCP clients.
- [ ] Add hosting-specific introspection validation that preserves issuer, audience, active session, expiration and client checks. Subject, session, scopes and preview state must come from native verification. Do not trust a submitted actor or role.
- [ ] Compose `/api/mcp`: webdock-scoped operator access registers current registry tools; hosting-scoped access registers only authorized hosting tools. A token with both scope families may use both only after both native authorizations succeed. `validateMCPToken` used by Registry remains strict.
- [ ] Inventory all `mcpScopes`, consent and client metadata call sites with `rg`, update explicit allowlists and consent descriptions. Tests must assert customers cannot obtain/use `webdock:write` through hosting registration and hosting token refresh cannot add scopes.
- [ ] Run Auth MCP/Studio/preview and Studio registry/MCP regressions alongside new authorization tests. Failure of Auth introspection is fail-closed, never a cached permission fallback.

## Task 3: Transactional customer/provider/project reservations

**Produces:** `reserveHosting(actor, input)`, `getHostingUsage(actor, customerID)`, `setHostingLimits(actor, input)`.
**Input:** customer/project/provider/cluster target, exact dimensional demand, expected subscription/limit revision, idempotency key and canonical payload hash. Client-supplied ownership is cross-checked against the target, not adopted.

- [ ] Create a transaction fixture with 1000 CPU-millicores allowance and concurrent requests for 750:

```ts
const outcomes = await Promise.allSettled([
  reserveHosting(actor, demandA), reserveHosting(actor, demandB),
]);
assert.equal(outcomes.filter(x => x.status === 'fulfilled').length, 1);
assert.equal((await getHostingUsage(actor, customerID)).reserved.cpuMillicores, 750);
```

Use real concurrent pool clients; a mocked sequential loop is insufficient.
- [ ] Implement locking order: existing `plan:${customerID}` transaction advisory lock, then cluster lock, then target row. Plan edits/offer acceptance already use the plan lock; reuse it, not a competing hosting-only lock. Read current subscription revision after acquiring the lock. Compare expected revision before any reservation.
- [ ] Derive effective ceiling as the minimum of applicable customer, provider and project caps. Normalize null as infinity only after explicit validation. Sum active and reserved demand with overflow-safe arithmetic; no integer-to-float loss from SQL bigint conversion.
- [ ] Treat `replicasPerApp` as a per-app maximum, not a sum over all apps. Cluster-wide budget dimensions sum across projects and all current/reserved replicas. Keep measured CPU/memory separate from allocated demand.
- [ ] Persist reservation, queued operation and audit entry atomically. Repeat same actor/key/payload returns the existing operation; changed payload returns 409. Subscription or limit revision mismatch returns 409 without side effects.
- [ ] Cluster allocation requires current verified capability state and fresh observations. Inventory-only or heartbeat-only clusters are not eligible. Reject stale or insufficient capacity. Capacity accounting includes explicit platform reserve and observed non-Webdock workload reservations without double counting managed allocations.
- [ ] Implement limit editing with revision comparison. Lowering below allocations sets over-allocation and blocks positive growth; it does not delete/suspend workloads or free reservations. Zero-replica apps retain volume allocation.
- [ ] Add tests for multiple clusters sharing one customer budget, cross-provider project caps, overflow, retained PVC budget, simultaneous plan edits, idempotency races and unavailable cluster measurements. Successful usage responses carry observation timestamps and enforcement labels.

## Task 4: Durable operations, cluster enrollment and observations

**Produces:** `registerCluster`, `createEnrollment`, `consumeEnrollment`, `reportCluster`, `claimOperation`, `completeOperation`, `reconcileOperation`, `revokeAgent` in their owning modules.

- [ ] Test one-time enrollment atomically with two concurrent consumers: exactly one succeeds. Expired/wrong-cluster/revoked enrollment fails. Persist only SHA-256 of a cryptographically random enrollment token; five-minute TTL. Return the raw token once only to the authenticated operator browser setup flow with no-store headers.
- [ ] Implement MCP enrollment as a reference to that browser flow, never a bearer secret. Redemption requires the current authorized operator; the URL reference itself is not a credential.
- [ ] On enrollment, issue a random revocable agent credential bound to cluster ID; persist only its hash. Agent uses Authorization header over HTTPS. Local HTTP is allowed only in explicit loopback test/development configuration. Comparisons are constant-time, errors are redacted, bodies bounded and rate limits durable.
- [ ] Heartbeat payload includes schema version, sequence, observation timestamp, k3s version, node IDs/roles, architecture and bounded capacity/capability observations. Reject duplicate/out-of-order sequence or generation, unknown fields, unsafe quantities and reports for a different cluster. Display offline after 90 seconds without a valid observation; injected time makes tests deterministic.
- [ ] Implement claims with `FOR UPDATE SKIP LOCKED`, leased generation and target serialization. Only matching live agent/cluster/operation/generation may report completion. Bound claim count and response size.
- [ ] Exercise state transitions:

```ts
assert.equal(transition('queued', 'claim'), 'running');
assert.equal(transition('running', 'lease-expired'), 'needs-reconciliation');
assert.throws(() => transition('needs-reconciliation', 'release-without-proof'));
```

`transition` is a pure helper defined in `operations.ts`; storage methods additionally enforce leases and expected revisions. Terminal statuses cannot be arbitrarily reopened.
- [ ] Add tests proving expired lease does not release quota, stale completion cannot overwrite newer generation, duplicate valid completion is idempotent, revoked agent cannot heartbeat/claim, and cluster A cannot read cluster B operations. No test claims actual Kubernetes application success in this increment.
- [ ] Store audit actor/target/action/outcome and sanitized error codes. Prove raw enrollment and agent credentials do not appear in inventory, operation, audit or error responses.

## Task 5: REST, Studio bridge and MCP parity

**Produces:** fixed `executeHosting(actor, command)` dispatcher; shared command input/output schemas; `handleHostingRequest`, `registerHostingTools` and authenticated `hostingCall` facade.

Commands in this increment: cluster list/read/register, enrollment-reference creation, revoke agent, customer limits read/update, usage read, hosting project list/create/mode update, operation read. Reservation and lease functions remain internal until their actual producer/consumer exists; no public arbitrary “enqueue operation” endpoint.

- [ ] Define discriminated `HostingCommand` for these commands with strict schemas. Fix allowed methods/path lengths/query parameters; page size max 50. Limit JSON bodies to 64 KiB with streamed size checking, reject duplicate query keys, use no-store responses.
- [ ] Add private `/api/hosting/bridge`: native verification accepts only configured Studio confidential client and genuine delegated token or validated hosting OAuth token. The caller cannot override source, subject, session, audience, target service URL or preview state. Reuse bounded-body and native OAuth patterns; do not forward cookies.
- [ ] Public `/api/hosting` uses bearer scopes; browser forms use the existing delegated-session pattern through server actions. They invoke the same dispatcher and transactional authorization.
- [ ] Test the same limits mutation via REST and MCP:

```ts
assert.equal((await restSetLimits(customerToken, input)).status, 403);
assert.equal((await mcpSetLimits(customerToken, input)).isError, true);
assert.equal((await restSetLimits(readOnlyOperatorToken, input)).status, 403);
assert.equal((await restSetLimits(operatorWriteToken, input)).status, 200);
assert.deepEqual(await restReadLimits(customerToken), await mcpReadLimits(customerToken));
```

These test adapters invoke real handlers/MCP client transport with fixed fixture authentication. Do not expose them as runtime helpers.
- [ ] Register hosting tools without registering Registry tools for customers. Add missing tools to mutation scope checks. Tool descriptions disclose capability/inventory limitations. Correct readOnly/destructive/idempotent hints; creation requiring idempotency is genuinely replay-safe.
- [ ] Read operations by authorizing their current target ownership, not just matching submitted operation ID. Unknown foreign IDs return no identifying details.
- [ ] Run negative origin, spoofed actor, session expiry, scope escalation, oversized body, cross-tenant pagination, token redaction and existing Registry regression tests.

## Task 6: Useful Studio screens and package management

**Consumes:** only safe shared result types and `hostingCall`/server actions; no direct browser access to infrastructure credentials.

- [ ] Add operator Infrastructure list/detail with explicit unconnected/connected/stale/revoked state, node count, version, verified location, observation age and separate workload-readiness capability status. Add registration/enrollment browser flow with one-time display and credential download redaction protections.
- [ ] Add customer hosting tab and operator customer quota page: package allowance, project/provider sublimits, reserved/allocated/measured values, enforcement label and last observation. Missing measurements render “Unavailable”, not zero. Show revision conflict without discarding unsaved input.
- [ ] Extend both Auth and Studio plan field components and shared `studio-contracts.ts` to include hosting allowances in templates, customer assignment and offers. Prevent legacy clients from clearing new fields. Show null explicitly as “Unlimited”, zero as zero; inputs accept fixed units with exact conversions.
- [ ] Add managed/selfservice mode and permitted-image policy to hosting project form. Default existing/unconfigured customers to hosting disabled. Display the project action permissions rather than assuming mode alone grants them.
- [ ] Reuse shared list controls/pagination; add backward-compatible explicit status options and a shared table renderer for hosting lists. Do not undertake unrelated migration of all existing lists during this increment.
- [ ] Translate source messages through existing gettext workflow:

```sh
npm run extract -w @spitzli/i18n
npm run compile -w @spitzli/i18n
npm run catalog:check -w @spitzli/i18n
npm test -w @spitzli/i18n
```

After extraction, author German entries before compilation. Update only owning catalogs, preserving unrelated translations.
- [ ] UI tests assert no customer/preview mutation controls, supported keyboard labels, unavailable usage, no fictitious deploy action and no provider credentials in rendered HTML. Auth/API tests remain authoritative even if a disabled control is bypassed.

## Task 7: Full verification and first-node handoff

- [ ] Use Node 24 and verify the exact local disposable database URL host and database name in test bootstrap without printing the URL. Do not execute the existing full test suites until both Admin and Auth test environment guards are confirmed.
- [ ] Run new pure tests, focused local DB tests and affected existing plans/MCP/Registry/preview tests. Then run:

```sh
npm run check -w @spitzli/admin
npm run check -w @spitzli/auth
npm run catalog:check -w @spitzli/i18n
git diff --check
```

Separate pre-existing failures from new failures with file/line evidence. Do not claim passing builds if the local required environment is unavailable. Do not fall back to remote builds.
- [ ] Open T3 preview with `preview_status`, then `preview_open` if needed. In real local operator session: create cluster inventory, assign finite customer allowance, create hosting project, obtain one-time enrollment, submit a local simulated agent heartbeat, see observed state. Mark simulated heartbeat evidence as simulated; never describe it as a live k3s integration.
- [ ] In real local customer session: see own hosting limits/project, verify managed restrictions, attempt forged foreign target and operator-only mutation. In operator customer-preview mode verify read-only state, then exit preview.
- [ ] Validate equivalent REST/MCP reads and mutation denials with local issued OAuth credentials without logging token values. Confirm a hosting customer token cannot list Registry customers or call delete_project.
- [ ] Document result in `docs/architecture/hosting-control-plane.md` with implemented/local-tested/live-tested/unavailable distinctions and commands. Link approved spec and this plan. Describe rollback as disabling new routes/agent credentials; do not drop tables or delete customer resources.
- [ ] Prepare node handoff instructions that request only nonsecret facts (provider/region, k3s version, architecture, CPU/RAM, StorageClass, ingress/DNS) and a private installation path. Enrollment manifests and real agent installation belong to increment 2, not an invented heartbeat-only success.

## Definition of done for this increment

Operators can save a cluster and finite customer hosting limits; customers can see only their own hosting records; persisted state is available consistently in UI/API/MCP. Concurrent reservations and plan edits are verified against real local PostgreSQL. Cluster enrollment is one-time, revocable and isolated. Unconnected/stale/capability-unknown states are explicit. No existing customer, package, Registry permission or Vercel project changes implicitly. The next increment starts from these tested contracts and adds the actual cluster executor and app delivery.

## Plan self-review

- Existing Auth MCP operator checks remain unchanged for existing clients; new hosting-client scopes receive separate native claims validation.
- Reservation correctness uses the existing subscription lock, covering concurrent offer acceptance as well as deployment requests.
- Hosting authority shares the subscription database without granting Studio a second privileged database identity.
- Secrets are excluded from MCP and normal records; enrollment flows return a browser reference only.
- All five review-focus conditions are assigned tests above. Remaining architecture increments are explicitly tracked rather than represented as completed by this foundation.
- Execution recommendation: native in this session, because Auth, contracts, Studio and MCP have tightly coupled interfaces and the repository contains extensive uncommitted work.

## Execution result — 2026-10-08

Foundation implemented and locally verified. See `docs/architecture/hosting-control-plane.md` for delivered behavior, 77 Auth + 90 Admin tests, typechecks, browser/HTTP proof and explicit remaining increments. The user subsequently supplied the live Contabo node and chose to leave Studio/Auth on Vercel; a read-only SSH observation bridge connects that node to the isolated local preview only. No production deployment or server mutation occurred. Work remains uncommitted.
