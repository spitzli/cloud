# Neon database plugin

Owner request: Neon is an independently published Spitzli plugin, initially focused on databases, with room for other Neon services later. All customer connection metadata and encrypted provider credentials remain in the existing Core database. Existing native connection management, MFA/passkeys, tenant authorization and application deployments must continue working.

The first release connects **existing** Neon databases. It browses authorized projects, branches, databases, roles and endpoints; attaches an existing tuple to a Core project; displays its current status and nonsecret connection settings; provides a separately write-authorized URI reveal; and disconnects the attachment without deleting provider data. API key remains the default; existing CLI instructions remain optional. Creating paid projects/compute, executing SQL and changing/deleting Neon branches are outside this first release. Auth, Storage, Realtime and other services are documented future capabilities, never simulated features.

Implementation choice: a stateless native v2 extension `neon` in independent `spitzli/cloud-plugin-neon`, exact pinned published SDK, existing gRPC/Unix or mTLS runtime and declarative UI. Core supplies an optional named dynamic HTTP adapter, reusing current capability and admission checks. The plugin owns its action/response contracts, provider-oriented database presentation and UI. Core alone resolves customer keys, fixes the Neon HTTPS destination, enforces project/organization scope and maintains protected attachment metadata. No separate metadata server or plugin PostgreSQL database is introduced.

The named `neon` callback supports read-only permitted Neon API paths and structured, bounded query parameters, without weakening legacy static HTTP path validation. Core handles the two internal attachment commands (attach/detach) and constructs/validates secret-bearing connection URI requests against the stored resource binding. It must use the admitted action, resource ID, customer, project and session from Core, never caller-selected identity. Browser authority is supported first; unsupported MCP/job authority fails closed. All normal native generation, peer, grant, capability, resource and before/after-IO fences remain in force.

The Core database stores attachment IDs, external Neon project/branch/database/role/endpoint, connection identity/revision, immutable local idempotency receipts and revisions under forced RLS. No provider API key or database URI goes in generic plugin JSON, manifests, runtime configuration, logs, cache keys, source or build context. Connection URIs are delivered only by explicit write-authorized reveal and never stored in ordinary operation results/receipts. Key replacement/disconnect, membership/session/preview changes and attachment changes during IO invalidate results. No transaction spans provider IO.

Publication requires an independently buildable OCI image, immutable release assets and signed registry admission, icon, README and actual UI screenshots with synthetic fixture data clearly labeled. The public registry must be checked after publication. Own GitHub CI uses Blacksmith only; manual native qualification/builds use the existing EU build server with full-process-sandbox rootless BuildKit. Existing tags/signatures are not overwritten. Core rollout requires owned PostgreSQL fixture/child validation and a targeted recoverable deployment; publication alone is not activation.

The concurrent catalog repair uses immutable-source owner/admin refresh for presentation only. The owner separately selected removal of `lumaserv`, `managed-analytics`, `managed-apps`, `project-notes`, and `postgres` catalog entries. Their metadata and release identities are backed up first; installed workloads, Git repositories, images, credentials and customer volumes are preserved.

## Frozen first-release callback contract

`HostClient.call(token, "http", "request", request)` remains the wire ABI. The optional named adapter consumes only `binding:"neon"`; other bindings retain their exact existing validation. The request has `method`, relative `path`, optional bounded structured `query`, optional JSON `body`. No origin/header/identity fields. It returns the existing `{status,body}` envelope.

Core constructs a nonserializable adapter context from persisted admission: invocation ID, scope, exact action/intent, request hash, resource ID, current Principal and Browser StoredAuthority. It checks generation/peer/grant/capability before and after callbacks; unsupported jobs/MCP fail closed. Every POST below requires `plugin.http.request` and exact write action even if it only fetches a provider GET.

Native actions: `databases.discover` (read, customer, feature `connect-databases`, no resource; used by custom Create UI), `databases.attach` (write/create offering `database`, feature `connect-databases`), `databases.view` (read/use), `databases.branches` (read/use), `databases.credentials` (write/use), `databases.detach` (write/delete). The discover input supports selected Neon project/branch and a cursor; the UI must provide selectors, not require people to copy opaque IDs by hand. The attach input is `{name,neon_project_id,branch_id,database_name,role_name,endpoint_id}`. Resource inputs use `resource_id`; credentials also accepts `pooled` (default true).

Inventory: permitted Neon project/branch/database/role/endpoint GET paths with structured query. Core projects safe fields before returning them; role responses must never expose a password. Virtual callback paths:

- `POST /_spitzli/databases/attach`: body is the original attach action input. Check canonical hash against admission. Validate the tuple with the selected connection, commit binding + public Core resource index + immutable public receipt + reservation settlement atomically; return `{resource:{id,name}}`. The final generic resource-record operation must be idempotent. A lost runtime reply replays that public receipt on fresh admission.
- `GET /_spitzli/databases/current`: resource solely from trusted context. Return safe resource/binding/status fields, never URI or provider key.
- `GET /_spitzli/databases/branches`: branch inventory for the stored, authorized Neon project; no caller-selected resource.
- `POST /_spitzli/databases/credentials`: body is the original admitted input; resource and tuple from Core. Require exact write action, construct provider connection URI query from stored tuple and explicit pooled option. Return `{value:uri}` only after full revalidation, no stored receipt/cache/log.
- `POST /_spitzli/databases/detach`: original admitted input, no provider-side deletion. Atomically tombstone binding/remove Core resource index/record immutable nonsecret receipt. Exact receipt replay is checked under current authority before missing-resource admission denial; changed input conflicts.

This first version needs no SDK wire version change. Publish independent SDK dependencies at an existing reviewed exact revision. An additive forced-RLS attachment/receipt migration is permitted when needed for atomicity; prior migrations remain byte-identical. Core invocation responses for sensitive reveal carry `Cache-Control: no-store`. Member/preview status must not advertise connection management they cannot execute.

## Initial region policy

Customer database region requirements retain the existing EU policy (the Blacksmith exception covers own CI only). For this first release, attach/view/reveal provider checks require actual project.region_id `aws-eu-central-1` (Frankfurt); discovery may show other projects but UI disables their attachment. Unknown regions fail closed. Local detach remains possible. Other EU regions require a reviewed policy extension; no existing provider project is moved/deleted. Official Neon Regions documentation checked2026-11-02 lists Frankfurt as the current AWS EU region (London is UK); deprecated Azure projects remain outside this initial integration scope.

Provider-wide discovery/branch inventory retain the workspace-management authorization of the existing Connections inventory, even though their native actions are read-only. Authorized read-only customer preview can read safe settings only for its admitted attachment. Ordinary membership alone does not grant Neon actions; existing database-manager permissions are preserved. Local detach uses write/resource ownership, not the new-attachment feature, so a revoked creation entitlement cannot strand an existing attachment.

## Existing permission boundary confirmed during qualification

Migration 0071 already restricts provider connection reads to database managers
or an authorized read-only customer preview. The first plugin release preserves
that policy. Ordinary membership alone does not grant Neon actions. No broader
provider-key RLS policy is introduced to make a fixture pass. Workspace owners,
administrators and authorized operators retain management; authorized read-only
preview retains its existing safe settings view and cannot reveal credentials.
