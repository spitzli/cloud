# Plugin developer platform implementation plan

> **For agentic workers:** Use `superpowers:executing-plans` task-by-task; delegation requires authorization in the execution session. This is a required companion to the [runtime plugin plan](2026-10-16-runtime-plugins.md), not a separate alternative host implementation.

**Goal:** A developer scaffolds, develops, tests and publishes a plugin using a stable SDK with real DB, Cache and other host-service APIs, without reading Webdock internals.

**Architecture:** Public SDK contracts are shared with the host and CLI; backend clients call scoped host services and UI hooks use the existing isolated bridge. The host implements persistence, cache, background work and service adapters with the runtime plan's grants, generation fencing and retained lifecycle.

**Tech Stack:** Existing TypeScript/Bun/Node 24, React, PostgreSQL 18, optional existing Redis, configured object storage, Kubernetes/Helm, OCI and Tabularium. Two publishable packages: `@spitzli/plugin-sdk` and `@spitzli/cli`.

**Spec:** [Developer platform design and Tabularis study](../specs/2026-10-16-plugin-developer-platform-design.md), plus [runtime design](../specs/2026-10-16-runtime-plugins-design.md).

## Global constraints

- All runtime-plan constraints continue to apply. The document-only/fixed dataVersion limit is replaced by the developer spec's scoped DB and declarative migrations, not by raw SQL access.
- No private workspace imports in published packages or generated plugins. No duplicate independent manifest/permission/method definitions.
- No server tokens/secrets in UI exports, globals, URLs, source maps or `postMessage`. Runtime/user context is invocation-local and verified by the host.
- DB/cache isolation includes installation, customer and project; backend types and a cache prefix alone do not establish authorization.
- The exact service semantics, limits, CLI commands, migration operations and initial slots are defined in the linked spec. Implement them or explicitly report the task incomplete; never substitute a success-returning stub.
- Registry/forge/OCI publication and production mutations are explicit commands; creation/build/pack/test do not invoke them implicitly.
- Preserve all current uncommitted project work and use isolated fixtures. Local mock helpers never count as end-to-end evidence.

## Review focus

1. Types can look correct while the host ignores scope or permits unsupported methods; negative conformance must execute actual host methods (DX1–DX6).
2. Transactions/migrations/retries can leak tenant data or duplicate writes, especially through constraints and mutated idempotency payloads (DX2).
3. Cache outages, expiration and revocation must not become data/authorization errors or cross-tenant hits (DX3).
4. Generated packages often work only inside the monorepo; test packed artifacts in an empty directory without workspace links (DX1, DX8–DX9).
5. Browser slots, dev watch and publish can bypass lifecycle/trust or leak secrets; exercise stale frames, failed reload and explicit remote targets (DX7–DX9).

## Dependency order with the core plan

Use the following combined order: Core Tasks 1–6 → DX1–DX3 → Core Tasks 7–8 → DX4–DX6 → Core Task 9 → DX7 → Core Task 11 (distribution/fixtures) → DX8 → Core Task 10 (CLI-generated pilot) → DX9 → Core Task 12 (whole-flow acceptance). Introduce a focused fixture helper earlier if an integration test needs it, then reuse it; do not implement a second controller, queue or auth path. Core Task 1 establishes public contract files; DX1 completes SDK runtime and package behavior.

## Work map

| Location | Ownership |
| --- | --- |
| `packages/plugin-sdk/src/{contracts,server,ui,testing}/` | Public schemas/types, runtime wrapper, service clients, UI bridge/hooks and explicit testing helpers |
| `packages/plugin-cli/src/`, `packages/plugin-cli/templates/` | One CLI with authoring/lifecycle subcommands and versioned templates |
| `apps/control-plane/src/modules/plugins/services/` | Server-authoritative DB/cache/jobs/events/config/secret/object/HTTP/audit methods |
| `apps/control-plane/src/plugin-migrator.ts` | Separate narrow DDL process; never executes plugin-supplied code |
| `packages/platform-db/src/schema/plugins.ts`, numbered API migrations | Additional migration journal/outbox/cache/object metadata and plugin-owned table policy machinery |
| `tools/plugins/{conformance,sdk-parity,developer-e2e}.ts` | Host/SDK compatibility, fixture orchestration and empty-directory proof |
| `docs/developers/plugins/` | Quickstart, API reference, CLI reference, permissions and compatibility documentation |

## DX1: Deliver the public SDK and method catalog

**Files:** Create `packages/plugin-sdk/{package.json,tsconfig.json,README.md}`, `src/contracts/{manifest,methods,errors,capabilities}.ts`, `src/server/{index,runtime,context,transport}.ts`, `src/testing/index.ts`, tests and package build config; modify the pure plugin exports in `packages/module-sdk`; create `tools/plugins/sdk-parity.ts` and register new package tests in root tooling.

**Interfaces:** `definePlugin(definition)`, `defineAction({ input, output, intent, handler })`, `defineJob({ input, output, handler })`, `createPluginServer(plugin, verifiedRuntimeConfig)`, `PluginContext`, `HostServiceCatalog`, `PluginError`. Action/job type inference must follow schemas; `/contracts` is runtime-neutral and `/server` exports no browser globals.

- [ ] Add compile-time tests for inferred input/output, invalid service arguments and a browser import of server-only API; add runtime tests for malformed requests/responses, unsupported capability/protocol, missing host context, config failure and simultaneous invocations with different scopes.
- [ ] Run the new package tests and observe relevant failures before implementation.
- [ ] Implement a versioned method catalog shared by schema validators, public clients and host dispatcher; generate protocol docs from it. Use existing validation/build dependencies where suitable. Publish compiled ESM/types and JSON schemas, with no unresolved private workspace dependency.
- [ ] Implement authenticated HTTPS invocation handling, `AbortSignal`/deadline propagation, explicit initialize/ready/drain semantics, typed errors and idempotency-aware retries. Host grants remain authoritative; SDK checks improve errors but cannot replace them.
- [ ] Run `npm run test -w @spitzli/plugin-sdk`, `npm run check -w @spitzli/plugin-sdk`, `npm pack --workspace @spitzli/plugin-sdk --pack-destination <owned-temp-dir>` and install the tarball in an empty test project. Verify `/contracts` and `/ui` bundles contain no private/server credential modules. The helper must create/clean its own temporary directory.
- [ ] Add `sdk-parity` to CI: method names, versions, schemas, slots and exports must agree with host implementation; adding a type-only fake API must fail. Review and commit only owned changes if appropriate in the execution session.

## DX2: Implement relational DB, atomic batches and safe migrations

**Files:** Create `packages/plugin-sdk/src/server/db.ts`, `src/contracts/{database,migrations}.ts`, `apps/control-plane/src/modules/plugins/services/{db,db-query,db-policy,migrations}.ts`, `apps/control-plane/src/plugin-migrator.ts`, `apps/control-plane/tests/plugins-{db,migrations}-rls.integration.test.ts`; extend numbered SQL migrations, offline role bootstrap, policy/readiness and controller migration checkpoints. Add dedicated migrator script/image/chart settings with credentials absent from API/controller/plugin pods.

**Interfaces:** `defineSchema(tables)`, `defineMigration({ version, checksum, steps })`; `ctx.db.table(name).findOne/findMany/insert/update/delete`; `.op.insert/update/delete` builds typed serializable commands; `ctx.db.transaction(commands, { idempotencyKey })` executes one host transaction. Typed predicates are bounded `eq/ne/in/lt/lte/gt/gte/isNull`, deterministic ordered pagination, no SQL string escape hatch. `applyPluginMigrations(operationID)` runs only verified declarative steps under the separate role.

- [ ] Write real PostgreSQL tests for two plugins/two customers/two projects, same row IDs across scopes, cross-scope foreign-key/unique violations, forged table/predicate/scope, SQL injection text, RLS under restricted roles, read-only action write denial and forbidden core-table access.
- [ ] Add atomic rollback, lost-response retry, same-key/different-body conflict, 32-operation/1 MiB/100-row/five-second limits, concurrent update, migration crash/journal checksum, forbidden destructive step and compatible/incompatible executable rollback tests.
- [ ] Register tests in the explicit integration/RLS sets and run to a meaningful failure using disposable PostgreSQL 18.
- [ ] Implement host-generated physical tables in `webdock_plugins`, signed database scopes and forced RLS with scoped PK/FK/unique definitions. Compile validated query descriptions into parameterized SQL using existing PostgreSQL helpers. Reject unknown tables/columns/operators; no plugin `Pool`, DSN or unrestricted SQL is exposed.
- [ ] Extend bootstrap with a plugin-object owner/migrator role that owns only plugin objects and the narrow journal interface. Apply declarative create/add-nullable-column/index/FK steps transactionally after drain; no live arbitrary SQL/JS hooks. Persist schema history separately from executable release version and require declared readable schema range for rollback.
- [ ] Invalidate the plugin's cache generation on committed schema change, retain data on uninstall, and integrate migration failure/retry with the single lifecycle controller. State explicitly that additive DDL remains after executable rollback.
- [ ] Run restricted-role integration, type and compatibility tests to PASS. A table API backed only by JSON blobs does not satisfy this task.

## DX3: Add a shared, scoped Cache API

**Files:** Create `packages/plugin-sdk/src/server/cache.ts`, `apps/control-plane/src/modules/plugins/services/{cache,cache-postgres,cache-redis}.ts`, `apps/control-plane/tests/plugins-cache.integration.test.ts`, contract tests and a cache TTL/quota migration. Reuse Bun Redis transport patterns from `src/cache.ts` without changing its public-view behavior.

**Interfaces:** `ctx.cache.get<T>(key) -> { hit: false } | { hit: true, value: T }`; `set(key, value, { ttlSeconds }) -> { stored: boolean }`; `delete(key) -> { deleted: boolean }`; `getOrSet(key, loader, { ttlSeconds }) -> T`. Host returns permission errors distinctly from availability/miss.

- [ ] Test cross-plugin/customer/project isolation, simultaneous same keys, expiration at exact TTL boundary, quota/size rejection, DB/Redis outage, malformed backend value, revocation despite cached entry and cache generation after schema update.
- [ ] Run focused tests before implementation. Both selected real adapters must pass the same contract suite; an in-memory fake is not proof.
- [ ] Implement PostgreSQL TTL storage/cleanup as baseline and the optional configured Redis adapter. Host prefixes/validates keys, enforces grants/quotas, and applies 1–86400-second TTL. No raw Redis commands, locks or cache-based authorization.
- [ ] Implement `getOrSet` in the SDK as load-on-miss with explicit no-exactly-once guarantee, requiring read/write grants and write intent before its loader runs. Read actions use `get` plus uncached fallback. It may tolerate service outage but must propagate permission/validation errors. Document stale-data behavior and invalidation.
- [ ] Run cross-replica integration and verify a backend outage still lets the plugin load authoritative DB data. No new Redis service is automatically provisioned.

## DX4: Expose jobs and plugin-local events through code APIs

**Files:** Create `packages/plugin-sdk/src/server/{jobs,events}.ts`, `src/contracts/events.ts`, `apps/control-plane/src/modules/plugins/services/{jobs,events}.ts`, tests and outbox migration; extend the Core Task 7 scheduler/dispatch/execution tables rather than creating a second queue.

**Interfaces:** `ctx.jobs.enqueue(name, payload, { idempotencyKey, delaySeconds? })`, `ctx.jobs.cancel(executionID)`; `defineEvent({ name, version, schema })`, `ctx.events.emit(name, payload, { idempotencyKey })`, `ctx.events.emitOp(...)` for inclusion in `ctx.db.transaction`.

- [ ] Test scoped enqueue/cancel, undeclared/foreign job, excessive delay/payload, revoked job grant, running cancellation, duplicate event delivery and DB-write/outbox rollback under one transaction.
- [ ] Run failing tests, then implement schema validation, same-plugin/scope topics, durable handler grants, outbox-to-job fanout and idempotent receipts. Preserve 60-second leases, 20-second renewals, eight attempts and existing drain/generation behavior.
- [ ] Verify loss of a response after commit and controller restart do not duplicate effects; no job assumes a browser session or broad operator identity. Export stable job/event status/errors through SDK and operator inspect/logs.

## DX5: Add configuration, secrets and operational APIs

**Files:** Create `packages/plugin-sdk/src/server/{config,secrets,logging,audit,project}.ts`, corresponding host `services/` files and tests; extend the existing grant/configuration/secret-reference flows and audit composition.

**Interfaces:** immutable `ctx.config`; `ctx.secrets.get(name)`; `ctx.log.debug/info/warn/error(message, fields?)`; `ctx.audit.emit(event, fields, { idempotencyKey })`; scoped public `ctx.project` and `ctx.actor`. No client-supplied actor for audit.

- [ ] Test config defaults/validation, exact named secret allowlist, two tenants using the same logical secret name, revoked reference, no UI import, redaction, log quotas and attempted core/operator audit impersonation.
- [ ] Run tests to failure, implement real host-backed resolution through existing secret facilities with a dedicated plugin purpose, and never expose other products' encryption keys. Configuration changes follow the lifecycle/config revision path.
- [ ] Verify failures return typed errors without raw secrets or provider credentials; read-only actions cannot mutate settings or emit unauthorized audit events. Document log retention/rate limits and do not claim redaction makes arbitrary plugin code trusted.

## DX6: Implement bounded object storage and outbound HTTP ports

**Files:** Create `packages/plugin-sdk/src/server/{storage,http}.ts`, host `services/{storage,storage-s3,http,transfer-tickets}.ts`, scoped object metadata and quota migrations, integration tests and reusable local object/HTTP fixtures under `tools/plugins/`.

**Interfaces:** `ctx.storage.put/get/delete(key, ...)` using host-issued bounded transfer tickets and authenticated streaming endpoints; `ctx.http.request(bindingName, { method, path, headers?, body? })`. Exact byte/stream return types live in shared contracts. Grant identity is never part of user-supplied parameters.

- [ ] Test prefix traversal, foreign scope/key/ticket, quota races, 10 MiB object bound, body hash mismatch, interrupted upload, duplicate commit/delete, revoked grant during transfer and retained objects after uninstall. Test named HTTP binding, disallowed method/path/header, redirect authority change, DNS rebinding, private-address policy, timeout and secret redaction.
- [ ] Run against disposable real object storage and HTTP fixtures before implementation; registry public-network protections must remain active outside explicit fixture policies.
- [ ] Implement a genuine S3-compatible port with host-held credentials and authenticated transfer endpoints; do not count existing inventory adapters as object storage. Bound streaming without buffering full objects or sending them through iframe messages. Wire explicit streaming/ingress limits (the current Bun server's global body cap is smaller than 10 MiB), preserving stricter JSON/action limits. Recheck grant/generation at final upload commit and deny revoked access on subsequent chunks/requests as applicable; already delivered bytes cannot be revoked.
- [ ] Implement outbound broker binding policy using the existing validated fetch rules. Credential attachment occurs in the host where configured; do not create unrestricted network egress or proxy cloud metadata. Required unavailable storage/HTTP binding blocks activation; optional missing services return `capability_unavailable`.
- [ ] Run service conformance, quota race and outage recovery tests; document adapter configuration and independent host service ownership when optional customer Storage/Hosting are disabled.

## DX7: Add typed UI slots and browser SDK hooks

**Files:** Create `packages/plugin-sdk/src/ui/{index,bridge,slots,hooks}.tsx` (use `.ts` for non-JSX), `src/contracts/slots.ts`; extend `apps/console/src/components/plugins/` with slot registry/renderers and update the concrete project overview/actions/operator-settings mount sites found by `rg`. Extend the Core Task 9 bridge rather than introducing window-global host APIs.

**Interfaces:** `defineSlot<S extends SlotName>(name, component)` with `SlotContextMap`; hooks from the developer spec. Contributions carry manifest ID/order/relative UI entry; parent sends only serializable authorized context. `usePluginAction` infers request/response from the plugin's generated action contract.

- [ ] Add compile-time wrong-slot-context tests, deterministic ordering, unknown slot/capability rejection, drain/update cleanup, duplicate registration, frame teardown/timeout, locale/theme changes and denied cross-origin messages.
- [ ] Add browser tests proving project panels/actions and operator settings are real mount sites, disabled plugins lose customer/custom runtime surfaces but retain host-rendered schema repair settings with zero plugin replicas, and no token or host DOM/API is exposed.
- [ ] Implement iframe-backed slots, bounded host-native button descriptors and structured toast/dialog/settings operations. Bundle plugin React inside its frame; do not use `eval`, `new Function`, remote React component imports or arbitrary HTML in the Console.
- [ ] Implement non-secret user-editable settings through existing auth/CAS/lifecycle APIs; tenant UI cannot change installation capabilities, shared configuration or grants. Translation assets remain plugin-scoped and signed via the image digest.
- [ ] Add the spec's `usePluginFiles` host-owned upload/download flow with declared action checks, native picker/confirmation, streaming routes, verified object-metadata callback and temporary upload cleanup. Test 10 MiB transfer outside `postMessage`, canceled selection, rejected/revoked grant, failed attachment action and metadata-only frame results. Host transfer/session tokens must never enter plugin UI.
- [ ] Run SDK/host parity and Console/browser checks. Add a public example of each initial slot with documented guaranteed fields.

## DX8: Build the CLI and the full authoring loop

**Files:** Create `packages/plugin-cli/{package.json,tsconfig.json,README.md,src/cli.ts}`, `src/commands/{create,dev,validate,test,build,pack,publish,lifecycle,doctor,migrate,auth}.ts`, versioned `templates/{backend,fullstack}/` and tests; add `apps/control-plane/src/modules/plugins/{cli-auth,logs}.ts`, scoped token storage migration and operator mint/revoke/inspect/log routes plus Console controls. Connect existing `tools/plugins` fixture/build/release helpers. No template imports from `apps/control-plane` or unpublished workspace paths.

**Interfaces:** Commands/flags and output semantics are fixed in the developer spec. Add exit codes `0` success, `1` execution failure, `2` invalid input/config, `3` policy/compatibility blocker; `--json` emits structured result/error with redacted diagnostics. `--version` derives from package metadata. Use Node's argument parsing if sufficient; no framework dependency solely for dispatch.

- [ ] Write CLI tests for invalid/traversal IDs, path escape/symlink/nonempty target, whitespace paths, no accidental shell expansion, offline scaffold, invalid schema/SDK, missing dependency/adapter, credentials in JSON output and denied implicit production actions.
- [ ] Add a packed-package test that scaffolds a plugin outside the repo, installs tarballs from a local fixture, compiles/tests it and validates every generated manifest/slot/schema/CI reference. Do not rely on global CLI installs, repository `node_modules`, or publishing to npm to make this pass.
- [ ] Implement create/validate/test/build/pack/doctor/migrate-check with shared contracts and real project commands. Generate backend/fullstack templates with selectable declared service examples, no fake success handlers, lockfile guidance, Dockerfile and release workflow. Do not execute dependency installation scripts automatically during `create`.
- [ ] Implement `dev` using the explicit owned local Kubernetes/registry fixture: watch/rebuild plugin, create a new dev release/digest, reconcile, and preserve host process/digest. A failed build leaves the prior working version active. On Ctrl-C clean only task-owned resources, preserve the reusable fixture when requested and never change the user's kube context.
- [ ] Implement the spec's scoped opaque CLI-token mint/revoke in the authenticated operator Console and service. Add `POST /v1/operator/plugin-cli-tokens` and `DELETE /v1/operator/plugin-cli-tokens/:id`; raw token appears only in the issuing response, hashes alone are stored. Validate bound live identity/session, expiry and scopes for each bearer request, including under Better Auth/Clerk's existing local identity mapping. Preserve browser CSRF/origin enforcement and test wrong audience/host, tenant token, preview, expired/revoked session and read-only mutation denial.
- [ ] Implement auth login/logout and lifecycle/inspect/logs against the same operator services with explicit host-bound private profile, CAS/idempotency and blockers. Add `GET /v1/operator/plugins/:id/logs?cursor=...&limit=...` with a maximum 100 redacted entries per page and existing operator read authorization. No browser cookie/password scraping, token in argv, direct kube access or bypass of current operator authority.
- [ ] Implement explicit publish with OCI push plus configured forge descriptor upload plus scoped Tabularium publication. `--dry-run` makes zero remote mutations and prints targets/digests. Wait for verifiable integrity, handle partial upload retries idempotently, reject changed same-version content and never put tokens in argv/logs. Generate CI with pinned tool versions and scoped credential inputs; public execution requires separate authorization.
- [ ] Run CLI tests, packed template smoke tests, real local dev reload and local registry publication tests. Verify all commands in `--help` actually exist; `migrate --check` must not perform remote DDL.

## DX9: Prove the developer contract and document it

**Files:** Create `tools/plugins/{conformance,developer-e2e}.ts`, `docs/developers/plugins/{quickstart,sdk,services,cli,permissions,compatibility}.mdx`, a short internal verification record and docs navigation entries. Modify Core Task 10's pilot to use only the generated project and public SDK.

- [ ] From an empty directory, install packed CLI/SDK, scaffold Project Notes, add its real schema/cache/job/event/attachment/slot features, run `doctor`, `validate`, `test`, `build`, `pack`, publish locally and install through the operator flow. Record exact artifacts and commands.
- [ ] Exercise a schema-1 to schema-2 nullable-column update, preserved old notes, cache invalidation and compatible executable rollback; reject a destructive migration and incompatible readable-schema range. Run two customers/projects concurrently with the same logical IDs and cache/object keys.
- [ ] Compile documented TypeScript examples, test supported host/SDK version combinations, detect method/schema/export drift, and assert unsupported capabilities fail instead of silently downgrading. Use real services for integration; keep mocks explicitly labelled for unit tests.
- [ ] Complete the Core Task 12 runtime failure matrix with SDK/CLI calls as well as Console. Capture dev reload without host restart, stable UI context after update and rejected stale grants/frames. Treat a local script that bypasses the CLI/SDK as insufficient developer-experience proof.
- [ ] Run package tests/checks, all relevant core integration/RLS suites, packed artifact tests, distribution builds and required docs validation. Record skipped environments honestly and leave production deployment/public publication unclaimed.
- [ ] Update both plan progress ledgers and architecture status only against evidence. Completion means the CLI and host-service implementations work together; generated types, a catalog and empty adapters alone do not satisfy the request.
