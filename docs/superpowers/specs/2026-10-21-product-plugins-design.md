# Product plugins with operator and self-service areas

Date: 2026-10-21. Status: approved by the owner on 2026-10-21. Step 1 (self-service framework) implemented per `docs/superpowers/plans/2026-10-21-plugin-self-service-framework.md`; step 2 (PostgreSQL plugin, local placement) per `docs/superpowers/plans/2026-10-22-postgres-plugin.md`; mail (sending, BYOK turboSMTP) and hosting (apps) plugins with local placement followed on 2026-10-22; plugin databases, workload access and installation-scoped operator actions move to the PostgreSQL plan.

## Intent

The owner wants every Webdock product to be a plugin so the Core stays a slim kernel. Everything Webdock can do today must be available to customers in self-service, but the operator decides per plugin which parts are self-service and for whom.

Owner statements this design is built on (2026-10-21):

- All products are plugins: managed databases through shared databases ("same plugin, different product"), mail, and everything else Webdock does today.
- Each plugin defines an operator area and, optionally, a self-service area.
- Per plugin the operator decides what is available in self-service, for example turboSMTP BYOK.
- Per self-service item the operator chooses one of three modes: global self-service, self-service for selected customers, or no self-service where the operator assigns the resource.
- Databases: focus on PostgreSQL and the Tabularis web database browser. Tabularis web gets an official release today; wait with that part.
- CMS is out of scope for now.

This supersedes the "optional built-in features" paragraph of `2026-10-19-rust-platform-design.md` (hosting, databases, mail and storage as Core built-ins behind `WEBDOCK_FEATURES`). Everything else in that spec, in particular the gRPC execution boundary, host capabilities and the two plugin UI modes, still applies.

Webdock is in shadow mode. There is no backward compatibility with the current Core product tables, `WEBDOCK_FEATURES` product switches or built-in `plugin_installation` rows; local dev data may be reset.

## Goals

1. Core contains no product logic: no PostgreSQL, CNPG, Stalwart, turboSMTP, hosting or storage code, tables, routes or Console pages. The Core keeps identity, customers/projects/members, authorization, audit, secrets, jobs, the plugin host and a generic renderer for plugin UI.
2. A plugin declares two areas: an operator area (always) and a self-service area (optional).
3. A plugin declares self-service features. For each feature the operator sets the access mode: `everyone`, `selected` or `assigned`.
4. In `assigned` mode the operator creates the resource for a customer project from the operator area; the customer then sees and uses it, but cannot create more.
5. The same rules hold for Console, public API, MCP and jobs.
6. First product plugin: PostgreSQL, with shared and dedicated databases as two offerings of one plugin.

## Non-goals

- CMS, analytics, websites as a product.
- Tabularis web database browser until its official release is out (see "Waiting on Tabularis").
- New billing, payment providers or price enforcement. Existing plan/offer records stay what they are today.
- The native catalog installer (separate open item in the handover). Dev installs plugins with the existing seed path.
- New provider integrations beyond what Webdock already does.

## Anti-goals

These would make the result a failure even if it works:

- A product name branch in Core (`if plugin == "postgres"`), a product table in the Core schema, or a product page in the Console's own source. First-party plugin UIs live in `plugins/<id>/ui`; the Console only finds them by a generic glob (see "First-party plugin UI").
- Self-service on by default. A freshly installed plugin exposes nothing to customers until the operator sets a mode.
- A customer seeing operator settings, provider credentials, pool internals or another customer's resources.
- A plugin receiving platform database, platform Redis, cluster-admin or session-signing credentials.

## Vocabulary

| Term | Meaning | Example |
| --- | --- | --- |
| Plugin | Installable unit with its own runtime, storage and UI declarations | `postgres`, `mail`, `hosting` |
| Offering | Kind of resource a plugin can create | `postgres.shared`, `postgres.dedicated` |
| Resource | One instance of an offering, owned by one customer project | the database `orders-prod` in project Shop |
| Self-service feature | Something a customer may do themselves, gated by an access mode | "Create shared databases", "Resize dedicated instances", "Connect own turboSMTP account" |
| Access mode | Operator's choice per feature: `everyone`, `selected`, `assigned` | |
| Operator area | Pages and actions only operators see | pool capacity, plans, all resources across customers |
| Self-service area | Pages and actions customers see inside their projects | database list, connection details, create form |

## Manifest additions (protocol v2)

Bump the manifest to protocol/data protocol 2. Existing v1 plugins (echo-demo, lumaserv-compute, github-repository, mail-relay) are ported; no v1 compatibility. `lumaserv-compute` and `github-repository` become operator-only plugins without a self-service area; the repository service lane carries over unchanged (my call).

```jsonc
{
  "offerings": [
    { "id": "shared", "label": "Shared database" },
    { "id": "dedicated", "label": "Dedicated instance" }
  ],
  "selfService": {
    "features": [
      { "id": "create-shared", "label": "Create shared databases", "offering": "shared" },
      { "id": "create-dedicated", "label": "Create dedicated instances", "offering": "dedicated" },
      { "id": "resize", "label": "Resize dedicated instances", "offering": "dedicated" }
    ]
  },
  "actions": [
    { "name": "databases.create", "intent": "write", "audience": "customer",
      "offering": "shared", "feature": "create-shared", "...": "..." },
    { "name": "databases.credentials", "intent": "read", "audience": "customer",
      "offering": "shared", "resource": true },
    { "name": "pools.register", "intent": "write", "audience": "operator" }
  ],
  "areas": {
    "operator": { "navigation": [], "pages": [] },
    "selfService": { "navigation": [], "pages": [] }
  }
}
```

Rules:

- `audience: "operator"`: only operators may invoke.
- `audience: "customer"`: operators and customer members may invoke. It declares at least one gate, and both may be combined (my call):
  - `feature`: the customer needs the feature's access mode to allow them (below).
  - `resource: "create" | "use" | "delete"`: for use/delete the input carries `resource_id` (lowercase, as the schema validator requires). The Core checks that the resource belongs to the invoking project.
  - Create actions use `feature` only. Changes to an existing resource that the operator should be able to switch off (resize, pause/resume, rotate) use `feature` plus `resource`. Use actions (view, connection details, open browser) use `resource` only.
- `scope`: `project` (default) or `installation`. Operator actions such as `pools.register` have no project and use `installation`; only `audience: "operator"` may use it (my call).
- Operators are the existing platform operator role. They may invoke customer actions on any project; the invocation and the audit entry record "operator acting for customer X" (my call).
- A plugin without `areas.selfService` has no customer actions; validation rejects `audience: "customer"` there.
- Write actions by customers still need owner/admin in the customer. Read actions need membership. This is the existing `customer_allowed` rule. Preview sessions stay read-only and cannot invoke plugin writes.
- Validation rejects features that no action references, actions that reference unknown features or offerings, and duplicate ids.

## Access modes

Per (plugin installation, feature) the operator stores a mode. The default for every feature is `assigned`.

| Mode | Who may use the feature |
| --- | --- |
| `everyone` | Every customer, global self-service |
| `selected` | Only customers on the feature's allow list |
| `assigned` | No customer; operators create resources and assign them to a customer project |

Actions gated by `resource` only do not depend on the mode: once a customer owns a resource, whether the operator assigned it or the customer created it, they can view and use it. Actions that also name a feature follow its mode on owned resources too, so `assigned` on "Resize" means only the operator resizes. Switching a feature to `assigned` never removes existing resources.

`plugin_grant` and per-project enablement are dropped (my call). Remaining ceilings: installation enabled, `approved_capabilities` (operator ceiling per installation), the action's gates. Every invocation stores the policy revision it was admitted under; dispatch rechecks against the current revision, replacing `grant_revision`.

Core tables, replacing `plugin_grant`:

- `plugin_feature_policy(installation_id, feature_id, mode, revision, updated_by, updated_at)`
- `plugin_feature_customer(installation_id, feature_id, customer_id, added_by, added_at)`
- `plugin_resource(installation_id, resource_id, offering_id, customer_id, project_id, name, state, created_via operator|self-service, created_at, updated_at)`. The plugin reports changes through a new host callback `ReportResource`. The Core uses this index for authorization, project overviews, search and customer deletion blockers. The plugin stays the source of truth for resource details.

Policy changes are audited. Admission evaluates policy at invocation time and again when a job dispatches. Both evaluations happen in `admit_authority`, next to the existing capability ceiling (`plugin_configuration.approved_capabilities`).

## Plugin storage

Each product plugin installation gets its own PostgreSQL database and login role, provisioned by the Core installer step (dev: the seed). The role cannot reach the platform database. The plugin receives the URL as a secret reference and runs its own migrations at startup with an advisory lock. This replaces the manifest's action-only lifecycle restriction for plugins that declare `"storage": "database"`. The Core never reads plugin tables.

Current Core product tables move out of `crates/persistence/schema/baseline.sql` into the owning plugin's migrations. Examples: `hosting_database_*`, `databases.*`, `webdock_mail.*`, `hosting_byok_policy`, `hosting_turbosmtp`.

## Infrastructure access

Product plugins provision real workloads: CNPG clusters, Stalwart instances and app deployments. Webdock already places workloads on several clusters, each with an enrolled agent that pulls jobs and writes receipts. That stays (my call):

- Clusters, agent enrollment and capacity inventory stay in Core as generic infrastructure that several plugins share.
- A new host capability `plugin.workloads` lets a plugin submit desired state to a chosen cluster: Kubernetes objects or Docker containers/volumes/networks. The Core routes it to that cluster's agent and returns receipts and observed state.
- The agent applies only what the manifest declares and the operator approved: allowed object kinds (for example `postgresql.cnpg.io/Cluster`, Secret, Service), a namespace prefix such as `webdock-pg-`, and an image digest allowlist. Everything is labelled with the installation id; nothing outside it can be read or changed.
- The agent knows nothing about PostgreSQL, Stalwart or apps. The CNPG, Stalwart and hosting knowledge in `crates/agent` moves into the plugins; the agent keeps generic apply/observe, receipt and lifecycle code.
- Placement (which cluster, which shared pool) is the plugin's decision, using the Core capacity inventory.

## UI

### Operator area

- **Extensions → *plugin*:** the plugin's operator pages, with plugin navigation and declarative panels.
- **Self-service tab per plugin:** global settings only. Every declared feature has its mode switch (`everyone`, `selected`, `assigned`); for `selected` the tab only lists who is allowed.
- **Customer page → Self-service:** the place where the operator decides per customer. For one customer, every `selected` feature of every plugin, with a checkbox to add or remove that customer.
- **Assign:** in the plugin's operator pages, a create form that additionally asks for customer and project. It invokes the same create action with operator authority and a target project.

### Self-service area

- **Project page:** lists the project's resources from `plugin_resource`, grouped by plugin and offering. The page offers an "Add" menu containing only the features this customer may use. A customer with no usable feature and no resources does not see the plugin at all.
- **Resource page:** the plugin's self-service pages for that resource, as declarative panels.

This needs the customer project view that does not exist yet; today non-operators are redirected to Workspaces. Follow the agreed hierarchy: workspace (customer) → project → resources.

### Declarative schema v2

Extend `apps/console-ui/src/plugins/contracts.ts` (panel v1: text, status, table, form, actions) to what the PostgreSQL plugin needs:

- pages and navigation
- form field types: text, number, select, checkbox and write-only secret
- row links to resource pages
- operation progress for long-running actions
- a **reveal** block for credentials that calls a dedicated read action on click, is audited, and never puts the secret into panel data

Webmail and other editors that cannot be declarative use the existing isolated custom UI mode.

### Help in every plugin

Every offering carries a `description` (required, up to 280 characters: what it is and when to choose it) and optional `highlights` (up to six short points: what it brings). The Console shows them as cards on an empty product page, under "Which one do I need?" below the list, in the create menu, above the create form and on each resource page. Texts promise only what the plugin does today.

### First-party plugin UI (Rust + React)

The initial plugins (PostgreSQL, mail, hosting, storage, Tabularis) are built like the Core: a Rust backend and a React frontend with the Console's toolchain and design system. Each ships a React package in `plugins/<name>/ui` with its operator and self-service pages. The Console bundles these packages at build time and loads each lazily. A page appears only when the plugin is installed and its manifest declares the page; every call still goes through the plugin action API and its gates. Declarative panels remain for third-party plugins, whose code never runs in the Console origin.

## Plugins and order

Each step ends green under `bun run quality` with real fixture tests before the next starts.

1. **Core framework.**
   - manifest v2 validation
   - policy tables and admission
   - `ReportResource`
   - plugin databases
   - scoped Kubernetes/Docker workload access
   - declarative v2
   - operator Self-service tab, customer page section and customer project page

   Port echo-demo as the reference plugin and test.
2. **`postgres`:** offerings `shared` and `dedicated`. Port `crates/databases` plus the CNPG parts of `crates/agent` into `plugins/postgres`, and the UI from `ManagedDatabases.tsx`/`Databases.tsx` into declarations.

   Features:
   - create shared database
   - create dedicated instance
   - resize
   - pause/resume
   - rotate credentials

   Operator area:
   - pools
   - plans
   - capacity
   - all resources
   - restore

   Self-service: list, detail, connection details via reveal, and actions as the modes allow.

   No prices, offers or orders for now (owner, 2026-10-21: "Ohne Preise"). Creating a database is just creating it, by the customer in self-service or by the operator in `assigned` mode. Plans remain as technical sizes (CPU, memory, storage, connection limits) the operator defines; billing comes later. The current offer/order tables and commands are dropped, not ported.

   The gateway authority endpoints and `database_binding/grant/launch/session` move into the plugin too. The gateway binary itself stays unchanged and only points at the plugin (my call), so the browser launch keeps working in between.

   Remove `crates/databases` and the Console database pages from Core.
3. **`mail`:** offerings `outgoing`, `incoming` and `byok-turbosmtp`. Port `crates/mail` (Stalwart per customer, outbox, domains, mailboxes, webmail as custom UI). `mail-relay` merges into `mail` as its turboSMTP provider module, so no plugin-to-plugin calls are needed (my call; the provider stays outside Core).

   Features:
   - activate outgoing
   - activate incoming
   - manage domains and mailboxes
   - connect own turboSMTP account (BYOK)
4. **`hosting`:** apps, deployments, environment, Git, BYOK Kubernetes/Vercel as features. Port `crates/hosting`.
5. **`storage`:** port the remaining storage inventory.
6. **Tabularis web browser:** inside `postgres` as a resource action "Open in browser", once the official release is pinned (below).

## Waiting on Tabularis

`vendor/tabularis-web-ui.json` pins a patched build of an unreleased pull request. Once the official release is published, replace the pin with the released artifact and its checksum. Check whether the local patches are still needed. Then move `crates/database-gateway` into `postgres` as a plugin component. Until then the gateway stays as is and the PostgreSQL work does not touch it.

## Verification

- **Policy matrix per plugin.** For each mode (`everyone`, `selected` listed, `selected` not listed, `assigned`), run each of operator, owner, admin, member, foreign customer and preview, via Console, API and MCP. Expect allowed/denied exactly as specified. Switching to `assigned` keeps existing resources usable.
- **Core boundary test.** No product crate or plugin dependency in the Core workspace. No product table in `baseline.sql`. No product page in the Console bundle. Extend the existing core-product-boundary check.
- **Real fixtures.** PostgreSQL shared and dedicated against disposable Docker and Kind, with no production data and no real mail. SMTP only to loopback fixtures, never port 1025.
- **Browser tests.** Operator sets a mode; the customer sees or does not see "Add"; assigned resource flow.

## Left to the builder

- exact field names, Protobuf numbering and table layouts
- how the reveal block's audit entry looks
- how the plugin DB role is created in dev (seed) versus production (installer)
- splitting work into commits

## Owner answers

- Self-service database creation: no prices or orders for now (2026-10-21).
- Direction approved; start with steps 1 and 2 (2026-10-21).
