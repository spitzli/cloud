# Native tenant database browser with Tabularis Web

Status: implementation authorized on 2026-10-10; production activation is not part of the current work.
Date: 2026-10-10.

## Intent and confirmed requirements

Webdock customers and platform operators should use Tabularis to browse their assigned project databases inside Studio. Access depends on explicit database permissions. The user is a Tabularis maintainer and requested design and ideas before implementation. This agent owns both phases; implementation uses medium reasoning effort.

The user selected Tabularis PR [#676](https://github.com/TabularisDB/tabularis/pull/676) as the starting point for the web package. Reuse its `packages/web-ui`, typed client and Rust web transport. The verified starting commit is `aeb589e6a3fdd091184a3693bb2f8629bff46f96` on `feat/web-ui`; an embedding change should be developed on a separate branch based on that commit. Selecting this base does not decide between isolated runtimes and shared upstream tenancy.

Work happens in `t3code/native-db-browser-tenant`. RelayKit and Git deployment work are proceeding in separate worktrees. Database browsing must not take over their services or deployment workflows.

“Native” is interpreted here as an integrated Studio workspace using Tabularis components, existing Webdock login and a selected tenant/project context. This interpretation, the initial PostgreSQL scope, and the runtime strategy below are proposals, not yet user-approved decisions.

## Findings from the existing code

Webdock already has the required identity foundation:

- `webdock_admin.customers` and projects are the registry authority; `webdock_auth.tenant_customer` maps customers to Better Auth organizations.
- Studio delegates authenticated operations to Auth. It does not receive unrestricted identity database access or the platform encryption key.
- Hosting authorization rechecks the native session, active customer/project, membership and operator MFA. Read-only customer preview denies live operations.
- Some existing websites share a PostgreSQL database with distinct schemas and restricted logins. A schema name or UI filter alone cannot be the SQL authorization boundary. Existing preview credentials must not be assumed to identify an isolated preview database.

Tabularis Web exists in the open PR [#676](https://github.com/TabularisDB/tabularis/pull/676), branch `feat/web-ui`. The inspected revision is `aeb589e6a3fdd091184a3693bb2f8629bff46f96`.

- `packages/web-ui` contains the React/Vite application, a typed client and HTTP/WebSocket transport.
- Its package is private and currently has no public component exports. `App` owns its router and application shell. Native embedding needs an explicit entry point; it is not an existing drop-in npm component.
- The Rust server supports authenticated proxy access, CSRF/origin checks, bounded queries and session-owned operations.
- `ApplicationRequestContext` carries authorization level and session ID, but no tenant or actor identity. `RuntimeApplicationApi.get_connections` ignores that context and loads the runtime's common connection file.
- Proxy authentication validates `X-Tabularis-User`, but the session record does not retain that user as an authorization principal. An existing Tabularis cookie can authenticate without repeating proxy-header authentication.
- Remote `database` permission includes queries and record editing. It does not mean read-only SQL or per-connection authorization.

Consequently, a shared Tabularis process behind Webdock login is not sufficient tenant isolation. The findings are source inspection, not a claim that the upstream branch was built or penetration-tested here.

## Approaches

| Approach | Benefit | Cost / limitation |
| --- | --- | --- |
| Native Tabularis UI with isolated execution workspaces — recommended first delivery | Reuses Tabularis functionality while isolating its existing runtime-global state | Requires an embedding entry point, an authenticated gateway and runtime lifecycle support |
| Shared tenant-aware Tabularis service | Efficient resource sharing and a reusable upstream hosting model | Requires tenant/actor/connection policy throughout persistence, pools, plugins, events, exports and background operations |
| Separate Tabularis application opened from Studio | Smallest initial UI integration | Does not fully satisfy the proposed native Studio experience; still requires the same authorization/isolation work |

The first approach reduces the amount of upstream tenancy refactoring required to ship a correct first version. A shared service can be reconsidered after actual resource measurements. Runtime isolation is not a replacement for restricted database credentials.

## Proposed user experience

Entry point: **Tenant → Databases**, with an additional entry from the owning project. The directory uses compact rows: database name, project, actual environment, engine, connection state and the current user's permitted access. It follows Studio's existing light/dark themes and English/German interface conventions.

Opening a database enters a full-height workbench:

- One context bar: tenant, project, database, environment and access level, plus a return action.
- Left: schema/table tree with search.
- Main area: table and SQL tabs, with the existing Tabularis editor and results grid.
- Query actions: run, cancel, execution status, row count, pagination and visible errors.
- Data changes: available only with the appropriate grant, with Tabularis change review and clear production context.

The Studio shell supplies product navigation; Tabularis supplies the database workspace. Avoid duplicate global navigation. On narrow screens the schema tree becomes a drawer and result scrolling stays inside the grid. Changing tenant/database must not silently rebind an existing SQL tab or discard unsaved SQL.

Disconnected, starting, permission-revoked and credential-unavailable states are distinct. A failed request is not an empty database. A write with an uncertain outcome must not be automatically replayed.

## Authorization and database bindings

Introduce an explicit database binding:

`customer → project → database binding → environment → allowed database/schema scope`

Each binding references a controlled connection configuration and encrypted credentials by purpose. It records engine, lifecycle state and revision. No automatic discovery or extraction of a project's `DATABASE_URL` from arbitrary application environment variables.

Database grants belong to a binding and an authenticated user, with an optional explicit owner/admin membership policy configured on that binding. Ordinary tenant membership, CMS access, or being a tenant administrator does not automatically grant database access.

Proposed access profiles:

| Profile | Intended authority |
| --- | --- |
| Browse | Authorized metadata and SQL reads |
| Edit data | Browse plus permitted row inserts, updates and deletes |
| Manage schema | Edit data plus explicitly permitted DDL within the target scope |

Operators receive a separate, audited policy and retain their real identity. Customer preview displays registry metadata only and cannot open a database session.

SQL privileges must be enforced by database roles, including grants on schemas, tables, sequences and callable functions, and restrictions on role changes. Do not use SQL-prefix checks or hidden buttons as the read-only boundary. Do not use the application owner's or control-plane credentials. For shared PostgreSQL databases, validate cross-schema denial and PUBLIC/default privileges with actual SQL probes before enabling a binding.

Permissions also govern exports, metadata, cancellation and events. Revocation or a permission downgrade invalidates existing browser sessions and closes their active streams. In-flight work is cancelled best-effort; already committed writes cannot be undone by revocation.

## Components and request flow

1. **Studio** renders the database directory and embedded workbench. The frontend selects an opaque binding ID, never an upstream host, secret reference or tenant identity to trust.
2. **Auth database service** resolves that binding, checks the live Webdock session and explicit database grant, and issues a short-lived, single-use launch authorization bound to actor, session, customer, binding and grant revision.
3. **Database gateway** exchanges the launch authorization for a secure browser session and authorizes every RPC, transfer and event connection. It controls upstream routing and headers. A Tabularis cookie alone cannot bypass Webdock authorization.
4. **Isolated Tabularis runtime** executes through the authorized database role and returns bounded results. SQL/result traffic travels through the gateway, not the Studio-to-Auth business-command bridge.

The gateway and long-lived Rust runtimes belong on controlled server/container capacity close to the target databases. Do not route interactive queries through the hosting agent's asynchronous operation queue. Remote customer infrastructure requires a separately designed connector; an outbound inventory agent does not already supply an interactive database tunnel.

The native client needs a supported transport arrangement. Preferred: serve the database route and gateway API through one browser origin with explicit path routing. Verify that routing with the existing Studio host before implementation. A separate gateway origin needs an explicit cookie/CORS/CSRF/WebSocket design; the current transport's `same-origin` credentials setting cannot simply be pointed at another hostname.

## Runtime isolation proposal

Initially scope each active runtime to one database binding, actor and database access profile, within its owning tenant. Isolate data directories, secrets, pools, temporary transfers and network access. Two actors in the same tenant can have different database grants and private query history, so a single unrestricted tenant-wide process would still be insufficient.

Runtimes start on demand with finite concurrency, memory, connection and query-duration limits; idle runtimes stop. Persistent workspace state, if enabled, is owned by the same scope and cannot be mounted into another scope. Disabling a binding revokes access without deleting customer data or saved work automatically.

Only the bound database destination is reachable. Users cannot add arbitrary connection hosts, change credentials, install plugins, start SSH/Kubernetes tunnels, use host administration or enable MCP through this workspace. Those restrictions apply at the backend command boundary, not only in the UI. Default-deny unrecognized RPC commands after upgrades.

The initial package includes vetted drivers installed by the operator. Tabularis remote high-risk mode remains disabled. Verify a noninteractive, isolated credential backend for the packaged service; the desktop OS keychain must not be assumed to work in a container.

## Native UI and upstream integration

Propose an upstream embedding entry point, conceptually `TabularisWorkspace`, with an injected typed client, initial connection, capabilities, locale/theme inputs and lifecycle cleanup. The host owns outer routing and login. Preserve the ordinary desktop and standalone web entry points.

Build this entry point from PR #676's existing implementation. Keep the embedding API host-neutral: Webdock-specific organization IDs, OIDC delegation and database-grant storage belong in the integration, not in the reusable UI package. Any later upstream tenancy extension is a separate architectural decision.

The first embedded surface includes explorer, editor, results and authorized row editing. Host settings, connection management, plugin lifecycle and desktop onboarding do not belong in this surface. Shared global CSS, keyboard handlers, Monaco workers and provider dependencies need verification in Studio's actual Next.js environment.

UI assets and the Rust binary must be built and pinned to the same reviewed upstream revision. Do not depend on a moving feature branch or advertise a published upstream web release while #676 remains open. Any upstream implementation changes need their own isolated Tabularis checkout and review; this design does not alter that PR.

## Ownership alongside the other worktrees

This work owns database bindings/grants, session authorization, the gateway adapter and Studio database UI. Prefer separate `database-*` contract/modules and explicit migrations rather than expanding unrelated Mail or Git deployment files.

Git deployment work can consume a non-secret binding reference when configuring an application and deploy pinned gateway/runtime artifacts. It must not turn a Git rollback into a database rollback, grant browser permissions, or put connection secrets in Git. Container lifecycle integration needs an explicit interface agreed against that worktree's final contracts.

RelayKit has no functional dependency on database browsing. Reuse established identity, shell and translation patterns; make no Mail behavior changes.

## Proposed first delivery and acceptance

Start with explicitly registered PostgreSQL databases, customer/operator access, the three access profiles where supported by actual database roles, schema browsing, SQL execution/cancellation, pagination and permitted row editing. PostgreSQL-first is a proposed delivery boundary, not a claim that other Tabularis engines are unsupported.

Saved-query sharing, AI, plugin installation, database provisioning, backups/restores, general network tunnels and SQLite volume attachment are later decisions. In particular, attaching a live application's SQLite file introduces file ownership, concurrency and volume-lifecycle requirements beyond a TCP database connection.

Acceptance checks must include:

1. Two tenants, two users with different grants in one tenant, and an operator cannot cross their authorized bindings via UI, RPC, events, cancellation or transfers.
2. A read-only database role rejects writes, DDL and unsafe privilege escalation through direct SQL, including batches and callable routines.
3. Revoked membership/grants, expired sessions, forged proxy headers and existing upstream cookies cannot preserve access.
4. Connection metadata, query history, saved queries, temporary files, pools and caches remain within the runtime's scope.
5. Queries time out/cancel predictably; reconnect does not execute a write twice; oversized results and disconnects remain bounded.
6. Native Studio flows work with real disposable PostgreSQL data, in English/German and light/dark mode, without a second login, broken routing or lost unsaved SQL.
7. Shared-database fixtures prove denied cross-schema access. Production/preview labels correspond to real registered targets.
8. Matched frontend/backend packaging and headless secret access work on the intended runtime image. Record measured startup time and memory before choosing capacity.

No production database query, credential change, upstream PR modification, runtime deployment or new infrastructure purchase has been performed for this design.

## Source references

- [Tabularis Web PR](https://github.com/TabularisDB/tabularis/pull/676)
- [Application API and request context](https://github.com/TabularisDB/tabularis/blob/aeb589e6a3fdd091184a3693bb2f8629bff46f96/src-tauri/src/application/api.rs)
- [Connection services](https://github.com/TabularisDB/tabularis/blob/aeb589e6a3fdd091184a3693bb2f8629bff46f96/src-tauri/src/application/connections.rs)
- [Web authentication](https://github.com/TabularisDB/tabularis/blob/aeb589e6a3fdd091184a3693bb2f8629bff46f96/src-tauri/src/transport/web/auth.rs)
- [Web request security gate](https://github.com/TabularisDB/tabularis/blob/aeb589e6a3fdd091184a3693bb2f8629bff46f96/src-tauri/src/transport/web/server.rs)
- [Remote deployment security](https://github.com/TabularisDB/tabularis/blob/aeb589e6a3fdd091184a3693bb2f8629bff46f96/web-ui-project/docs/WEB_REMOTE_SECURITY.md)
- [Web UI package](https://github.com/TabularisDB/tabularis/blob/aeb589e6a3fdd091184a3693bb2f8629bff46f96/packages/web-ui/package.json)
- Local: `docs/architecture/customer-tenants.md`, `docs/architecture/studio-product-ui.md`, `docs/architecture/spitzli-control-plane.md`, `apps/auth/src/lib/hosting/authorization.ts`.
