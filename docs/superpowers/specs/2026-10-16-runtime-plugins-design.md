# Runtime plugins with Tabularium

Status: implementation handoff design, 2026-10-16. No plugin host, registry deployment or product migration is claimed as delivered by this document.

## Intent and delivery boundary

An operator can start a Webdock installation with a selected product X, find plugin Y in a Tabularium registry, install and activate Y while Webdock is running, and later deactivate, reactivate, update or uninstall Y. Installing Y does not rebuild or restart the Console or Control Plane. Plugin runtime containers may start, stop or restart.

The delivery contains the runtime milestones below plus the required [developer platform amendment](2026-10-16-plugin-developer-platform-design.md), added after the owner's explicit request for a scaffolding CLI, code APIs for DB/Cache and a study of Tabularis:

1. All six currently bundled modules can be controlled live through persisted installation state, with safe admission and shutdown across API and worker replicas.
2. A separately built Project Notes plugin can be installed from Tabularium and used through its own UI, API and scheduled job without being included in Webdock's application build.
3. A developer can create that plugin through `@spitzli/cli` and use the public `@spitzli/plugin-sdk` for real DB, Cache, jobs/events, configuration/secrets, storage/HTTP, logs/audit and typed UI slots. The companion specification owns the exact service contracts and limits; SDK/CLI delivery is required before declaring the combined plan complete.

This is not a claim that the existing six products have all been removed from the monolith. Their later extraction is a separate migration program. In milestone 1, disabling a bundled product changes availability but does not remove its code from the image. In milestone 2, new external plugins really are absent from the host build. Do not call this a physically minimal core-only distribution until the remaining built-in dependencies have been extracted.

Planning choices: initially support operator-approved publishers and registries, one installed version of a plugin per Webdock installation, Kubernetes as the production runtime, and an isolated iframe UI. Installation-wide activation is distinct from customer entitlement and project access. The operator selects X; do not silently choose Hosting or Mail as the default. Dependencies of X are included explicitly. Existing installations retain their previous effective selection.

## Sources and current constraints

Read the current source as well as these records:

- `docs/architecture/module-installation.md`, `modular-platform-status.md`, `mail-deployment-decision.md`, `hosting-runtime-boundary.md`, `storage-module-boundary.md`.
- `packages/module-sdk/src/{index,selection,react}.ts*`: static manifests, three availability states and public revision.
- `apps/control-plane/src/{installation,config,index,app,worker,jobs,module-preflight}.ts`: environment configuration, startup route selection, fixed worker handlers, existing leases and shutdown inventory.
- `apps/control-plane/src/protocols/legacy-auth.ts`: numerous routes are currently omitted at startup when a module is disabled. A live state store alone cannot enable them.
- `apps/control-plane/src/modules/portal/studio-api.ts` and `packages/api-client`: session projection and authenticated browser operations.
- `agents/kubernetes/{daemon,connect,permissions,executor,databases,mail_executor}.py`: existing agent authentication, generation, lease, checkpoint and reconciliation patterns.
- Tabularium inspected at commit `5d1818352f7e4a018f6ace31a7f185d5a6b6f292`: <https://github.com/TabularisDB/tabularium/tree/5d1818352f7e4a018f6ace31a7f185d5a6b6f292>.

The working tree already contains substantial uncommitted modularization. Preserve it. Do not reset, clean, stage all files, rename the npm workspaces, or build this work from HEAD while ignoring the current files. Package names are still `@spitzli/api` and `@spitzli/admin` despite their new directory names.

## Architecture and ownership

Tabularium owns discovery, publishing, versions and integrity metadata. The Webdock core owns trust configuration, installation records, operator decisions, entitlement checks, audit, secrets, runtime control and capability enforcement. The container registry owns executable OCI artifacts. Existing customer/project registry and platform persistence remain core services; they are not Tabularium.

The Plugin Controller is a separately started process from the Control Plane workspace, independent of optional product worker selection. It records durable operations and asks the existing authenticated Kubernetes agent to reconcile a restricted plugin workload. It holds no cluster-admin credential. The agent's plugin support is core transport and must work when the customer Hosting product is disabled.

For v1, the Control Plane API, controller and plugin workloads share the managed Kubernetes network. API/controller invoke the plugin's private Service; no public arbitrary-upstream proxy or reverse tunnel is introduced. External-network API hosting needs a later authenticated transport extension. Generate Services and their destinations from installation IDs and runtime generations, not URLs supplied by a plugin. Use cluster TLS for private requests, a generation-specific runtime credential and the per-invocation token; credential rotation/revocation belongs to lifecycle operations. A configured namespace-scoped cert-manager Issuer supplies per-Service certificates from host-generated Certificate requests; API/controller trust its configured CA bundle and validate the expected Service DNS name. The issuer/CRDs are infrastructure prerequisites, never a silent production install. Mount only that runtime's leaf certificate/key plus an immutable host invocation public-key bundle. Plugins never receive a CA key or host signing private key. Missing/expired credentials or unknown host keys fail closed; rotation uses a host-controlled overlap of public keys and a new runtime configuration generation.

Plugins execute outside the API process. Do not use `npm install`, remote JavaScript imports, arbitrary Helm charts, arbitrary Kubernetes YAML, or plugin-defined shell commands in the host. The host generates workload objects from a validated manifest and operator policy.

Each plugin gets an isolated workload identity, no Kubernetes service-account token, no host mounts, bounded resources, non-root execution, a read-only root filesystem and an ephemeral writable directory. Network policy permits only required host capabilities and UI ingress; the pilot needs no Internet egress. The runtime has no platform database password, identity signing key, provider credential collection or Mail encryption key.

## Availability and lifecycle

Keep public module availability `enabled | draining | disabled`. Store package installation, runtime health and lifecycle-operation progress separately; an installation error is not a fourth availability state.

- Install verifies and records a release and configuration. Default availability is disabled; it does not grant customers access.
- Enable checks dependencies, operator-approved capabilities, configuration and runtime readiness before publishing enabled state.
- Disable first publishes draining under the same admission fence used by new work. New provisioning and new user work for external plugins are refused. Already accepted operations, required callbacks and reconciliation finish or report blockers.
- Full disabled state is published only after authoritative preflight confirms no retained operational obligations or active execution. A timeout yields a visible blocker; it never force-stops Mail or abandons billing work.
- Reactivate uses retained data and configuration. No resource IDs or provider bindings are silently recreated.
- Uninstall removes the executable workload and registration from active discovery, but retains plugin data and operation history. Purging data is not included in v1.
- Update stages a verified candidate, checks compatibility, drains the old runtime, stops its job ownership, applies approved declarative migrations through the dedicated plugin migrator, starts the candidate and switches routing only after readiness. Old and new versions must never both own scheduled work. A plugin-specific interruption is acceptable; core and other plugins stay available.
- A failed compatible update restores the previous executable version. Never promise rollback across an incompatible data transformation.

Disable, uninstall or version change is blocked while any enabled/draining dependent requires the target's current exact version. Return the dependent IDs and required versions. There is no automatic cascade or coordinated multi-plugin upgrade in v1: the operator must drain/disable dependents explicitly first. Check the whole affected dependency graph in the same transaction and stable lock order as lifecycle mutation so concurrent dependent activation cannot race past this blocker. Disabled dependents may remain installed but cannot reactivate until all exact dependencies are satisfied.

Mail rules are unchanged: incoming email is explicitly enabled per customer, Stalwart CE is isolated per customer, outgoing-only does not provision Stalwart, suspension retains storage, deletion is separate. Retained Mail resources may keep the module draining indefinitely until explicitly resolved or handed over.

## Distributed admission and bootstrap

The database is the authority; browser state, environment variables and process-local caches are not. Add a numeric installation generation/revision and compare-and-swap mutations. The operator supplies the revision they reviewed. Conflicting commands return 409 without partial effects. Repeated idempotency keys return the same operation; a changed body under the same key returns 409.

Implement a shared/exclusive database fence for admission versus state changes. New mutating work validates enabled state and records its durable operation inside a transaction holding the shared fence; draining/disable acquires the exclusive fence. Already accepted continuations carry recorded operation identity and retain the existing provider/agent generation and lease checks. A process-local snapshot must not independently authorize work. Check current state and authority at job claim, lease renewal and externally visible commit/checkpoint boundaries. Do not hold database transactions open across provider HTTP calls; record intent/outbox under the fence and perform the side effect through the fenced durable operation.

Callbacks needed to resolve an existing resource are authenticated and admitted as continuations, never reclassified as fresh provisioning. Existing `requireMail` and other synchronous ports need a complete caller audit when introducing async checks. Avoid changing a callback to return a Promise while leaving callers unawaited. Startup route composition must mount owned built-in routes independently of current availability, with per-request gating. Process ownership remains explicit; enabling a module on an installation with no suitable API/worker owner returns a blocker, not success. Workers advertise supported domains and protocol version through expiring database heartbeats.

`SPITZLI_MODULES` is an explicit bootstrap input after migration, not an ongoing second authority. Use an offline initialization command to import the existing effective selection once. Missing legacy configuration means the current legacy selection (all six enabled), not an empty installation. A new installation must choose a default selection explicitly. A later environment change must not overwrite operator decisions. Before enabling live control, roll every API/worker/agent participant to the new protocol and remove old replicas; the one-time cutover is not itself a zero-restart migration. New API readiness requires an initialized state store. Core startup must not silently fabricate that state.

## Tabularium contract and trust

Use a dedicated configurable registry URL, not a hardcoded public deployment. Configure kind `extension` and required runtime fields at the manifest root using Tabularium's per-kind schema support. Unknown extension properties may be stripped by Tabularium, so schema installation and a round-trip check are prerequisites to publishing.

The v1 kind-specific fields are `protocolVersion: 1`, `sdkVersion: 1`, a core-version compatibility expression, exact dependency versions, an OCI `image` reference pinned by SHA-256 digest, configuration JSON Schema, capability names, action declarations with `read | write` intent, navigation/typed slot entries with relative UI paths, and optional named jobs with a minimum 60-second interval. The developer amendment adds required/optional service versions, declared tables/events/secret/HTTP binding names, `dataProtocolVersion: 1`, `schemaVersion`, readable-schema range and checksummed declarative migrations. Fix backend port 8080 and endpoints `/healthz`, `/readyz`, `/webdock/actions` and `/webdock/jobs`. Reject unknown privileged fields. The manifest declares no arbitrary host URL, namespace, RBAC, filesystem mount, executable migration, command or raw SQL. Keep the schema within Tabularium's supported extension depth/property limits and verify round-trip retention.

For v1 use JSON manifests, a bounded schema subset and one small release asset named `<id>-universal.json` containing the same manifest bytes. This makes the existing Tabularium release resolver useful without adding OCI support to Tabularium. The runtime image itself lives in an operator-approved OCI registry. Reject ambiguous asset selection and duplicate JSON keys before interpreting security-relevant manifest data.

The actual checked API is:

- `GET /api/plugins?kind=extension` for discovery.
- `GET /api/plugins/:id/releases/:version` for pinned asset resolution.
- `GET /api/plugins/:id/releases/:version/integrity` for signed JWS, hashed assets and `manifest_raw`.
- `GET /.well-known/registry-key.json` for registry keys.

Verify JWS algorithm/key against operator-established trust, signed `registry`, `plugin_slug`, `release_version`, manifest hash and selected asset hash/size. Parse only verified bytes; validate core compatibility, dependency closure, permissions and image digest before installation. Store the verified manifest bytes, digest, image digest, signer identity and exact version locally. Never use mutable catalog extensions to launch a pinned old release. Missing signatures or `manifest_raw`, yanked versions, changed bytes at the same version and unknown signing keys block new installs/updates. Key rotation requires a separately authenticated trust update, not silently accepting whatever JWKS returns. Existing installed releases continue running without the registry.

Use an HTTP timeout of 10 seconds, maximum manifest/descriptor size 256 KiB and at most three validated redirects. Allowlist registry, artifact and OCI origins; reject userinfo, non-HTTPS, private/link-local destinations and DNS/redirect rebinding for public sources. Explicit operator-configured private registry endpoints and isolated test endpoints are separate allowlisted policies, not a global SSRF bypass.

Tabularium's consumer documentation differs from the inspected code: integrity uses the separate endpoint, the signed identity fields are `plugin_slug`/`release_version`, `manifest_raw` is implemented, and `verifyAssetHash` returns an object with `.ok`. Test against the pinned server. Its TypeScript client currently exposes a loose `any` proxy; prefer a narrow validated HTTP adapter using existing `jose`/`zod`, not assumed generated type safety.

## Capabilities, data and UI

The capability catalog includes the original `project.read` and `plugin.data.read/write` plus the developer amendment's DB, cache, jobs/events, storage, named secrets/HTTP and audit capabilities. They are intersected with installation approvals, customer grants, current membership, project ownership, preview restrictions and the specific invocation intent. Declared service resource names impose further limits. Plugin activation alone grants none of these; dynamic capability discovery cannot expand a grant.

Grants are explicit for `(plugin installation, customer, project)` and may separately enable the plugin's declared scheduled jobs. A job runs as this durable project grant, not an expired browser session or a fabricated operator. Revalidate the grant, active customer/project, runtime generation and requested capabilities at scheduling, claim and capability use. Removing a member revokes that person's actions; revoking the durable job grant stops future job work independently.

The controller owns the external-plugin scheduler. Persist one job execution per `(installation, customer, project, job name, scheduled slot)` with a unique constraint, claim token, lease expiry and attempt counter. Use 60-second leases renewed every 20 seconds, at most eight attempts, one-minute retry delay and a 30-second HTTP execution budget per attempt. Delivery is at least once. Retry uses the same execution/operation identity, and data writes store idempotency receipts atomically, so a crashed response does not duplicate effects. Draining stops new slots; previously accepted executions may finish/retry under continuation authority. There is no unauthenticated plugin-owned cron loop. Candidate generations never schedule until activation; old generations cannot renew after ownership transfer.

Plugin persistence includes the host-provided 64 KiB JSON document API (`ctx.data`) and real plugin-owned relational tables (`ctx.db`), each scoped by plugin installation, customer and project. The developer amendment defines typed queries, atomic command batches, RLS, generated additive migrations, a separate limited migration role, and readable-schema compatibility. Cache (`ctx.cache`) is a separate expiring shared service. No plugin receives platform SQL credentials. Documents, tables, objects and migration history are retained on disable/uninstall. Arbitrary SQL/code migrations, destructive transformations and backfills remain unsupported in this first delivery.

Every backend invocation receives a short-lived signed token (maximum 60 seconds) bound to plugin, runtime generation, action/job, customer, project, actor or durable job grant, approved capabilities and operation ID. The host capability API rechecks current state, generation and grant revocation; token lifetime alone does not establish authority. Bind mutation idempotency to invocation identity and payload. The per-runtime credential proves workload identity but cannot on its own select any customer or access their data. Never use caller-supplied customer/project headers as authentication.

UI is served as static assets from `/ui/` on a dedicated origin per plugin generation, on a separate registrable domain from Webdock authentication. Only UI assets are publicly exposed there; internal backend endpoints are not. Console embeds the page with a restrictive iframe sandbox and CSP. No authentication cookie, provider secret or backend invocation token is sent into the frame or a URL.

The iframe uses a versioned `postMessage` bridge to ask the parent to invoke declared actions and the bounded UI operations from the developer amendment. Parent validates `event.source`, exact origin, protocol version, request ID, declared action/UI method and message size (32 KiB); it uses the existing authenticated Console transport. The backend determines customer/project scope and authorization. The plugin can receive only the public launch context and results it is authorized to read. Disabling the plugin invalidates active bridges and stale tabs; UI refresh is for responsiveness, never the authorization boundary. Typed project/operation settings slots use this same isolation and lifecycle, not direct React imports into the host.

The generic tenant route requires an explicit project ID selected from authorized projects; never infer it from a tenant's first project. The operator UI includes catalog search, verified release inspection, capability approval, configuration/secret-reference editing, project/job grants, lifecycle operations and recoverable errors. Installing a release carries the manifest hash the operator inspected, preventing a same-version change between review and execution from silently changing the approved program.

## Pilot and evidence

Project Notes is generated with the public CLI into a separate example package outside root application workspaces. It lets an authorized customer/project member read and update a Markdown note (16 KiB maximum), and runs a scheduled job that stores its last successful maintenance timestamp. It uses public SDK APIs for a relational table/additive migration, scoped cache, a local event, an attachment and a typed project slot. Host object-storage configuration is supplied by the local fixture; plugin code gets no provider credential. Render Markdown safely or show plain text; never inject raw HTML. Separate conformance cases exercise named HTTP/secret bindings. Store and access data exclusively through the public capability APIs.

Package two compatible versions and one deliberately unhealthy version for the update exercise. Build the host once, then build/publish the plugin into an isolated local Tabularium/OCI fixture. Install Y after the host is running; unchanged host image digests and process start times are part of the evidence. X is a selected built-in module in this first demonstration, explicitly documenting that its code remains bundled.

Acceptance includes two API replicas, two workers/controllers competing for claims, two customers, a second project, membership/grant revocation, registry outage, interrupted install, duplicate commands, stale agent completion, tampered manifest/image reference, failed readiness, retained uninstall/reinstall and compatible rollback. Real container startup and HTTP/browser behavior must supplement unit tests. Local Kubernetes must enforce NetworkPolicy; an ordinary Docker test alone is not production-isolation evidence.

Production registry setup, public release publication and migrations against customer databases are separate operational actions. Implement code, charts and a concrete runbook now; execute production actions only when the execution session explicitly authorizes them. The approved Contabo destination is not proof of access credentials, a domain choice or authorization to mutate the cluster in a planning task.
