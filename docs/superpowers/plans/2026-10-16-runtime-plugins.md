# Runtime plugins implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:executing-plans` to implement this plan task-by-task. Use `superpowers:subagent-driven-development` only if the execution session authorizes delegation. Steps use checkbox (`- [ ]`) syntax for tracking. This handoff requests implementation by a different agent; it does not start implementation in the planning session.

**Goal:** Make existing module availability operator-controlled at runtime, then create, install and operate a genuinely external plugin through the public SDK/CLI and Tabularium without rebuilding or restarting Webdock.

**Architecture:** Tabularium supplies verified release metadata; Webdock persists installation decisions and enforces capabilities. A dedicated Plugin Controller reconciles isolated Kubernetes workloads through the existing agent, while a generic Console route embeds plugin UI on a separate origin. Existing modules remain bundled during this first delivery.

**Tech Stack:** Existing TypeScript/Bun/Elysia Control Plane, Next.js 16.3.8/React Console, PostgreSQL 18 with restricted roles/RLS, Python Kubernetes agent, Helm, OCI images, Tabularium. Reuse installed `jose`, `zod`, database, audit, lease and identity facilities.

**Spec:** [Runtime plugins design](../specs/2026-10-16-runtime-plugins-design.md). Read the full spec before implementation; it owns the lifecycle, trust, data and browser contracts.

**Required companion:** [Plugin developer platform plan](2026-10-16-plugin-developer-platform.md) and its [design/Tabularis study](../specs/2026-10-16-plugin-developer-platform-design.md). The owner explicitly added scaffolding CLI and code APIs for DB/Cache and other services. Complete DX1–DX9 as part of this delivery. Follow the combined dependency order in that plan; in particular, package distribution/fixtures before the CLI-generated pilot. Do not execute this older runtime sequence alone and call the expanded request complete.

## Global constraints

- Preserve the current uncommitted modularization; no reset, clean, mass staging or accidental commits of other work.
- Preserve existing npm names, protocol URLs, decimal Snowflake IDs, customer/project identities, provider bindings and encryption keys.
- Kubernetes is the production runtime. Core agent transport and plugin management must not require customer Hosting to be enabled.
- Availability is `enabled | draining | disabled`; package installation, runtime health and operation state are separate.
- Core state is database-backed; security decisions never depend solely on environment variables, notifications or cached UI.
- V1 supports trusted/operator-approved publishers, one installed version per plugin, installation-level availability and separately authorized customer/project use.
- Manifest/descriptor maximum 256 KiB, public fetch timeout 10 seconds, maximum three validated redirects, iframe message maximum 32 KiB, plugin document maximum 64 KiB, invocation token maximum 60 seconds, scheduled interval minimum 60 seconds.
- The public capability catalog and service limits are owned by the developer-platform spec. `dataProtocolVersion` is 1; relational `schemaVersion` and readable range are separate. Only generated declarative migrations are allowed. No raw SQL/code migrations, host imports, shell commands, Helm charts or Kubernetes manifests from plugins.
- Deactivation/uninstall retain data. Unresolved operational obligations block full shutdown. No purge operation in v1.
- Read applicable `AGENTS.md`, `@RTK.md` if present, and installed Next.js routing/security documentation before editing framework code. Record an absent instruction file; do not invent its contents.
- No production deploy, customer migration, external release publication or secret rotation is implied by this plan. Prepare and test deployment artifacts locally.

## Review focus

1. A disabled-at-start module must become usable on every owning API/worker without restart; startup omissions and unawaited async guards are likely regressions (Tasks 3–4).
2. A stale replica or lease must not admit new work after drain or complete work after losing authority; accepted continuations must still finish safely (Tasks 2–4, 7).
3. A signed but wrong release, changed same-version manifest, malicious download redirect or substituted image must not execute (Tasks 5–7).
4. A plugin iframe/runtime must not inherit host cookies, select another customer, replay a revoked grant or reach privileged cluster services (Tasks 6–9).
5. Crash/retry/update/uninstall must retain documents and avoid duplicate jobs, broken rollback or dependence on an online registry (Tasks 7, 10–12).

## Work map and milestones

| Milestone | Tasks | Independently reviewable result |
| --- | --- | --- |
| A: live built-in modules | 1–4 | Operator control with persisted state, distributed admission and safe worker behavior |
| B: external plugin | 5–10 | Verified Tabularium installation, isolated runtime, authenticated UI and Project Notes |
| C: distribution and proof | 11–12 | Reproducible local end-to-end evidence and production-ready configuration/runbook |
| Required developer platform | DX1–DX9 in the companion plan | Publishable SDK, actual DB/Cache/services, typed UI slots, CLI create/dev/test/build/pack/publish and empty-directory proof |

New code locations are proposed paths; existing locations below were inspected. Recheck them before editing. Keep implementations in the stated owning package, even if a focused file split is needed.

| Location | Responsibility |
| --- | --- |
| `packages/plugin-sdk/src/contracts/` and compatible `packages/module-sdk` re-exports | One public schema/type source for the host, CLI and plugins; pure lifecycle rules remain in module SDK |
| `packages/plugin-sdk/src/{server,ui,testing}/`, `packages/plugin-cli/` | Required public developer APIs, typed slots and CLI from the companion plan |
| `packages/platform-db/src/schema/plugins.ts` | Physical installation, operation, grant and plugin-document schema mappings |
| `apps/control-plane/src/modules/plugins/` | State/admission, registry adapter, lifecycle, capability API, transport and agent composition |
| `apps/control-plane/src/plugin-controller.ts` | Dedicated durable reconciliation process |
| `apps/control-plane/src/protocols/plugins.ts` | Authenticated operator/tenant/runtime route composition |
| `apps/console/src/app/(console)/(workspace)/admin/plugins/` | Operator list, inspect and lifecycle controls |
| `apps/console/src/app/(console)/(portal)/tenants/[customerID]/plugins/[pluginID]/[[...path]]/` | Generic scoped plugin UI route; align with the actual tenant route group if moved |
| `agents/kubernetes/plugin_{client,executor}.py` | Restricted agent protocol and generated workload reconciliation |
| `examples/plugins/project-notes/` | Independently built pilot, outside host workspaces and imports |
| `deploy/plugins/`, `tools/plugins/` | Registry/runtime configuration, packaging and local lifecycle verification |

## Shared interfaces

Implement these contracts in Task 1; later tasks must consume the same definitions. Do not replace the existing pure `ModuleManifest`/`createRegistry` API with network access.

```ts
type PluginID = string; // existing slug rules; built-in/core IDs are reserved
type Revision = string; // non-negative decimal bigint encoded as a string
type Capability = import('@spitzli/plugin-sdk/contracts').Capability;
type Availability = 'enabled' | 'draining' | 'disabled';
type Intent = 'read' | 'write' | 'provision' | 'continue';
type SessionActor = { subject: string; sessionID: string };
type OperatorActor = SessionActor; // operator status is rechecked, not asserted by this alias
type PluginScope = { customerID: string; projectID: string };
type Mutation = { expectedRevision: Revision; idempotencyKey: string };
type Blocker = { code: string; count?: number; message: string };
type OperationRef = { id: string; status: 'pending' | 'running' | 'succeeded' | 'failed' | 'blocked' };
type PluginTarget = { kind: 'builtin' | 'external'; id: PluginID };
type InstalledPlugin = {
  id: PluginID; kind: PluginTarget['kind']; revision: Revision;
  desiredState: Availability; state: Availability;
  version: string; imageDigest: string | null;
  runtime: 'absent' | 'starting' | 'ready' | 'stopped' | 'failed';
  blockers: Blocker[];
};
type VerifiedPluginRelease = {
  registryURL: string; id: PluginID; version: string;
  manifestBytes: string; manifestSha256: string;
  image: string; signerKeyID: string; manifest: PluginManifest;
};
```

`PluginManifest` is the validated Tabularium core plus the spec's top-level runtime fields. Keep it separate from an installed record. Validate serialized JSON at every trust boundary. Additional internal row/checkpoint types may be defined by their owning task; public naming and semantics above stay stable.

## Task 1: Define the v1 contract and lifecycle rules

**Files:** Establish the publishable `packages/plugin-sdk` skeleton and its `src/contracts/{manifest,capabilities,methods,errors}.ts` in coordination with DX1; keep pure lifecycle rules/tests in `packages/module-sdk/src/plugin-lifecycle.ts` and SDK manifest tests in `packages/plugin-sdk`. Modify module SDK exports to re-export public contracts compatibly. Create `tools/plugins/tabularium-kind.schema.json` and a valid/invalid manifest fixture directory. Do not create a competing private manifest implementation.

**Interfaces:** Produce `parsePluginManifest(input: unknown): PluginManifest`, `checkPluginCompatibility(manifest: PluginManifest, coreVersion: string, installed: readonly InstalledPlugin[]): Blocker[]`, and `planPluginTransition(current: InstalledPlugin, desired: Availability, blockers: readonly Blocker[]): { state: Availability; desiredState: Availability; blockers: Blocker[] }`.

- [ ] Add tests named `rejectsUnknownPrivilegedFields`, `rejectsUnpinnedImage`, `rejectsDuplicateAndCyclicDependencies`, `rejectsUnsupportedProtocolOrDataProtocol`, `rejectsInvalidSchemaCompatibilityRange`, `blocksDependencyRemovalOrUpgradeWhileDependentsAreActive`, `keepsDisableDrainingWhileResourcesRemain` and `doesNotEnableWithoutReadyRuntime`. Assert all spec bounds and distinguish retained data from retained operational obligations. Built-in activation checks its process owners and does not require an external container.
- [ ] Run `npm run test:modules`; observe failures in the new contract tests before implementing them.
- [ ] Implement the pure contract using existing validation tools. Pin `protocolVersion`, `sdkVersion` and `dataProtocolVersion` to 1; validate positive `schemaVersion`, readable schema range and ordered checksummed migration declarations from the developer spec. Use exact dependency versions in v1. Reuse a suitable installed SemVer implementation if present; otherwise add one maintained small dependency rather than invent range parsing. Register public SDK tests in the root test command.
- [ ] Generate the per-kind extension schema from the same definitions, within Tabularium's supported schema subset; add a check that generated and checked-in schema agree. Do not allow extension drift or arbitrary `$ref` fetching.
- [ ] Run the same tests to PASS. Inspect only task-owned changes and create a scoped commit if commits are part of the execution session.

## Task 2: Persist state, initialize once and enforce RLS

**Files:** Create `packages/platform-db/src/schema/plugins.ts`, `apps/control-plane/src/modules/plugins/{store,bootstrap}.ts`, `apps/control-plane/tests/plugins-state-rls.integration.test.ts`, `tools/plugins/bootstrap.ts`, and the next unused numbered `apps/control-plane/drizzle/NNNN_plugin_state.sql`; modify `apps/control-plane/scripts/lib/install-policies.ts`, `apps/control-plane/src/db/readiness.ts`, `packages/database-runtime/src/index.ts`, `tools/migrations/{install.ts,install.integration.test.ts}`, and the existing policy sources. The migration loader already discovers numbered SQL files; do not change it unless a demonstrated need requires that. Register the integration test in `apps/control-plane/scripts/test.ts`'s RLS set.

**Interfaces:** Produce `initializePluginState(selection: unknown, mode: 'fresh' | 'legacy'): Promise<void>`, `readInstalledPlugins(): Promise<InstalledPlugin[]>`, `requestPluginState(actor: OperatorActor, id: PluginID, desired: Availability, mutation: Mutation): Promise<OperationRef>`. Database access must receive existing authenticated/scoped context, never create a fake operator.

- [ ] Add restricted-role tests: duplicate bootstrap is a no-op or explicit conflict without overwrite; legacy missing configuration preserves six enabled modules; fresh explicit selection disables omitted products; tenant cannot read secrets or mutate installations; forged worker scope fails; concurrent same-revision mutations yield one success; repeated key/same payload yields the same operation and changed payload yields 409.
- [ ] Create isolated fixtures using the repository fixture scripts, then run the new RLS test through `npm run test:rls -w @spitzli/api`; confirm a relevant failure. Never aim the fixture runner at a customer database.
- [ ] Add `plugin_installations`, `plugin_releases`, `plugin_operations`, `plugin_grants`, `plugin_documents`, `plugin_job_executions` and `plugin_participants` using existing schema/ID/journal conventions. Persist desired/actual availability, exact verified release, current/candidate/previous runtime generation, approvals, redacted config/secret refs and recoverable operation checkpoints. Unique active lifecycle operation per installation; revisions and idempotency are enforced transactionally. Job slots/leases and per-document mutation receipts follow the spec.
- [ ] Define RLS for installation administration, read-only public projection, narrow controller/agent purposes and document access. Bind document keys to plugin installation, customer and project; prohibit caller-supplied scope escalation. Table owners and offline migrators stay separate from runtime roles. Extend readiness to verify required policies, not just table existence.
- [ ] Implement explicit bootstrap/import under a singleton migration lock. Keep immutable baseline artifacts stable; add journaled migrations instead of rewriting deployed baseline checksums. API startup fails with a useful setup message if initialization is missing.
- [ ] Run RLS tests and empty-install/retained-upgrade tests to PASS. Record the cutover procedure and task-owned diff before committing.

## Task 3: Replace cached admission and startup-only routing

**Files:** Create `apps/control-plane/src/modules/plugins/{admission,public-view}.ts` and `apps/control-plane/tests/plugins-admission.integration.test.ts`; modify `src/{installation,config,index,app,module-preflight}.ts`, `src/protocols/{legacy-auth,integrations,billing,billing-tenant,polar-hosted}.ts`, `src/modules/portal/{studio-api,hosting}.ts`, `src/modules/mcp/{mcp-server,hosting-mcp}.ts`, and all module admission factories/callers discovered below. Preserve module-owned interfaces rather than importing application code into `modules/`.

**Interfaces:** Produce `withModuleAdmission<T>(id: PluginID, intent: Intent, run: (context: AdmissionContext) => Promise<T>): Promise<T>` and `readPublicInstallation(): Promise<ModuleView>`. `AdmissionContext` carries authoritative revision and accepted operation identity where applicable. Transaction sharing/reentrancy belongs to this implementation; nested multi-product checks acquire locks in sorted ID order.

- [ ] Inventory every caller with `rg -n 'installationRegistry|publicInstallation|requireInstalledOperation|requireMail|validateWorkerModules|config\.modules' apps packages modules`. Save the affected-file list in the verification record. Audit synchronous `require*` port signatures and direct/legacy/Studio/MCP paths.
- [ ] Add real two-connection tests: a stale enabled snapshot cannot provision after drain commits; accepted setup work continues during drain; shutdown waits for in-flight accepted intent; database failure refuses new privileged work; nested Hosting/Database/Mail checks do not deadlock. Register DB tests in the correct explicit test-runner set.
- [ ] Add composed-API tests that boot with a disabled owned module, activate it, call its existing direct and Studio endpoints successfully, then disable it and verify denial without recreating the app. Cover billing webhook and MCP invocation paths, not only navigation.
- [ ] Run new tests through the existing integration/RLS runners and observe the expected failures.
- [ ] Implement shared admission/exclusive transition fencing and durable intent creation per the spec. Convert all affected ports/callers coherently; security-relevant checks cannot remain synchronous cached callbacks. Keep continuation authority distinct from fresh provision authority and reuse existing idempotency/outbox/operation journals for actual side effects.
- [ ] Mount all owned built-in routes at startup, moving availability checks to requests/services. Retain process-domain ownership checks. Replace `config.modules` and environment reads as authorization sources; public DTOs remain metadata, not authority. Reuse shutdown inventory through a callable preflight returning structured blockers without deleting anything.
- [ ] Run `npm run test:modules`, affected API integration/RLS tests and `npm run check -w @spitzli/api` to PASS. An API process losing database access must not keep authorizing from its last snapshot.

## Task 4: Make workers dynamic and expose operator controls

**Files:** Modify `apps/control-plane/src/{worker,jobs,index}.ts`, `src/modules/billing/worker.ts` and the module-owned billing worker composition, `packages/api-client/src/{index,registry-types}.ts`, `apps/console/src/components/navigation.tsx`; create `apps/control-plane/src/plugin-controller.ts`, `src/modules/plugins/{participants,operator-service,controller,lifecycle}.ts`, `src/protocols/plugins.ts`, `src/routes/operator/plugins/` route files, `apps/console/src/app/(console)/(workspace)/admin/plugins/{page,form}.tsx`, its `actions.ts`, and tests `plugins-workers.integration.test.ts`, `plugins-operator.test.ts`, `apps/console/tests/plugins-operator.test.ts`.

**Interfaces:** `GET /v1/operator/plugins`; `POST /v1/operator/plugins/:id/state` accepts `{ desiredState, expectedRevision, idempotencyKey }` and returns 202 plus `OperationRef`. Mutations use current operator identity, security-setup and preview rules. Participants advertise supported domains/protocol, heartbeat and active generation.

- [ ] Write tests for disable/enable in a running worker, stale worker ownership, lease loss, billing's separate batch loop, two workers competing for one job, missing domain ownership and an expired participant. Assert that draining allows maintenance/accepted continuations but blocks fresh provisioning; full disable reports blockers instead of losing jobs.
- [ ] Write route/UI tests for unauthorized tenant, read-only preview, stale revision 409, pending operation polling, visible blockers, and buttons that remain usable after recoverable errors. No optimistic claim of completed shutdown.
- [ ] Run the focused API and Console tests and observe failures.
- [ ] Build handlers for supported process domains independently of initial module availability, then consult authoritative state at schedule/claim/renew boundaries. Preserve domain ownership. Make long-running workers advertise protocol/readiness; live control is enabled only after the cutover removes legacy processes.
- [ ] Introduce the dedicated core controller entrypoint now, including heartbeat, lease-based operation claim and built-in state reconciliation, so milestone A has a working executor for its 202 operations. Add its workspace script. Task 7 extends this same controller with external-runtime operations; it must not create a competing state-change path.
- [ ] Add operator protocol under the core authenticated domain and Console client/actions. Follow the existing delegated-session pattern but enforce operator privilege in the service too. Render current/desired state, version, operation progress and shutdown blockers; no secret values in DTOs.
- [ ] Run API checks, module tests, affected worker/RLS tests, Console tests/check. Milestone A is complete only when activation from disabled-at-start works without API/worker restart.

## Task 5: Integrate Tabularium with verified pinned releases

**Files:** Create `apps/control-plane/src/modules/plugins/{registry,integrity,download-policy}.ts`, `apps/control-plane/tests/plugins-registry.test.ts`, `tools/plugins/configure-tabularium.ts` and `tools/plugins/fixtures/`; extend operator catalog/inspection routes. Use the spec's pinned upstream source for a disposable integration fixture.

**Interfaces:** `listRegistryPlugins(registryID: string): Promise<PluginCatalogEntry[]>`; `resolvePluginRelease(registryID: string, id: PluginID, version: string): Promise<VerifiedPluginRelease>`. `PluginCatalogEntry` contains display-only metadata and available versions. Trust configuration is operator-owned and never populated from plugin-controlled URLs.

- [ ] Add local HTTP fixture tests for correct identity/signature/hashes, missing integrity, wrong registry or version, null raw manifest, tampered descriptor/image reference, yanked release, same-version changed bytes, unknown key, malformed/duplicate-key JSON, oversize body, timeout and hostile redirects/DNS resolution. Assert `.ok` if reusing Tabularium's hash helper.
- [ ] Run the focused unit suite before implementing; network tests must use explicitly allowed loopback fixtures, while ordinary unit tests retain the repository network blocker.
- [ ] Implement the narrow validated adapter with existing `jose` and `zod`. Verify trusted JWS first, hash raw manifest and descriptor, require equality of their bytes, then parse and validate. Resolve the OCI image exclusively from verified manifest content. Pin downloaded manifest and image identity in local state; do not resolve `latest` during retries.
- [ ] Add idempotent Tabularium kind/schema configuration and a round-trip compatibility test. Fetch the actual pinned API contract, not the stale consuming example. Keep tokens out of logs; no automatic public registry writes.
- [ ] Run adapter tests and a contract test against a disposable real Tabularium instance. Registry unavailability must not change installed state or stop a running plugin.

## Task 6: Add scoped plugin capabilities and retained data

This task supplies the base authenticated gateway, document API and grants. DX1–DX6 implement the full public SDK and real host services against it; those tasks are mandatory, not represented by this task's smaller document API.

**Files:** Create `apps/control-plane/src/modules/plugins/{grants,tokens,capabilities,documents}.ts`, `src/routes/runtime/plugins/` and tenant action routes under the plugin protocol; extend `packages/database-runtime/src/index.ts` and policy installers with narrowly authenticated plugin invocation scope. Create `plugins-capabilities-rls.integration.test.ts`, `plugins-tokens.test.ts` and `packages/api-client/src/plugin-types.ts`.

**Interfaces:** `authorizePluginInvocation(actor: SessionActor, id: PluginID, scope: PluginScope, action: string): Promise<PluginInvocation>`; `authorizePluginJob(id: PluginID, scope: PluginScope, jobName: string, executionID: string): Promise<PluginInvocation>` uses a durable job grant. `PluginInvocation` includes runtime generation, immutable operation ID, effective capabilities and token. Data methods are `readPluginDocument(key: string)`, `writePluginDocument(key: string, value: unknown, expectedRevision: Revision | null)` within verified invocation context; null means create-only. Expose no arbitrary SQL or unscoped list API.

- [ ] Write tests with two customers and two projects: cross-scope read/write denied; forged grant/header denied; installation approval without customer grant denied; membership/grant revocation denies an unexpired token; wrong audience/generation/job denied; read action cannot write; repeated mutation is idempotent; data CAS rejects lost updates; disable/uninstall retain bytes.
- [ ] Run focused unit/RLS suites and confirm meaningful failures.
- [ ] Implement operator-approved installation capabilities plus explicit customer/project grants, audited grant mutation and current membership/preview checks. `PUT /v1/operator/plugins/:id/grants` replaces the operator-reviewed grant set with CAS/idempotency; each entry names customer, project, allowed capabilities and separately enabled jobs. No blanket grant on install and no unauthenticated scope selector.
- [ ] Issue bounded invocation tokens and verify workload identity separately. Use existing signing/secret patterns with a dedicated purpose; do not reuse Mail or browser session keys. Retain idempotency receipts atomically with document writes. Only host-validated invocation context can establish the database plugin scope.
- [ ] Implement the v1 JSON document API and public project read projection; redact internal metadata and secrets. Limit keys, body sizes and result sizes. Retained documents are not a runtime shutdown blocker by themselves.
- [ ] Run token, restricted-role and authorization tests to PASS. Ensure direct public access to runtime endpoints without a valid invocation is denied.

## Task 7: Implement durable installation and reconciliation

**Files:** Extend `apps/control-plane/src/plugin-controller.ts` and `src/modules/plugins/{controller,lifecycle}.ts` from Task 4; create `src/modules/plugins/{agent-protocol,scheduler,dispatch}.ts`, `apps/control-plane/tests/plugins-{lifecycle,jobs}.integration.test.ts`; extend `modules/hosting/src/server/agent-api.ts` and its Control Plane composition using the existing authenticated transport. The current `scripts/build.ts` copies the whole source tree, so verify inclusion of the controller rather than inventing a separate build system.

**Interfaces:** Operator routes `POST /v1/operator/plugins/install`, `POST /v1/operator/plugins/:id/update`, `POST /v1/operator/plugins/:id/uninstall`, `PUT /v1/operator/plugins/:id/configuration`, and `GET /v1/operator/plugins/operations/:operationID`. Install takes registry/id/exact version/inspected manifest hash, approved capabilities, validated configuration/secret references and idempotency key; update/configuration/uninstall additionally require current revision. Configuration changes drain/restart the affected runtime through the same lifecycle. `reconcilePluginOperation(operationID: string): Promise<void>` uses persisted checkpoints; `schedulePluginJobs(now: Date): Promise<void>` and `dispatchPluginJob(executionID: string): Promise<void>` implement the spec's durable scheduler. Agent actions are `plugin-claim`, `plugin-guard`, `plugin-checkpoint`, `plugin-complete`, `plugin-observation` within existing agent authentication.

- [ ] Add tests that crash/restart after each persisted phase, replay install/update requests, race two controllers, lose a lease, complete with a stale agent generation, fail dependency/config/readiness checks and retry a missing image. Every case must converge or report a recoverable blocker without overwriting another operation.
- [ ] Add update tests proving the candidate is not routed before readiness, the old runtime loses job ownership before the candidate gains it, failed compatible update restores old routing, and unsupported data protocol or incompatible readable-schema range is refused before runtime changes. Include the DX2 migrator's transactional failure and additive-schema rollback behavior.
- [ ] Add concurrent dependency tests: A activating against B@1.0.0 cannot race B's disable/uninstall/update to 1.0.1. Enabled/draining reverse dependents block the target transition and are listed in the result. No implicit cascade; disabled incompatible dependents remain disabled.
- [ ] Add external-job tests for concurrent schedulers/claimers, duplicate scheduled slot, lost response after committed data write, revoked job grant, no browser session, stale generation renewal, retry exhaustion and stopping new slots during drain. Assert the spec's lease/timeout/attempt bounds and at-least-once delivery with idempotent effects.
- [ ] Run the new integration suite through the restricted fixture runner and observe failures.
- [ ] Implement phases `verify → prepare → migrate → start → ready → activate` and `drain → stop → disabled`; install may end prepared/disabled until explicit enable. The separate DX2 migration process applies only verified declarative migrations after old writers drain; ordinary controller/plugin credentials have no DDL authority. Store candidate/previous generations, exact release identity, phase and error. Use lease/attempt/checkpoint patterns from existing operations, with no HTTP request waiting for Kubernetes completion.
- [ ] Add controller participant heartbeat, recovery and shutdown. Register operation pending status as a shutdown blocker. For uninstall remove executable resources only after drain; preserve release/history/documents and revoke runtime identity. Reinstall the same plugin identity must recover retained documents without silently restoring old grants.
- [ ] Implement the external scheduler and authenticated private runtime dispatcher, using host-generated Service destinations, cluster TLS, workload identity and Task 6 invocation tokens. Limit ordinary action/job response size to 64 KiB with a 30-second deadline; named host-service routes use the developer spec's explicit DB/HTTP/streaming limits. Record timeout/lease loss as recoverable execution state. Runtime readiness must identify expected plugin version/protocol/generation and required host-service availability, not merely return HTTP 200. Fresh actions and jobs are never sent to candidate or stopped generations.
- [ ] Implement core agent routes with full cluster, plugin, operation, lease and generation binding. Factor only shared transport needed for core independence; do not create a second unauthenticated agent connection or gate plugin commands on customer Hosting.
- [ ] Run integration and type checks to PASS. No production agent request is necessary for this task.

## Task 8: Run restricted plugin containers through Kubernetes

**Files:** Create `agents/kubernetes/plugin_{client,executor}.py`, `test_plugin_executor.py`, `test_plugin_client.py`; modify `agents/kubernetes/{daemon,observer,permissions}.py` and relevant enrollment/capability contracts; create `deploy/plugins/runtime/` policy templates and extend `deploy/helm/spitzli-agent` only for explicit plugin support.

**Interfaces:** Agent packet includes protocol version, cluster ID, installation/plugin ID, operation ID, agent/runtime generations, lease token, validated image digest, bounded resource settings and a host-selected namespace. `validate_plugin_packet(packet)` rejects extra privileged fields; `reconcile_plugin(packet, checkpoint)` returns readiness/observation proof tied to the same generation.

- [ ] Test rejection of foreign namespace/cluster, wrong digest, arbitrary command/mount/RBAC, secret overreach, stale checkpoint and oversized packet. Test idempotent apply, partial resources after crash, readiness timeout, generation-bound completion and retained uninstall.
- [ ] Run `python3 -m unittest discover -s agents/kubernetes -p 'test_plugin*.py'` and observe failures before implementation.
- [ ] Implement generated Deployment/Service/Secret/NetworkPolicy objects plus narrowly generated Certificate requests to the configured namespace-scoped cert-manager Issuer, explicit labels and allowed fields. Credentials are delivered only through the authenticated claim/checkpoint path and stored with existing secret protections. Mount per-runtime TLS credentials and the host public verification-key bundle; never a CA/signing private key. Readiness checks issuer availability, certificate DNS/expiry and trust, with no insecure fallback. Keep SSA field ownership explicit; a stale apply must not restore stopped replicas.
- [ ] Use an operator-provisioned runtime namespace with explicit default-deny policy and narrowly scoped agent RBAC; the plugin pod itself receives no API token or RBAC. Do not silently grant namespace/ClusterRole creation. Agent advertises plugin protocol and policy readiness before the controller can select it.
- [ ] Wire a dedicated reconciliation tick into the existing daemon without starving Mail/Database/Hosting work. Plugin health observation is generation-bound and does not expose credentials.
- [ ] Run Python tests and real disposable Kubernetes smoke tests, including NetworkPolicy enforcement, non-root/read-only behavior, core API capability reachability, denied arbitrary egress and stop/restart. Docker-only verification is insufficient for this task.

## Task 9: Add generic plugin UI and authenticated invocation bridge

DX7 extends this same bridge with typed project/operator slots and public UI hooks. Do not implement a second unisolated UI loader.

**Files:** Create `apps/console/src/components/plugins/{plugin-frame,plugin-bridge}.tsx`, the generic tenant plugin route in the work map, `apps/console/src/lib/plugin-actions.ts`, `apps/console/tests/plugins-{bridge,route}.test.ts`; extend the operator plugins page/actions/forms from Task 4 with catalog/install/configuration/grant/update flows; modify `src/components/{navigation,workspace-sidebar}.tsx`, `src/lib/{module-route-guard,module-data}.ts`, `packages/module-sdk/src/react.tsx`, authenticated session/client DTOs, `apps/control-plane/src/modules/plugins/{launch,invoke}.ts` and the plugin protocol.

**Interfaces:** `getPluginLaunch(actor, id, scope)` returns only `{ protocolVersion: 1, pluginID, generation, origin, uiPath, allowedActions, publicContext }`. `invokePluginAction(actor, id, scope, action, payload, idempotencyKey)` authorizes in the host and calls the private runtime. Iframe messages are `{ protocolVersion: 1, requestID, type: 'invoke', action, payload }`; reply uses the same request ID and sanitized result/error. The frame never receives backend tokens.

- [ ] Test forged `event.source`, sibling/previous-generation origins, invalid message types, oversize payload, undeclared action, tenant switch, stale tab after disable and direct deep links. Assert that no host credential or token appears in HTML, URL, messages or logs.
- [ ] Run focused Console and API tests before implementation.
- [ ] Read the installed Next.js dynamic-route/route-handler docs. Implement one generic catch-all route that resolves installed plugin metadata at request time. Keep customer/project authorization on the backend; route/nav filtering alone is insufficient.
- [ ] Require explicit project selection from authorized projects; do not pick the first project implicitly. Complete operator catalog search, signed release inspection, approval of capabilities, schema-based configuration/secret references, customer/project/job grants and install/update/uninstall progress. Submit the inspected manifest hash and revision; surface changed-release or stale-revision conflicts for a fresh review.
- [ ] Implement a sandboxed frame with exact CSP/origin checks and a bounded bridge using existing authenticated Console transport. Serve only static `/ui/` through a dedicated plugin ingress origin; block `/webdock/`, health and runtime credential endpoints from public routing. Ensure ingress normalization allows these configured UI hosts without broadening trusted Webdock origins.
- [ ] Refresh public installation metadata within five seconds while the operator/plugin page is open, and on focus/navigation. Server-side invocation must reject immediately after authoritative drain even if the view is stale.
- [ ] Run Console/API checks and browser verification through the T3 shared preview when available. Demonstrate real frame invocation, denied cross-origin messages and a stale open tab after deactivation.

## Task 10: Ship Project Notes as an independently built plugin

**Files:** Generate `examples/plugins/project-notes/` using DX8's packed CLI, including standalone package/lockfile, Dockerfile, `webdock.plugin.ts`, schema/migrations, `src/server.ts`, UI source, tests and README. Final `dist/tabularium.json` and universal descriptor are CLI build/pack outputs. Reuse the CLI's packaging implementation instead of adding a parallel `package-plugin` script. Keep the package outside host workspaces and imports.

**Interfaces:** Actions `notes.read` (read), `notes.write` (write), job `notes.maintenance` (60 seconds). UI accepts only the public parent bridge. Runtime consumes the scoped host data/project API and validates host invocation signature/audience/generation. Notes are at most 16 KiB; maintenance updates a timestamp with operation idempotency.

- [ ] Write standalone tests for authorized read/write, size limit, safe Markdown/plain-text rendering, idempotent job retry and rejected invocation identity. Run with the package's own test command and observe failures.
- [ ] Implement the editor with the public SDK: a real relational note table, additive migration, scoped cache, maintenance job/local event, attachment storage and typed project slot. Use isolated real object storage from the development fixture; no direct core database/provider credentials, mock success or hidden host callback. HTTP/secret bindings are exercised by separate conformance fixtures.
- [ ] Build an image, generate the digest-pinned manifest and identical universal descriptor, then publish into the disposable test registries using scoped test credentials. Wait for integrity metadata; an incomplete release is not installable.
- [ ] Prepare v1.0.0 at schema 1 and compatible v1.1.0 at schema 2 (a nullable-column addition), plus a test-only unhealthy candidate. Declare appropriate readable-schema ranges, prove data retention and executable rollback, and reject a destructive migration. Use fixed digest references; tests must detect changed bytes under a pinned version.
- [ ] Run standalone tests and the full install/enable/invoke/job/disable/reactivate/update/uninstall/reinstall path through Webdock. Preserve notes and revoke/reapprove grants as specified.

## Task 11: Package the controller, registry configuration and cutover runbook

**Files:** Modify `deploy/docker/Dockerfile`, `deploy/compose/compose.yaml`, `deploy/helm/webdock/{values.yaml,values.schema.json,templates/platform.yaml}`, `tools/release/{manifest.mjs,check-charts.py}` only where those files actually own distribution contracts; create `deploy/plugins/tabularium/{values.example.yaml,README.md}`, `tools/plugins/registry-fixture.ts`, `docs/operations/runtime-plugin-cutover.md`, and extend `docs/developers/modules.mdx` or add `docs/developers/plugins.mdx` with a `docs/docs.json` entry.

**Interfaces:** Explicit settings for registry URL/trust roots, allowed artifact/image origins, runtime cluster/namespace, namespace-scoped runtime TLS Issuer/CA trust, invocation verification keys, plugin UI base domain and controller enablement. Secrets use existing external secret references. No registry/domain/credential is hardcoded to a customer environment.

- [ ] Add chart/distribution checks that controller can run with all optional products disabled, runtime credentials are absent from Console/public manifests, missing trust/domain configuration fails clearly and image references are pinned. Extend existing test machinery rather than adding a parallel release framework.
- [ ] Build/package dedicated controller and restricted plugin-migrator entrypoints plus the public SDK/CLI artifacts. Use the developer plan's backend configuration and fixtures; do not share migrator credentials with controller/API/plugins. Reuse Webdock release signing/SBOM practices. Ensure standalone pilot source/images are not copied into API/Console runtime images or Next standalone traces; explicitly exclude `examples/plugins` from host build context if the broad `COPY . .` would include it.
- [ ] Pin a tested Tabularium image digest and chart revision after verifying the artifact exists and matches the tested API. Upstream Compose and Helm defaults reference different image registries; do not assume either default is the production selection. Configure persistent data, encryption/signing-key backup, external secrets, TLS and health checks. No secrets in committed values.
- [ ] Implement a disposable local Tabularium/OCI fixture with deterministic cleanup, persisted keys across restart and scoped publisher credentials. Keep it outside customer infrastructure and allow only explicit loopback/test networking exceptions.
- [ ] Provision the configured internal TLS Issuer in the disposable Kubernetes fixture and prove API/controller Service reachability, certificate hostname verification and unknown-key denial. Document colocation, cert-manager and an enforcing CNI as production prerequisites; do not install cluster-wide infrastructure implicitly during a plugin activation.
- [ ] Write cutover steps: inventory baseline; offline schema/policies; import existing selection; deploy new API/worker/controller/agent protocol; remove old participants; verify readiness/ownership; enable live operator controls. Explain restore/rollback limits and never restart on an environment value that overwrites stored state.
- [ ] Document registry key rotation, registry outage, stuck draining, unavailable agent, failed candidate, retained uninstall and recovery from partial operations. Production deployment requires the execution session's explicit target/domain authorization; missing operational inputs do not prevent code/local verification completion.
- [ ] Run `npm run test:distribution`, build checks, chart rendering and the public docs checks required by `docs/AGENTS.md` if public docs changed. Run the existing API/Console build commands; inspect image contents for accidental pilot/secret inclusion.

## Task 12: Verify the whole flow and record honest completion

Completion also requires DX9's packed-CLI/SDK empty-directory proof and actual host-service tests. The original small document-only pilot is no longer the complete acceptance scope.

**Files:** Create `tools/plugins/e2e.ts`, `docs/verification/2026-10-16-runtime-plugins.md` (use the actual execution date if different); update `docs/architecture/module-installation.md` and `modular-platform-status.md` only after implementation evidence exists. Extend the existing CI workflow under `.github/workflows/` that owns distribution tests, after inspecting its current name.

- [ ] Build host images once and record their digests, process start times and current selection X. Start two API replicas, competing workers/controllers, a real network-policy-capable local Kubernetes runtime and the isolated registries. Seed two customers/projects through existing fixture APIs, not fabricated auth headers.
- [ ] Install Y from a release published after the host build; prove Y has no host source import or image contents. Approve the intended customer grant, use the real Console frame to save/read a note, observe a scheduled maintenance result, and deny the second customer's access.
- [ ] Drain/deactivate Y while work is in flight; prove new invocations fail and accepted work converges. Reactivate and recover data. Complete a compatible update, reject an unhealthy candidate, verify rollback, uninstall/reinstall with retained data and fresh grant approval. Host digests/start times and X availability stay unchanged throughout.
- [ ] Interrupt each controller/agent lifecycle phase and restart; prove durable recovery, exclusive job ownership and rejection of stale completion. Stop Tabularium and verify installed plugins keep working while new installations show a useful failure.
- [ ] Exercise tampered manifest/descriptor, wrong signer, yanked version, wrong image digest, private-destination redirect, malicious iframe message, expired/revoked token, removed membership, DB outage, unsupported protocol and retained Mail shutdown blocker. These must fail before unauthorized effects.
- [ ] Run `npm run test:modules`, `npm run test:unit -w @spitzli/api`, the registered integration/RLS suites with isolated fixtures, `npm run check -w @spitzli/api`, `npm run test -w @spitzli/admin`, `npm run check -w @spitzli/admin`, the Python agent tests, distribution checks and API/Console builds. Use existing network-blocked suites; live fixture integration is explicitly separated.
- [ ] Record exact commands, outcomes, source/image versions and limitations without credentials. Remove only fixtures owned by this task. Do not claim skipped checks passed, production rollout, safe arbitrary third-party execution or completed extraction of all six built-ins.
- [ ] Review the final diff for authorization bypasses and retained-data risks; resolve findings. Hand off the tested code, operational runbook, remaining deployment inputs and verification record.

## Follow-up program: extract existing products

After milestone C, inspect Storage as the first extraction candidate. Create a separate per-product migration plan covering SQL/transaction ownership, provider secrets, jobs, old URLs, resource identifiers, retained data and recovery. Then prioritize the remaining products based on their actual dependency graph; Mail/Hosting require dedicated operational migration work. Do not turn all existing SQL ports into unrestricted RPC, duplicate transactions across processes, or promise universal cloud providers as part of this plan.

The later core-only distribution must remove static product imports and copied product source from API/Console images, preserve necessary shared core agent transport, and pass a test that a previously absent product can be installed from Tabularium. Until then describe the delivered result as live built-ins plus external plugins.

## Suggested instruction for the implementing agent

> Implement `docs/superpowers/plans/2026-10-16-runtime-plugins.md` AND its required companion `docs/superpowers/plans/2026-10-16-plugin-developer-platform.md`, plus both linked designs, in the current worktree while preserving existing uncommitted changes. Follow their combined dependency order and complete Core milestones A–C and DX1–DX9 with real local verification. Read applicable repository instructions and installed Next.js docs. Track progress, resolve routine implementation choices and record deviations with evidence. Do not deploy to production, publish externally, purge data or migrate customer databases unless this execution session explicitly authorizes those actions. Completion requires the public scaffolding/dev/build/pack/publish CLI, typed SDK with real DB/Cache/host-service implementations, isolated UI slots and a CLI-generated independently built plugin, including background work and retained lifecycle, without rebuilding the host. Toggles, a registry catalog, generated types or success-returning service stubs alone are insufficient.
