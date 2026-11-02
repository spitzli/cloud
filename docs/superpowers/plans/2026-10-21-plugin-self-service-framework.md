# Plugin self-service framework implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Step 1 of the product plugin design. Plugins declare offerings, self-service features and audiences. Operators choose an access mode per feature. Customers get a project area that shows plugin resources and an "Add" menu limited to what they may use.

**Architecture:** Manifest protocol 2 in `crates/plugin-protocol`. Three new Core tables, plus gates added to the existing admission path in `crates/plugin-host/src/authority.rs`. `plugin_grant` stays as an internal table that admission fills automatically, so the RLS functions and foreign keys that join it keep working. The Console gets declarative panel v2, an operator Self-service tab, and customer project pages. echo-demo is the reference plugin.

**Tech Stack:** Rust (SeaORM raw statements, axum), PostgreSQL RLS, React/TanStack Router/Query, zod, Bun.

**Spec:** `docs/superpowers/specs/2026-10-21-product-plugins-design.md`

## Global Constraints

- Shadow mode: no v1 manifest compatibility, no data migration for existing grants. Local dev data may be reset.
- No product names in Core code paths: Core never branches on a plugin id.
- Default access mode for every feature is `assigned`. A new plugin exposes nothing to customers.
- Preview sessions stay read-only and cannot invoke plugin writes. Plugin admission already rejects preview.
- Customer writes need owner/admin; reads need membership (`webdock_rls.customer_allowed`).
- Migrations are additive files in `crates/plugin-host/migrations/`, linked from `crates/persistence/migrations/` with the next free number (0026). Do not edit `baseline.sql` or `frozen.json` entries.
- Tests use only disposable local PostgreSQL fixtures (`.runtime-test/*.env`, `--ignored`). No production data, no real mail, never port 1025.
- No names of external inspiration sources anywhere.
- JavaScript tooling is Bun only.

## Review Focus

1. A customer passes a `resourceID` that belongs to another project or customer. Expect denial, never a cross-tenant call. Test: `resource_gate_rejects_foreign_resource` (Task 3).
2. The operator switches a feature to `assigned` while a customer's write is in flight. The invocation admitted under the old revision is denied at dispatch. Test: `policy_change_revokes_inflight_invocation` (Task 3).
3. A customer removed from a `selected` list keeps using their existing resources, but cannot create new ones. Test: `removed_customer_keeps_resources` (Task 3).
4. A plugin's create output omits or forges `resource`, or uses an id containing `/`. Expect the invocation to fail with an output error and no index row written. Test: `create_output_requires_valid_resource` (Task 3).
5. A member (not owner/admin) opens "Add". The menu shows only read-only resources and no create forms, and a direct API create is denied. Test: `member_cannot_create` (Task 3) and the browser check (Task 8).

---

### Task 1: Manifest protocol 2

**Files:**
- Modify: `crates/plugin-protocol/src/manifest.rs`
- Test: `crates/plugin-protocol/tests/contracts.rs`
- Modify: `plugins/echo-demo/manifest.json`, `plugins/lumaserv-compute/manifest.json`, `plugins/github-repository/manifest.json`, `plugins/mail-relay/manifest.json` (and any embedded copies these plugins compile in; grep `protocolVersion`)

**Interfaces:**
- Produces:
  - `Manifest::offering(&self, id: &str) -> Option<&Value>`
  - `Manifest::feature(&self, id: &str) -> Option<&Value>`
  - `Manifest::features(&self) -> &[Value]`
  - `Manifest::self_service_pages(&self) -> &[Value]`
  - `Manifest::operator_pages(&self) -> &[Value]`
  - action JSON fields `audience` (`"operator"|"customer"`), `offering?`, `feature?`, `resource?` (`"create"|"use"|"delete"`)

Manifest changes:
- `protocolVersion`, `sdkVersion` and `dataProtocolVersion` must be 2.
- Remove `slots` and `navigation`.
- Add required `offerings`, `selfService` and `areas`.
- Every action needs `audience`.

Shapes:
```jsonc
"offerings": [{ "id": "note", "label": "Note" }],             // 0..16, id SLUG, label 1..80
"selfService": { "features": [{ "id": "create-notes", "label": "Create notes", "offering": "note", "description": "..." }] }, // 0..32
"areas": {
  "operator":    { "pages": [{ "id": "overview", "label": "Overview", "action": "demo.operator" }] },
  "selfService": { "pages": [{ "id": "note", "label": "Note", "action": "notes.view", "resource": true }] }  // optional key
}
```

- [ ] **Step 1: Write failing tests** in `contracts.rs`:
  - `v2_manifest_accepts_reference`: a v2 echo manifest parses.
  - `rejects_v1_protocol`: `protocolVersion: 1` is an error.
  - `customer_action_needs_gate`: `audience: "customer"` with neither `feature` nor `resource` is an error.
  - `customer_action_needs_self_service_area`: `audience: "customer"` without `areas.selfService` is an error.
  - `feature_must_exist_and_be_used`: an action with an unknown `feature` is an error, and a declared feature with no action is an error.
  - `create_needs_feature_and_offering`: `resource: "create"` without both `feature` and `offering` is an error.
  - `use_needs_resource_input`: for `resource: "use"|"delete"`, the input schema must have a required string property `resourceID` with `maxLength` ≤ 128.
  - `page_action_must_be_read_of_matching_audience`: operator pages need `audience: "operator"`, `intent: "read"`. Self-service pages are resource pages: they need `resource: true` and an action with `audience: "customer"`, `intent: "read"`, `resource: "use"`.
  - `product_names_not_reserved`: `mail` parses as a name, and `core` is still rejected.
  - `ids_unique`: duplicate offering, feature or page ids are rejected.
- [ ] **Step 2: Run** `cargo test -p webdock-plugin-protocol --test contracts`. Expect FAIL.
- [ ] **Step 3: Implement** in `validate_manifest`:
  - Remove `mail`, `hosting`, `databases`, `cms` and `storage` from `RESERVED`.
  - Delete the `SLOTS` and `UI_PATH` validation paths.
  - Action optional keys gain `offering`, `feature` and `resource`; required keys gain `audience`.
- [ ] **Step 4: Port the four plugin manifests.**
  - echo-demo is rewritten in Task 7. For now its actions just get `audience: "operator"`, and its slot becomes an operator page.
  - lumaserv-compute, github-repository and mail-relay: all actions `audience: "operator"`, `offerings: []`, `selfService: {features: []}`, `areas: {operator: {pages: []}}`. Keep their existing settings slot's action as an operator page if it is a read action.

  Then run each plugin's own `cargo test` (they are standalone workspaces).
- [ ] **Step 5: Run** `cargo test -p webdock-plugin-protocol` and the four plugin test suites. Expect PASS.
- [ ] **Step 6: Commit** "Plugin manifest protocol 2 with offerings, self-service features and areas".

### Task 2: Self-service tables

**Files:**
- Create: `crates/plugin-host/migrations/0004_self_service.sql`
- Create: symlink `crates/persistence/migrations/0026_plugin_self_service.sql -> ../../plugin-host/migrations/0004_self_service.sql`
- Test: `crates/plugin-host/tests/postgres.rs` (migration applies; runtime roles can read but not write policy rows directly except through the operator path)

**Interfaces:**
- Produces tables (schema `webdock_auth`, owner `webdock_platform_owner`, FORCE RLS, platform_owner policy like neighbors):
  - `plugin_feature_policy(plugin_id text FK plugin_installation(id) ON DELETE CASCADE, feature_id text, mode text CHECK IN ('everyone','selected','assigned'), updated_by text, updated_at timestamptz, PK(plugin_id, feature_id))`
  - `plugin_feature_customer(plugin_id, feature_id, customer_id text FK webdock_admin.customers(id) ON DELETE CASCADE, added_by text, added_at timestamptz, PK(plugin_id, feature_id, customer_id))`
  - `plugin_resource(plugin_id FK, resource_id text CHECK (resource_id ~ '^[A-Za-z0-9_.:-]{1,128}$'), offering_id text, customer_id text, project_id text, name text CHECK length 1..160, created_via text CHECK IN ('operator','self-service'), created_by text, created_at timestamptz, PK(plugin_id, resource_id))`, index `(project_id, plugin_id)`. FK `(project_id)` → `webdock_admin.projects(id)` without cascade: a resource blocks project and customer deletion.
  - `plugin_installation.policy_revision bigint NOT NULL DEFAULT 0`.
- Read policies:
  - `webdock_auth_runtime` and `webdock_worker_runtime` may SELECT all three tables when `webdock_rls.operator()` or `webdock_rls.customer_allowed(customer_id)`. Policy tables are installation-wide; a customer reads its own allow-list rows only.
  - INSERT/UPDATE/DELETE for `webdock_auth_runtime` only when `webdock_rls.operator()`.
  - `plugin_resource` INSERT/DELETE for `webdock_worker_runtime`, done by the controller transaction during admission finish (see Task 3).

- [ ] **Step 1: Write the failing test** `self_service_tables_enforce_rls`:
  - A customer session cannot insert into `plugin_feature_policy`.
  - It can read only its own `plugin_feature_customer` rows.
  - An operator session can insert into both.
- [ ] **Step 2: Run** it with the fixture env: `set -a; . .runtime-test/identity-restricted.env; . .runtime-test/identity.env; set +a; cargo test -p webdock-plugin-host --test postgres self_service -- --ignored`. Expect FAIL (relation missing).
- [ ] **Step 3: Write the migration** following `0002_mail_policy.sql`/`0003_continuations.sql` style. Grants and policies are explicit per role.
- [ ] **Step 4: Run** the test again, plus `bun run quality:contracts`, which checks migration symlinks. Expect PASS.
- [ ] **Step 5: Apply** to the dev DB through Tilt (it migrates on start). Check with `docker exec webdock-dev-postgres-1 psql -U postgres -d webdock -c '\d webdock_auth.plugin_resource'`.
- [ ] **Step 6: Commit** "Plugin self-service policy and resource index tables".

### Task 3: Admission gates and resource index

**Files:**
- Modify: `crates/plugin-host/src/authority.rs` (`metadata`, `action`, `admit_authority`, `persisted`, `project_plugins`)
- Create: `crates/plugin-host/src/self_service.rs` (gate evaluation and grant materialization; keeps `authority.rs` from growing)
- Modify: `crates/plugin-host/src/lib.rs` (`mod self_service;`)
- Modify: `crates/control-plane/src/plugins.rs` (`invoke_admitted` records `resource` output)
- Modify: `crates/plugin-host/src/admin.rs:271` (grant capability editing becomes `sync_grants`)
- Modify: `tools/dev/local.mjs:168` (drop the grant insert)
- Modify: test fixtures that insert grants (`crates/plugin-host/tests/postgres.rs`, `tests/support/controller.rs`, `crates/build/tests/support/repository_flow.rs`) to rely on materialization or keep inserting where they test grant internals
- Test: `crates/plugin-host/tests/postgres.rs`

**Interfaces:**
- Consumes: Task 1 accessors, Task 2 tables.
- Produces:
  - `pub(crate) async fn gate(tx: &DatabaseTransaction, actor: &Principal, manifest: &Manifest, action: &Value, plugin: &str, customer: &str, project: &str, input: &Value) -> Result<()>`
  - `pub(crate) async fn ensure_grant(tx: &DatabaseTransaction, plugin: &str, customer: &str, project: &str) -> Result<()>`: upserts the `plugin_grant` row with `capabilities = plugin_configuration.approved_capabilities` and `revision = plugin_installation.policy_revision`. If the existing row's revision is lower, it is updated.
  - `pub async fn record_resource(&self, claims: &InvocationClaims, actor: &Principal, output: &Value) -> Result<()>`: called after a successful `resource: "create"` or `"delete"` invocation. It validates `output.resource = {id, name}` for create and writes or deletes the index row in one transaction.
  - `pub(crate) async fn bump_policy(tx, plugin) -> Result<i64>`: `UPDATE plugin_installation SET policy_revision=policy_revision+1`, then `UPDATE plugin_grant SET revision=<new>`. Used by Task 4.
- Gate rules, in this order:
  1. `audience == "operator"` → `actor.is_operator()`, else denied.
  2. If the action has a `feature`, unless `actor.is_operator()`:
     - mode `everyone` → ok;
     - mode `selected` with a row in `plugin_feature_customer` → ok;
     - otherwise denied. A missing policy row means `assigned`.
  3. `resource ∈ {use, delete}` → a `plugin_resource` row with `(plugin_id, resource_id=input.resourceID, project_id=project)` must exist. This applies to operators too.
  4. `resource == "create"` → `input.resourceID` must be absent.

- [ ] **Step 1: Write failing tests**, with a v2 test manifest in the test support. Cover the policy matrix: mode {everyone, selected+listed, selected+unlisted, assigned} × actor {operator, owner, admin, member, foreign owner, preview} for the create action. Expected results:
  - operator: always allowed;
  - owner/admin: allowed iff mode permits;
  - member: always denied (write);
  - foreign owner: denied;
  - preview: denied.

  Plus:
  - `resource_gate_rejects_foreign_resource`
  - `policy_change_revokes_inflight_invocation`: admit, `bump_policy`, then `authenticate` is denied
  - `removed_customer_keeps_resources`: a use action still passes after removal
  - `create_output_requires_valid_resource`
  - `member_cannot_create`
  - `operator_assigns_resource`: an operator create on a customer project records `created_via='operator'`, and the customer can then use it.
- [ ] **Step 2: Run** `cargo test -p webdock-plugin-host --test postgres -- --ignored` with the fixture env. Expect FAIL.
- [ ] **Step 3: Implement.**
  - `admit_authority` calls `ensure_grant` before `metadata` re-reads the grant. The order is: controller tx, `ensure_grant`, `metadata`, `action`, `gate`.
  - `action()` keeps the capability ceiling check.
  - Admission stores `resourceID` in `session_context`. `persisted()` rechecks the feature mode and the resource row from it.
  - `grant_revision` already equals the policy revision, so `bump_policy` invalidates in-flight calls through the existing `metadata.grant["revision"] != row["grant_revision"]` check.
- [ ] **Step 4: Discovery.** `project_plugins` stops joining `plugin_grant`. It lists enabled, ready external installations, and for each returns the existing manifest view plus:
  - `features`: usable feature ids for this actor and customer. Operators get all of them; members get none, since they cannot write.
  - `resources`: `[{id, offering, name, createdVia}]` for the project.

  Non-operators only see a plugin when `features` or `resources` is non-empty.
- [ ] **Step 5: Run the tests.** Expect PASS. Also run `cargo test -p webdock-control-plane` and `cargo test -p webdock-build` (repository flow).
- [ ] **Step 6: Commit** "Gate plugin actions by audience, self-service mode and resource ownership".

### Task 4: Operator policy API

**Files:**
- Modify: `crates/plugin-host/src/admin.rs` (new methods)
- Modify: `crates/control-plane/src/plugins.rs` (routes)
- Test: `crates/plugin-host/tests/postgres.rs`, `crates/control-plane` route test if one exists for `/api/console/plugins/installed` (mirror it)

**Interfaces:**
- Produces, all operator-only via `operator_tx` and audited via `audit_admin` with action names `plugin.feature.mode` / `plugin.feature.customer`:
  - `HostService::self_service(&self, actor, plugin: &str) -> Result<Value>` returns `{revision, features: [{id, label, description?, offering?, mode, customers: [{id, name}]}]}`.
  - `HostService::set_feature_mode(&self, actor, plugin, feature, mode: &str, expected_revision: &str) -> Result<Value>`. It rejects unknown features and a stale revision (409), then `bump_policy`.
  - `HostService::set_feature_customer(&self, actor, plugin, feature, customer: &str, allowed: bool, expected_revision: &str) -> Result<Value>`. Same checks, then `bump_policy`.
  - `HostService::customer_self_service(&self, actor, customer: &str) -> Result<Value>` returns `[{plugin, feature, label, mode, allowed}]` for all `selected` features of enabled plugins.
- Routes:
  - `GET /api/console/plugins/{id}/self-service`
  - `PUT /api/console/plugins/{id}/self-service/features/{feature}`, body `{mode, expectedRevision}`
  - `PUT /api/console/plugins/{id}/self-service/features/{feature}/customers/{customer}`, body `{allowed, expectedRevision}`
  - `GET /api/console/customers/{id}/self-service`

  Browser authority goes through `admin_guard` like the existing `/installed` admin route.

- [ ] **Step 1: Write failing tests:**
  - `operator_sets_mode_and_list`
  - `customer_cannot_change_policy`
  - `stale_revision_conflicts`
  - `unknown_feature_rejected`
  - `customer_view_lists_selected_features`
- [ ] **Step 2: Run them.** Expect FAIL. **Step 3: Implement.** **Step 4: Run them.** Expect PASS.
- [ ] **Step 5: Commit** "Operator API for plugin self-service modes".

### Task 5: Customer projects in Core

Customers cannot list or open their projects today: the registry is operator-only. The self-service area needs both.

**Files:**
- Modify: `crates/access/src/service.rs` (tenant service) or the module that serves `/api/console/tenants`. Pick the one that already resolves membership.
- Modify: `crates/control-plane/src/access.rs` (routes)
- Test: `crates/access/tests/tenants.rs`

**Interfaces:**
- Produces:
  - `GET /api/console/tenants/{id}/projects`: members; returns `[{id, name, status}]` for active projects.
  - `POST /api/console/tenants/{id}/projects` with `{name}`: owner/admin; creates an active project for the customer and audits it.
  - `GET /api/console/projects/{id}`: members of the owning customer or operators; returns `{id, name, customer: {id, name}, role}`.

  Reuse the registry service's project insert if it can run under a customer actor; otherwise insert directly under the restricted role and adjust RLS. Check `webdock_admin.projects` insert policy first.

- [ ] **Step 1: Write failing tests:**
  - `member_lists_projects`
  - `admin_creates_project`
  - `member_cannot_create_project`
  - `foreign_customer_denied`
  - `preview_cannot_create`
- [ ] **Step 2: Run them.** Expect FAIL. **Step 3: Implement**, with an additive migration if RLS needs a customer insert policy (next free number). **Step 4: Run them.** Expect PASS.
- [ ] **Step 5: Commit** "Customers list and create projects in their workspace".

### Task 6: Declarative panel v2

**Files:**
- Modify: `apps/console-ui/src/plugins/contracts.ts`, `apps/console-ui/src/plugins/Panel.tsx`
- Test: `apps/console-ui/tests/plugin-renderer.test.tsx`, `apps/console-ui/tests/plugin-contract.test.ts`

**Interfaces:**
- Produces:
  - `panelSchema` with `version: z.literal(2)`, optional `refreshSeconds` (int 5..300).
  - Field types add `checkbox`, `secret` and `textarea`. A secret field never pre-fills and never echoes.
  - Table optional `rowLink: "resource"`: rows must contain a string `resourceID`, and each row renders as `ui.clickableRow` linking to the resource page.
  - New block `{kind: "reveal", label, action}`: renders a button. On click it invokes `action` (read), shows `{value: string}` from the result in a `ui.mono` box with a copy button, and hides it again on navigation. The value is never stored in query cache: use a local state from `api.request`, not `useQuery`.
  - `PluginPanel` props gain `context: Record<string, string>`, which is merged into every action input (`resourceID` on resource pages).
- [ ] **Step 1: Write failing renderer tests:**
  - `secret field is write-only`
  - `rowLink rows navigate with resourceID`
  - `reveal shows value only after click and not in query cache`
  - `context merges into action input`
  - `rejects version 1`
  - `refreshSeconds bounds`
- [ ] **Step 2: Run** `bun run --filter @webdock/console-ui test`. Expect FAIL. **Step 3: Implement with `ui.ts` recipes.** **Step 4: Run it.** Expect PASS.
- [ ] **Step 5: Commit** "Declarative plugin panel v2".

### Task 7: echo-demo as reference product plugin

**Files:**
- Modify: `plugins/echo-demo/manifest.json`, `plugins/echo-demo/src/lib.rs`, `plugins/echo-demo/src/main.rs`
- Modify: `tools/dev/local.mjs` (seed: no grants; set feature `create-notes` to `selected` for the Demo customer so local dev shows the flow)
- Test: `plugins/echo-demo` unit tests, and the existing runtime test that runs echo-demo (`grep -rn echo-demo tools crates/*/tests`)

**Interfaces:**
- Consumes: Task 1 schema, Task 3 resource envelope, Task 6 panel v2.
- Produces, with offering `note` and feature `create-notes`:
  - `notes.create` (write, customer, feature `create-notes`, resource `create`, input `{name}`) returns `{resource: {id: <uuid>, name}}`
  - No project-level list action: the project page lists resources from Core discovery (Task 3). Self-service pages are resource pages only (Task 1 validation).
  - `notes.view` (read, customer, resource `use`) returns a panel v2 with the note name, a `reveal` block (`notes.secret` returns `{value: "echo:" + id}`) and a delete action.
  - `notes.secret` (read, customer, resource `use`)
  - `notes.delete` (write, customer, feature `create-notes`, resource `delete`)
  - `demo.operator` (read, operator) returns a panel with a static status. It is project-scoped for now; installation scope comes in plan 2.
- [ ] **Step 1: Write failing plugin tests** for the outputs above. **Step 2: Run them.** Expect FAIL. **Step 3: Implement.** **Step 4: Run them.** Expect PASS.
- [ ] **Step 5: Rebuild the plugin image and reseed:** `bun tools/dev/local.mjs plugin-image && bun tools/dev/local.mjs seed`, or via the Tilt resource. Confirm Core starts and lists echo-demo.
- [ ] **Step 6: Commit** "echo-demo as reference self-service plugin".

### Task 8: Console self-service and operator areas

**Files:**
- Create: `apps/console-ui/src/ProjectPage.tsx` (customer and operator project view: plugin resources grouped by plugin and offering, "Add" menu)
- Create: `apps/console-ui/src/plugins/ResourcePage.tsx` (renders a plugin's self-service resource page through `PluginPanel` with `context={{resourceID}}`)
- Create: `apps/console-ui/src/plugins/SelfServicePolicy.tsx` (operator tab: features with mode select and, for `selected`, a customer picker using `/api/console/registry/customers`)
- Modify: `apps/console-ui/src/plugins/InstalledExtensions.tsx` (each row links to `/extensions/$plugin`)
- Modify: `apps/console-ui/src/App.tsx`. Routes:
  - `/extensions/$plugin`: operator plugin page with tabs "Overview" (operator pages) and "Self-service"
  - `/workspaces/$id/projects`
  - `/projects/$id`
  - `/projects/$id/plugins/$plugin/resources/$resource`
- Modify: `apps/console-ui/src/Workspaces.tsx` (workspace rows link to the projects list; the projects list has a "New project" form for owner/admin)
- Modify: `apps/console-ui/src/CustomerMembers.tsx` or the operator customer detail in `Registry.tsx` (add a "Self-service" section from `GET /api/console/customers/{id}/self-service` with checkboxes)
- Modify: `apps/console-ui/src/Registry.tsx:352` (replace `ProjectExtensions` with a link to `/projects/$id`), then delete `ProjectExtensions.tsx`
- Modify: `apps/console-ui/tests/preview-server.ts` (fixtures for the new endpoints)
- Test: `apps/console-ui/tests/self-service-browser.mjs` (new; add to `test:browser`)

**Interfaces:**
- Consumes:
  - Task 3 discovery fields `features` and `resources`
  - Task 4 routes
  - Task 5 routes
  - Task 6 `PluginPanel` `context`
- "Add" menu:
  - It lists the features from discovery whose action has `resource: "create"`.
  - Choosing one opens a form generated from the create action's input schema: string → text, integer/number → number, boolean → checkbox, enum → select.
  - Submitting invokes the action. On success it navigates to the new resource page.
  - The menu is hidden when `features` is empty.
  - Operators on a customer project see all create features, labelled "Assign to this project".

Copy, written from the user's perspective in sentence case:
- Mode labels: "Everyone", "Selected customers", "Operator assigns". The help line under each feature reads "Who can do this themselves".
- Empty project with no usable feature: "Nothing here yet. Your provider adds resources to this project for you."

- [ ] **Step 1: Write the failing browser test** `self-service-browser.mjs` against the preview server:
  1. As operator: open `/extensions/echo-demo` → Self-service, set "Create notes" to Selected customers, and add Demo.
  2. As Demo owner: open the project, use Add → Create note → name, land on the resource page, click reveal and see the value.
  3. As operator: switch to Operator assigns.
  4. As Demo owner: "Add" is gone, and the note is still listed and opens.
  5. As Demo member: no "Add".
- [ ] **Step 2: Run** `bun run --filter @webdock/console-ui test:browser`. Expect FAIL. **Step 3: Implement.** **Step 4: Run** lint, typecheck, test and test:browser. Expect PASS.
- [ ] **Step 5: Commit** "Console project pages with plugin resources and operator self-service controls".

### Task 9: End-to-end check and push

- [ ] **Step 1: Run** `bun run quality`. Expect all green.
- [ ] **Step 2: Real stack.** With Tilt running, repeat the Task 8 browser flow against the real Core at `127.0.0.1:3121`, with the dev DB and the echo-demo runtime. Use the scratchpad screenshot helper and check desktop and phone width.
- [ ] **Step 3: Update docs.**
  - `docs/verification/2026-10-20-native-parity.md` Plugins row: self-service framework present.
  - Spec status line.
  - Spec Task 7 note: self-service pages are resource pages.
- [ ] **Step 4: Commit and push** to `t3code/rust-platform-rewrite`.
