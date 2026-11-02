# Webdock Rust platform and React Console

Date: 2026-10-19. Status: approved for implementation by the owner; implementation in progress.

## Confirmed direction

The owner wants a full rewrite of Webdock's backend in Rust and a redesigned operational Console. The owner explicitly clarified: everything except the frontend; retain React, Tailwind and an icon library. This is a platform migration, not a dashboard demo or a Rust proxy in front of the existing business logic.

Success means existing Webdock users can complete their existing workflows with Rust owning platform behavior, while the Console gains contextual navigation, deployment visibility and operational clarity. Existing implemented behavior is the parity floor. Historical plans are not automatically new feature commitments.

The owner subsequently selected SeaORM as the primary ORM and Loco as the Control Plane framework, with Webdock-owned authentication and no Clerk in the target runtime. The Core may contain a broader set of independently enabled/disabled built-in features. Provider-specific integrations, explicitly including the Lumaserv API, must remain separately installable plugins outside the Core. The existing plugin infrastructure is a required migration workstream. Webdock must have its own supervisor/agent, prioritize Kubernetes/Helm, and also fully support Docker. Use local Unix-socket gRPC where co-located and optional authenticated network transport for distributed operation. Plugin UI defaults to host-rendered declarations while isolated custom UI remains supported. Remaining implementation details below are recommendations by the author, not additional decisions already approved by the owner.

## Evidence and source baseline

- Open checkout: `73733b0`, with 123 existing changed/untracked entries when inspected. Do not reset, stash, overwrite or blanket-commit that work.
- Locally available `origin/main`: `15930ae`, 168 commits ahead of this checkout. Inspected with `git show` and `git ls-tree`; no claim that this ref was freshly fetched or deployed.
- The current layout includes `apps/control-plane`, `apps/console`, `apps/auth`, `apps/build-worker`, `apps/database-gateway`, `apps/mail`, `agents/kubernetes`, product modules, provider adapters and plugin contracts. The old checkout's `apps/auth` business logic is not the migration baseline.
- Current source is authoritative over old planning/status documents. For example, current README describes a product-free Core and optional products, with Billing excluded; older modular-status material still describes Billing factories. Retained billing records/contracts must be inventoried and preserved without reactivating a removed product.

Implementation begins in a dedicated checkout based on a verified current upstream commit. Save an inventory of the old working-tree changes and compare relevant work against upstream; preserve the originals. Port only changes needed for this task after checking whether upstream already contains them. Never silently copy the entire stale tree over newer code. Resolve genuinely conflicting product behavior with the owner; ordinary path and merge conflicts are implementation work.

## What becomes Rust

| Existing responsibility | Rust destination and required behavior |
| --- | --- |
| Control Plane, core registry, tenancy, grants, audit, module lifecycle | Loco-based Rust API and domain modules; preserve disabled/draining/enabled semantics; optional built-in features may ship in Core |
| Local identity, sessions, account security, OAuth/OIDC, MCP authorization | Webdock-owned Rust identity module; migrate existing local/external identities; remove Clerk from the target runtime |
| Hosting, Git/provider integrations, source/build/deploy orchestration | Rust domain services and independently distributed provider plugins; retain revision/lease/ownership checks |
| Durable jobs, retries, webhooks, reconciliation | Rust worker role sharing domain implementation with the API |
| Kubernetes agent and storage/database helpers; new Docker support | Own Rust supervisor/agent with Kubernetes/Helm and Docker runtime implementations, preserving separate privilege boundaries |
| Build worker, publication, plugin-build coordination | Rust host-side workers; preserve isolated untrusted build execution |
| Database gateway and Tabularis lifecycle management | Rust gateway, scoped credentials, isolated runtime management and current revocation behavior |
| Mail API, gateway, sending/outbox and provisioning glue | Rust services preserving delivery deduplication, quotas, incoming activation and suspension semantics |
| CMS/storage/provider integration and internal migrations/bootstrap | Rust services/tools with existing API and data compatibility |
| First-party plugin host and backend extensions | Rust host/implementations; preserve versioned language-neutral wire contracts |

React components, Tailwind, icons, browser state, generated TypeScript clients and frontend build tooling remain JavaScript/TypeScript. No first-party domain operations remain in Next.js server actions, route handlers or a permanent Bun/Python service at completion.

Third-party engines such as PostgreSQL, Kubernetes, Stalwart, Tabularis and customer-owned Payload instances remain integrated dependencies, not engines to reimplement. Webdock-owned server-side glue around them moves to Rust. Customer applications and external plugins may use any language. Existing third-party Payload hooks inside customer CMS deployments may remain protocol adapters, but must not contain Webdock's authoritative business rules. The final runtime inventory explicitly identifies these dependencies rather than claiming every deployed process is Rust.

The bundled website's CMS boundary needs explicit migration coverage: retain existing content/media/URLs through a separate CMS dependency and Rust-facing integration. React website rendering may be built statically; any dynamic first-party business endpoint moves to Rust. Merely leaving the current Next/Payload server untouched would not satisfy the requested backend rewrite.

## Architecture

ID allocation update: the owner selected an independently authored, internal Maylily-style Rust library (`crates/ids`, `publish = false`) for new allocations. Preserve all existing IDs. The platform layout keeps the 2026-01-01 epoch and positive signed-63-bit decimal string compatibility: 41 timestamp bits, 6 machine bits, 4 generator bits and 12 sequence bits. Combined identity slots 0 and 1 remain reserved for retained database allocators during migration. Generator identity assignment and durable high-water state require explicit coordination; container process IDs and truncated randomness are not uniqueness guarantees. Async persistence must lock the identity/checkpoint, compute, persist and commit before releasing a new ID. Generic layout support must not silently change the platform layout or re-encode old records.

Use a Cargo workspace and a modular monolith, with separate processes only for actual trust or execution boundaries.

- API: Loco on Axum/Tokio, with SeaORM, Tower middleware, Serde, structured errors and tracing. Use Loco's configuration, controller and application lifecycle facilities; keep domain services outside framework controllers. Preserve existing HTTP/error/host contracts rather than adopting generated defaults indiscriminately.
- Persistence: SeaORM against the existing PostgreSQL schemas, explicit migrations, restricted runtime roles and transactional authorization contexts. Use SeaORM entities for ordinary CRUD/relations and parameterized SQL on the same transaction for leases, reservations and PostgreSQL-specific operations. Preserve RLS and signed scope validation; tenant context must never escape its transaction into pooled sessions. Do not introduce a separate independently managed SQLx pool for those queries.
- Jobs: preserve the existing durable operation/outbox behavior in PostgreSQL. Commit the intended change and queued work atomically. Use bounded retries, idempotency, leases and fencing; do not rely on in-memory Tokio tasks for durable work. Loco's PostgreSQL queue may provide delivery for appropriate jobs; it does not replace domain operation state or imply an atomic enqueue with a separate SeaORM transaction. Inventory the existing plugin-job/BullMQ work, then migrate delivery through a tested drain/cutover while preserving its external semantics. Queue setup/migrations must also work with restricted runtime roles and explicit offline migrations.
- Kubernetes: `kube` client/watchers and periodic reconciliation. Watch events accelerate convergence; reconnect/relist and bounded periodic repair prevent missed events from losing work.
- Docker: a supported runtime for plugins and ordinary container applications, managed through the versioned Docker Engine API by the local Webdock agent. It shares operation, ownership, lifecycle and observation contracts with Kubernetes. No Kubernetes dependency for a Docker-only installation.
- Browser updates: authorized SSE for deployment events, logs and status; resumable cursors, bounded buffers, cancellation and revocation handling. Use WebSocket only for genuinely bidirectional existing workflows.
- Contracts: shared Rust DTOs/OpenAPI and generated TypeScript bindings; version existing agent/plugin protocols independently. Preserve existing compatibility endpoints and MCP tool semantics.
- Frontend: React + TypeScript + Vite, Tailwind, Phosphor icons, accessible headless primitives, TanStack Router/Query. Use existing compatible UI code where it helps. Dependencies and exact versions are pinned during implementation after checking their official APIs.
- Console output is static browser assets served by Rust or the installation's reverse proxy. Node remains build tooling, not the Console's required business runtime. Preserve deep links and history fallback without turning missing API routes into HTML responses.
- Distribution: Rust binaries and OCI images, Compose/Helm, explicit offline migrate/bootstrap commands, signed release metadata. Built-in optional features may ship in Core but require explicit configuration/enablement. Provider plugins ship separately and are absent from Core artifacts. Reconcile the old Core/Full packaging with this new boundary without deleting retained product data. No automatic production cutover.

Suggested initial workspace boundaries are `crates/core`, `crates/contracts`, `crates/control-plane`, `crates/agent`, `crates/build-worker`, `crates/database-gateway` and `crates/mail`. Domain modules initially live inside the owning crate; create further crates only when independent build/trust boundaries justify them. Privileged storage helpers remain separate executables. These are intended Rust locations, not a reason to delete the existing implementations before parity.

PostgreSQL remains the platform store. Kubernetes/Helm is the primary runtime; Docker is an additional supported runtime, not a replacement migration for the existing cluster. Existing ingress/TLS integration is preserved; the Docker runtime must supply and verify its own configured routing/TLS integration for exposed applications.

## Own supervisor and agent; local and distributed execution

The owner explicitly requires Webdock's own supervision on both runtimes. Keep one shared platform contract and two execution adapters rather than separate Docker and Kubernetes products.

- **Supervisor:** owns desired plugin/application state, placement, version/generation, admission, health conditions, operation progress, reconciliation and upgrade/rollback policy. Its state and operation leases are durable. It can run as the Control Plane's worker role using shared Rust domain code; it need not be a new microservice solely to have its own name.
- **Agent:** the separately deployed Rust executor near the managed runtime. It observes actual state, applies typed authorized commands, reports logs/metrics/conditions, and journals command identity/results so reconnects and process restarts do not blindly duplicate work. Extend/port the existing enrolled agent protocol and fencing checks.
- **Kubernetes/Helm execution:** the cluster agent manages approved Helm releases and Kubernetes resources through scoped credentials. Helm owns the release specification; Kubernetes controllers/kubelet own normal workload convergence and container restart. Webdock supervises release intent, authorization, progress and health, rather than racing native controllers by independently restarting Pods. Avoid a node-wide privileged DaemonSet unless a specific storage/host function requires it; retain separately privileged helpers where necessary.
- **Docker execution:** the node agent talks to the local Docker Engine API, handles digest-pinned images, controlled containers/networks/volumes, configured CPU/memory limits, events, health, bounded logs/metrics and verified routing. Engine restart policies handle ordinary container crashes; the agent repairs desired-state drift and records failures/backoff. Only this executor has the necessary Engine access; Core's public HTTP process and plugin containers never receive the Docker socket. Compose may bootstrap the platform; the supervisor manages dynamic workloads through the API without parsing arbitrary shell output.
- **Single-host local mode:** Core/supervisor, agent and containers may run on one machine. Local agent/plugin gRPC uses private per-installation Unix-socket directories with verified ownership/permissions and authenticated invocation context. A containerized Core/agent gets only the intended socket mounts; no TCP management listener or remote certificate enrollment is required merely for local execution. This choice does not forbid explicitly granted outbound provider calls.
- **Distributed mode:** enroll an agent per Docker host or managed Kubernetes cluster. Enable mutually authenticated network gRPC explicitly; the default remote-agent design is an outbound persistent connection to Core so each node need not expose an inbound management port. Network identity, certificate rotation/revocation and generation/lease fencing are required. The remote Docker agent can still use local Unix sockets to its plugin containers; distributed management does not require exposing every plugin over TCP.
- **Kubernetes transport:** separate Pods normally communicate through internal Services with mTLS, even on a single-node cluster. Do not force node-local hostPath sockets or put every plugin in the Core Pod to simulate the local Docker layout. The protocol remains the same while the transport and routing depend on placement.
- **Disconnection semantics:** retain running workloads and persisted data. Mark observations stale; no new provisioning, deletion or upgrade may proceed under an expired/revoked authorization lease. On reconnect, reconcile observed state and durable operation IDs before retrying. Assign one active owner/fencing generation per operation/resource. A network outage must not look like a successful stop or an empty inventory.
- **Runtime capabilities:** Docker and Kubernetes expose their real supported placement, networking, storage and rollout capabilities. The common create/deploy/start/stop/logs/health/limits/retained-volume/upgrade flows must work on both. Docker placement is explicit to enrolled hosts; cross-host local-volume migration, automatic storage replication and transparent HA are not implied by distributed connectivity. Do not offer unsupported actions as working buttons.

The runtime implementations are provider-neutral platform infrastructure. This does not permit Lumaserv or other vendor API clients into Core. Both execution adapters consume approved plugin artifacts and the same provider-independent plugin manifest; Kubernetes adds a Helm release, Docker adds a controlled container specification.

## Built-in features and external plugins

The owner explicitly relaxes the earlier product-free-Core constraint. The required distinction is now provider-neutral platform functionality versus independently distributed provider integration.

- **Always available platform kernel:** own authentication, users/workspaces/projects, authorization, audit, configuration, plugin discovery/installation/admission, secret references, API/MCP routing and operation infrastructure. These cannot be disabled in a way that bypasses access control or makes the installation unmanageable.
- **Optional built-in features:** provider-neutral hosting/deployment management, database/storage/Mail management, observability and their UI may ship with the platform and be enabled/disabled per installation. Shipping a feature does not activate it, grant access, provision infrastructure or select a provider account.
- **External provider plugins:** provider-specific endpoints, request/response mapping, reconciliation, resource discovery and configuration UI belong to independently packaged extensions. Lumaserv is the explicit required example; it must not become a Core module behind a Cargo feature or runtime flag. Apply the same boundary to other third-party provider integrations during the inventory.

Core defines capabilities such as resource provisioning, observation and lifecycle operations in provider-neutral contracts. A plugin registers the capabilities it actually supports. A configured integration binds a plugin/version to an owner and credential references; an existing resource retains that binding when defaults change. Core contains no Lumaserv SDK, endpoint catalogue, provider-specific configuration branch or UI bundle, including when the integration is disabled.

Preserve and port the existing Webdock plugin platform rather than substituting Loco initialization hooks for runtime plugins:

1. **Discovery and lifecycle:** preserve the separate Tabularium registry contract, source/release installation, immutable image digests, manifest/descriptor validation, approval tied to the exact manifest, versions, health and runtime generation. Installed, enabled, configured and project-granted are distinct states. Upgrade/rollback requires compatible schema/data handling; downgrading an image alone is not a database rollback.
2. **Execution boundary:** external process/container, versioned gRPC contracts over the selected local/network transport and signed short-lived invocations; retain the existing HTTP contract only for the migration period. No arbitrary plugin code or native dynamic-library ABI in the Core process. First-party backend plugins are Rust; external authors may use any language implementing the protocol.
3. **Host capabilities:** migrate current admission checks and implemented services for project/app access, HTTP, secrets, jobs, data/storage and audit. Inventory declared versus actually implemented SDK services; unsupported capabilities fail clearly rather than receiving a dummy implementation. No plugin receives platform-wide database, Redis, Kubernetes or session-signing credentials.
4. **Provider access:** use the existing named, scoped outbound HTTP bindings where applicable. Core's generic binding validation controls destinations, credentials, redirects, timeouts and response limits; the plugin owns provider-specific logic. Credential grants are limited to the plugin/integration scope. Provider responses do not confer tenant authority.
5. **API, MCP and Console:** plugin actions share host-side authorization, project grants, capability checks, generation and idempotency rules regardless of entry point. Retain namespaced routes/tools and declared UI slots. Host-rendered declarative UI is the default; independently packaged custom UI is isolated on a separate origin/sandboxed frame with a validated message bridge. Do not import untrusted React code into the authenticated Console or expose backend invocation tokens to it. Preserve existing supported bridge behavior while restyling the host shell.
6. **Jobs/events:** preserve plugin ownership, tenant/project context, authorization revalidation, bounded retry/deadlines, cancellation, deduplication and at-least-once delivery semantics. Disabling/revoking access prevents new actions and dispatch; explicit draining may continue only the authorized recovery/reconciliation needed for retained resources.
7. **Persistence and uninstall:** use scoped host-service access or isolated plugin-owned storage, never unrestricted platform SQL. Preserve bindings and data on disable. Uninstall with resources, credentials, jobs or state outstanding must report dependencies and require an explicit retention/migration/deletion path. No cascade deletion as an implicit uninstall effect.

Feature and plugin enablement is enforced in API, MCP, jobs and execution as well as navigation. Missing plugin capabilities produce an honest unavailable/setup-required state. Installation does not imply approval to purchase or provision paid provider resources.

Lumaserv is the architectural acceptance example: an independently installable plugin can contribute configuration and supported resource actions through generic contracts without rebuilding Core; revoking it blocks new actions through API/MCP/UI/worker while retaining resource records. Implement only capabilities confirmed against the actual provider contract, using disposable test infrastructure for verification. The design does not assume a Lumaserv integration already exists or authorize live provisioning.

### Selected gRPC transport and runtime deployment contract

The owner asked to retain the agreed direction and extended it to both runtimes: new plugin runtimes communicate over gRPC from separate containers. Kubernetes uses independently managed Helm releases and a generic runtime chart; Docker uses Webdock's own supervisor/agent. Co-located Docker supports Unix-socket transport; distributed operation optionally uses authenticated network transport. This architecture decision is recorded persistently here. It is not implemented and does not silently invalidate existing HTTP plugin installations. Comparing alternatives does not reopen or replace this choice unless the owner requests it.

- On Kubernetes, run each plugin installation as its own Deployment/Pod and internal ClusterIP Service, preferably in a dedicated namespace with its own service account and resource limits. Core and plugins share cluster connectivity but not a Pod, database credentials or unrestricted network access. A namespace alone is not a network isolation boundary. On Docker, use a separately owned, resource-limited container with only its private socket and approved storage/network bindings.
- Use one maintained, constrained plugin-runtime Helm chart with separate releases per plugin installation. Values select the approved image digest, resources, configuration/secret references and approved connectivity; new providers do not require hardcoded changes to Core. Keep the Core release independent. Chart/image versions are pinned separately. Existing-resource and database-migration compatibility controls still govern upgrade/rollback and uninstall.
- The operator or existing restricted installer owns Helm execution. Webdock's request-serving Core and provider plugins do not acquire cluster-admin permission or permission to install arbitrary plugin-supplied charts. Verify that the installation's network implementation actually enforces the selected NetworkPolicies.
- Define a versioned, provider-neutral Protobuf protocol for runtime description/capability negotiation, invocation, operation status and event streaming. Use generated Rust clients/servers with a supported pinned Tonic/Prost release. Provider-specific actions retain manifest-declared, bounded schema-validated payloads within the generic invocation contract; do not compile a Lumaserv-specific RPC service into Core. Shared capability contracts are typed and versioned independently from plugin releases. Never reuse removed Protobuf field numbers.
- Authenticate network workload identities with mTLS; local Unix sockets use controlled filesystem/peer identity. Authorize each invocation in both modes using a short-lived signed context binding installation, runtime generation, tenant/project, action and approved capabilities. Core remains the authority; co-location or being on the same cluster network is not a grant. Host-service callbacks require equally scoped validation and do not accept self-asserted tenant identifiers.
- Default-deny network policy; permit only the required Core↔plugin service ports, DNS and approved outbound paths. Preserve the named outbound-HTTP broker for provider access where appropriate. Standard Kubernetes NetworkPolicy cannot enforce provider URL paths or general FQDN allowlists; use that broker or verified network-specific support rather than pretending it can.
- Use bounded unary RPCs for actions and streaming for progress/logs where it helps. Persist long-running operation identity/state in the platform before dispatch; return operation IDs, resume streams by cursor and recheck status after disconnects. Deadlines, cancellation and retries are explicit. A timed-out RPC can already have changed the provider; idempotency and reconciliation prevent duplicate provisioning. gRPC is transport, not a durable job queue.
- Browser traffic stays on the existing HTTP/SSE API and isolated plugin UI bridge. Backend gRPC endpoints have no public ingress, and React receives no plugin workload credentials. This does not require gRPC-Web or a service mesh.
- Add protocol-version/manifest transport negotiation and a bounded migration adapter for existing HTTP plugins. Port first-party plugins to gRPC while preserving their external API/MCP/UI contracts; remove the legacy transport only after the supported installations have a tested upgrade path.

Acceptance adds independent Helm rollout, identity/certificate rotation, incompatible protocol rejection, revoked-call denial, interrupted-stream recovery and duplicate-safe timeout/retry tests. A Lumaserv plugin must install and negotiate through this generic contract without rebuilding Core.

### Two supported plugin UI modes

The owner approved host-rendered standard UI and retained custom interfaces. Both are part of the target, not alternatives requiring a different plugin backend.

1. **Declarative UI by default:** a bounded, versioned manifest describes navigation/slots, panels, tables, lists, status, metrics/charts, forms and actions. The plugin returns schema-validated data; Webdock renders its own React/Tailwind components with shared theme, responsive behavior, accessibility and localization. Forms declare fields and action inputs, not JavaScript; submit actions pass through the same authorization/idempotency path as API and MCP calls. Secrets are write-only form inputs and are never echoed into panel data. The host owns text/URL/markdown sanitization, payload/depth/row limits, loading/error states and confirmations for destructive operations. No arbitrary script, CSS, expression evaluation or HTML injection in declarations.
2. **Isolated custom UI:** complex editors and specialized screens may ship separate static React assets on an isolated origin in a sandboxed frame. Provide versioned, validated bridge messages for navigation, appearance/locale, scoped data and actions. Check message origin, source frame and installation identity; authorize server-side on every action. Do not forward Core session cookies, provider secrets or backend workload tokens. Custom UI carries visible plugin identity and cannot impersonate platform security/account screens.

A plugin may mix declarative overview/settings panels with a custom editor. Shared capability and permission checks apply to both. Standard Lumaserv resource/settings screens should use declarations unless a demonstrated interaction requires custom UI. Existing custom-UI plugins remain supported during the transport/frontend migration; the new renderer is not grounds to delete their functionality.

## Own authentication; no Clerk in the target

Webdock owns its account database, credentials, sessions, organization/project membership and authorization. React renders Webdock's own account/login/security screens with no Clerk SDK, hosted login, Clerk JWT dependency or mandatory external identity service. Loco supplies useful authentication primitives and application wiring; its generated JWT login is not by itself the complete Webdock identity system.

Retain invitation-only access, password setup/reset, passkeys, MFA/recovery, session management and immediate revocation, operator bootstrap and the existing OAuth/OIDC/MCP relying-party contracts. Use established password/cryptographic/WebAuthn libraries. Prefer revocable server-side browser sessions with the existing host-only secure cookie and CSRF boundaries; do not replace them with long-lived browser-stored JWTs solely because a starter generates them.

The Clerk removal is an explicit authorized change to the prior compatibility floor. Inventory which identities actually use Clerk before migration. Preserve canonical local IDs and resource ownership. Any former Clerk-managed role or membership transferred to local authority needs a verified, auditable mapping and an explicit cutover; never turn stale projections into new local grants. For accounts whose credentials cannot be securely migrated, require account activation/reset and MFA/passkey re-enrollment through a verified ownership flow. Do not promise password or passkey export from an external provider. End old external sessions at cutover and test local sign-in before removing fallback infrastructure. No email-only automatic linking and no removal of the existing identity service during planning.

## Compatibility and migration rules

1. Preserve customer/project/resource IDs and relationships. Existing IDs remain unchanged decimal strings on every JSON boundary. New allocations use the approved internal ID library and coordinated durable allocation, retaining epoch/range and reserving old generator slots during overlap; switch defaults/callers only through an explicit tested migration.
2. Inventory all routes, host restrictions, request/response/error shapes, cookies, OAuth issuers/audiences, redirects, webhooks, MCP methods, agent messages and plugin methods. Mark each with its Rust replacement and a passing compatibility check. Folder or branding changes do not change Studio/OAuth/MCP contracts.
3. Local auth is a required workstream, not a permanent Better Auth sidecar. Preserve password-hash verification or provide a validated credential conversion; migrate encrypted TOTP material, recovery-code state and passkey records with the same RP ID/origins. Use maintained cryptographic/WebAuthn components, no new cryptographic primitives. Preserve native operator bootstrap, password replacement, MFA, invite-only access, revocation and explicitly mapped external identities.
4. Existing OAuth/OIDC clients and grants remain compatible. Port session/token formats only where verified; otherwise use a documented, tested reauthentication transition. Never issue new privileges from an email match. Migrate any Clerk-managed identity and permission authority through the explicit local-auth cutover above; preserve bans and revocations. Clerk is not retained as a target adapter.
5. Preserve encrypted secret envelopes and key references; verify decryption against disposable known-answer fixtures. Never log provider credentials or put them in browser DTOs.
6. Existing migrations and their tracking remain authoritative. New Rust migration tooling must adopt the existing history without replaying it. Use additive schema changes until the old code can be retired; verify constraints, sequence allocation, encryption and restrictive role policies on an isolated upgraded database.
7. During migration, one implementation owns writes for each domain/endpoint and one worker generation owns its leases. Shadow reads are allowed; uncontrolled dual writes and duplicate webhook execution are not. Webhook deduplication survives switching implementations.
8. Agent protocol rollout supports the old deployed version during overlap. Fencing and revision checks prevent two executors applying the same operation. Revocation and deletion authorization remain live checks, including retries and reconnects.
9. Existing storage pools, retained volumes, database bindings and Mail data are preserved. Suspension, draining and deletion remain distinct. No automatic reprovisioning as a shortcut to migration.
10. Rust ports of privileged helpers expose fixed typed operations, validate local peer identity and resource ownership, and do not add arbitrary shell/YAML execution.
11. Import/deployment plugins retain their signed manifest/image/capability boundaries. Backend TypeScript SDK consumers can remain external protocol clients; the first-party host moves to Rust. Do not invent a Rust ABI plugin system.
12. Every migrated domain has a rollback procedure tested with the retained previous binary/schema compatibility. Irreversible schema cleanup is a separately reviewed final change after rollback is no longer needed.

## Console visual and interaction direction

Visual direction: a floating, slim left sidebar; restrained grayscale surfaces; small line icons; compact labels; rounded inset navigation; a slim global search/status header; local breadcrumbs; and an app sidebar that replaces global navigation while retaining identity controls. Dark and light are both deliberately styled. Use Geist for display, a system body stack, Geist Mono for technical labels, 4px spacing steps and restrained borders/elevation.

Webdock will use this density and hierarchy with its own wordmark and orange accent. Most surfaces stay neutral; green, amber and red communicate verified operational states. Use Phosphor icons consistently. No decorative gradients, dashboard filler or oversized marketing typography inside the Console.

### Global workspace

- Top left: Webdock, active customer/workspace and project selection. Account/security controls are visually and behaviorally separate from workspace selection.
- Top bar: breadcrumb, searchable command palette (`Ctrl/Cmd+K`), activity/notifications and appearance toggle.
- Navigation: Overview; Applications and Deployments; Databases, Storage and Mail when enabled; CMS for website resources; Integrations and Settings. Operator infrastructure controls appear only to operators. Tenant preview restrictions remain enforced by the API.
- Overview: actual health summary, recent operations/deployments, items needing attention and relevant capacity. Empty installations show actionable setup; no invented traffic, uptime or deployments.

### Application workspace

The sidebar becomes app-scoped, with a clear way back to the application list:

- Overview: name, verified health, environment, endpoint, source/revision, current deployment and primary actions.
- Deployments: searchable history, immutable image/commit references, current release, failed attempts, live rollout detail and rollback.
- Traffic: domains, DNS/TLS status and routing information supported by the configured backend.
- Configuration: source/build settings, variables/secrets, resource limits and storage bindings.
- Observe: logs, metrics, deployment markers and activity. Never show stale observations as current health.

Use this detail hierarchy: summary first, a specific warning with a next action when necessary, then supporting diagnostics. Deployment pages show source/build, publication, scheduling, health/readiness and traffic-switch stages as applicable to the real backend. A failed stage includes its reason, timestamp and relevant logs; skipped stages are explicitly skipped.

### Other resource screens

Database screens show health, connection/access, resource limits, viewer launch and existing backup/restore capabilities. Mail and Storage retain their full workflows in the same visual shell. Integrations show configuration, verified capabilities, last check and actions; module availability comes from the server. Tables support filtering and pagination. Mobile navigation becomes a drawer and dense tables use deliberate local scrolling or alternate row layouts.

Every screen has loading, empty, stale, forbidden and error states. Keyboard navigation, focus return, labelled icon buttons, reduced motion, readable contrast and touch targets are acceptance requirements. Retain existing localization contracts and avoid hardcoded English-only replacement flows.

## Functional scope

| Behavior | Webdock requirement |
| --- | --- |
| App-first navigation, global search, resource context | Required in the redesigned Console, scoped to the current user's accessible resources |
| Git/image deployment with a readable timeline | Required; reuse and port Webdock's existing Git/build/publication lifecycle |
| Health-gated rollout and rollback to a known release | Required; use Webdock's existing revision, operation and readiness mechanisms |
| Live logs, metrics and deployment correlation | Required; port existing sources and add bounded real telemetry where missing; label freshness/no-data |
| Environment editor with masked secrets | Required, with explicit authorized replacement/reveal policy and audit |
| CPU/memory limits, storage, domains and TLS diagnostics | Required for configured providers that support them; server reports capabilities |
| Database detail and verified backup/restore history | Preserve existing functionality and make state/evidence legible; no new reliability claims from a dashboard |
| Templates/catalog and plugin lifecycle | Preserve existing supported Webdock flows and improve discovery, installation progress and capability display |
| Desired versus observed state and actionable conditions | Required in reconciled infrastructure domains; recover after crashes and reconnects |
| Additional products | Existing Webdock behavior is the parity floor. A new BaaS/Functions product, new billing, hundreds of new templates, or a replacement network mesh are outside this rewrite. |

First-party implementation is original and uses Webdock branding. External inspiration is not named in repository files, paths, commits, pull requests or other GitHub content. Do not copy third-party source or assets whose required attribution would conflict with that constraint; use independently authored code and properly licensed dependencies.

## Delivery sequence and completion gates

1. **Baseline and contract ledger.** Establish the isolated current checkout, resolve relevant stale-work differences, inventory runtime dependencies and existing tests, and capture contracts from disposable fixtures. Record the actual module/product state. No production reads or credentials are required to design this inventory.
2. **Rust core and identity.** Implement the Loco API/runtime configuration, SeaORM/database roles/migration adoption, tenant/project registry, audit, feature/plugin lifecycle, Webdock-owned identity and Clerk removal/migration, legacy OAuth/MCP compatibility and generated clients. Establish plugin host contracts/admission at this stage, before provider integrations. First complete flow: real sign-in → authorized project list → create/edit project → persisted audit → cross-tenant denial.
3. **Console shell and application vertical slice.** Implement React/Tailwind navigation, command palette, resource pages and real API queries. Port hosting/build/jobs sufficiently for create app → isolated build/publication → readiness-gated deployment → logs/metrics → rollback. An empty Rust shell plus fake app cards does not pass this gate.
4. **All remaining backend services and plugins.** Port the rest of hosting/agent/storage helpers, database gateway, Mail/outbox, CMS/storage/integrations, first-party plugin execution and retained compatibility handlers. Implement the own supervisor/agent with Kubernetes/Helm priority and complete Docker runtime support, local Unix sockets and distributed network mode. Provider implementations move to independently distributed plugins, including the Lumaserv acceptance example. Deliver declarative plugin UI plus the isolated custom-UI bridge. Complete the route/operation parity ledger; no silent feature removals.
5. **Distribution and migration rehearsal.** Build Core with configurable built-in features and separate provider-plugin images, offline migration/bootstrap tools and Helm/Compose updates. Verify standalone Docker startup without Kubernetes and cluster deployment with independent plugin Helm releases. Rehearse empty install, retained-data upgrade, process interruption, local/remote agent recovery, auth migration and binary rollback with disposable infrastructure.
6. **Retirement and final validation.** Remove superseded first-party backend runtimes only after their parity gates pass, update all callers/config/tests/docs, and produce a runtime/dependency inventory. Production deployment is a separately reviewable concrete release step, not implied by a local rewrite.

Milestones describe implementation order, not permission to call the rewrite finished after milestone 2 or 3. Keep completion status honest throughout.

## Verification

- Rust: format, Clippy, unit and API contract tests, SeaORM/database integration tests with restricted roles and real RLS, worker crash/retry/idempotency checks.
- Auth: password + MFA, passkeys, recovery, setup gates, tenant/organization changes, bans/revocation, audited external-to-local identity/permission migration, OAuth/OIDC and MCP compatibility, cookie/CSRF/origin restrictions, account migration and host-bound legacy endpoints. A new installation and a migrated installation must work with no Clerk configuration or network access.
- Infrastructure: real disposable Kubernetes workload rollout/restart/rollback, agent revocation and stale lease denial, bounded logs/metrics, isolated builds, artifact ownership, volume retention and backup/restore where supported. No production cluster used as a test fixture.
- Docker/agent: real Engine-backed plugin and application lifecycle, socket ownership and caller isolation, image digest checks, network/volume/secret isolation, CPU/memory limits, routing/readiness, bounded logs/metrics, restart/backoff and retained volumes after stop/disable. Run local-only without a TCP management listener. Test two enrolled execution targets with disconnect/reconnect, expired/revoked certificates and leases, generation fencing and duplicate-safe recovery. Test that the Kubernetes adapter observes native rollout/restart rather than fighting it.
- Products: database gateway isolation and revocation; Mail accepted-send deduplication, quota behavior and suspension; CMS/media compatibility and secret redaction; plugin signature/capability denial; disabled/draining module checks in UI/API/MCP/worker.
- Plugin boundary: Core builds/starts without provider packages or credentials; independently install/update the Lumaserv plugin without rebuilding Core; assert plugin identity/version/grant checks across API/MCP/UI/jobs, denied cross-tenant access and revoked/disabled actions, retained resources on disable/uninstall, and isolated UI/runtime credentials. Check Core dependency graphs and browser bundles for accidental provider SDK/code inclusion. Verify real plugin-host behavior, not merely manifest parsing.
- Browser: actual React build against Rust APIs, desktop/mobile, light/dark, keyboard flows, direct deep links, workspace/app context, error and reconnect states. Screenshots alone do not verify backend completion.
- Plugin UI: one real plugin exposing a declarative table/form/action and an isolated custom screen; verify theme/locale, keyboard/mobile behavior, bounded malformed responses, script/HTML/message-origin rejection, no leaked credentials and identical revoked/cross-tenant action denial in both modes.
- Upgrade: compare preserved row counts/IDs/relations and representative content with a sanitized fixture; prove old/new compatibility or the declared reauthentication path; validate rollback with writes made after cutover.
- Final acceptance: a customer can sign in through Webdock's own auth, manage a project, deploy an app, inspect its failure/success, roll back, and use existing Mail/database/CMS/storage features without invoking Clerk or a superseded first-party Node/Bun/Python backend. Core works with all optional features disabled and no provider plugins installed; enabled features use only explicitly configured/granted provider capabilities.

## Source links

- [Axum documentation](https://docs.rs/axum/latest/axum/)
- [SQLx documentation](https://docs.rs/sqlx/latest/sqlx/)
- [SeaORM documentation](https://www.sea-ql.org/SeaORM/docs/index/)
- [Loco application hooks](https://loco.rs/docs/reference/hooks/)
- [Loco authentication primitives](https://loco.rs/docs/how-to/jwt-auth/)
- [Loco background processing](https://loco.rs/docs/explanation/background-processing-model/)
- [gRPC concepts and streaming](https://grpc.io/docs/what-is-grpc/core-concepts/)
- [gRPC authentication](https://grpc.io/docs/guides/auth/)
- [gRPC deadlines](https://grpc.io/docs/guides/deadlines/)
- [Kubernetes NetworkPolicies](https://kubernetes.io/docs/concepts/services-networking/network-policies/)
- [Helm charts](https://helm.sh/docs/topics/charts/)
- [HashiCorp subprocess plugin approach](https://github.com/hashicorp/go-plugin)
- [Wasmtime sandbox security](https://docs.wasmtime.dev/security.html)
- [Extism plugin model](https://extism.org/docs/concepts/plug-in-system/)
- [Kubernetes Rust client](https://kube.rs/)
- [Docker Engine API](https://docs.docker.com/reference/api/engine/)
- [Kubernetes Deployment controller](https://kubernetes.io/docs/concepts/workloads/controllers/deployment/)

## Operational tooling

For later management of the retained Vercel deployments/integrations, installing the Vercel CLI with `npm i -g vercel` is strongly recommended: it supports environment pulls, deployment inspection and logs. It is not a dependency of the Rust platform and is not installed as part of this design review.

## Review status

An independent builder check on 2026-10-19 found no important unresolved planning questions ("nothing important left") in the initial design. Subsequent owner decisions are incorporated above: SeaORM, Loco, own auth without Clerk, optional built-in Core features, external provider plugins, own supervisor/agent, Kubernetes/Helm priority plus Docker, local/optional network gRPC, and declarative/custom plugin UI. That earlier check predates these revisions. The owner has approved implementation, including the publication constraint above. Continue execution without another design-approval round.
