# Tenant database browser

Studio embeds the Tabularis explorer, SQL editor and data grid. The API authorizes Managed PostgreSQL tenant administrators and owns project bindings with explicit per-user grants for registered databases. This gateway connects each authorized session to an isolated Tabularis runtime. The integration supports PostgreSQL and project-owned SQLite volumes. Operators administer bindings and grants with MFA, but also need an explicit grant to browse a database. Customer preview cannot open database sessions.

## Registered project databases

1. Apply the existing Auth, hosting and Studio migrations first, then run `node --import tsx legacy/control-plane/scripts/legacy/migrate-databases.ts` with the Auth migration database credentials. The new tables live in `webdock_auth`; use the existing schema owner/runtime privilege conventions.
2. Configure Auth with `WEBDOCK_DATABASE_GATEWAY_ORIGIN` and a random `WEBDOCK_DATABASE_GATEWAY_SECRET` of at least 32 characters. The gateway uses the same secret. Auth also needs its existing `BETTER_AUTH_SECRET`, Studio OAuth consumer configuration and hosting region configuration. Production authorization retains the existing verified-region gate (`WEBDOCK_HOSTING_EU_VERIFIED=true` for the verified non-Vercel runtime, or the existing `vercel-fra1` policy).
3. Configure and start the native Rust gateway using `.env.example` and `cargo run -p webdock-database-gateway` from the repository root. Put TLS in front of it, including WebSocket upgrades. Use a gateway hostname on the same site as Studio, for example `database.webdock.dev` and `studio.webdock.dev`. The session cookie is HttpOnly, SameSite=Strict, Secure over HTTPS, and scoped to its workspace path. Cross-site embedding is not supported.
4. Provision a separate Tabularis process/container **and storage directory for every database binding, user and permission profile**. Restrict network access to the gateway and the assigned database. Configure only the assigned saved connection; do not mount unrelated credentials, plugins or host files. Runtime origins must be explicitly included in the gateway allowlist.
5. Use a dedicated PostgreSQL login whose actual privileges match the grant. A read role has only CONNECT, schema USAGE and SELECT on approved objects. A write role adds only intended INSERT/UPDATE/DELETE (and required sequence permissions). Schema roles may manage only their own project schema. Remove inherited/public privileges that would expose other tenants. The gateway does not infer SQL permissions from text; PostgreSQL enforces them, including arbitrary SQL typed into the console. SQLite runtimes instead receive only the approved project volume; the production Lunares grants allow full schema/data access to that volume. A read-only SQLite grant requires a separately verified read-only connection and mount.
6. Build Tabularis from the source recorded in `vendor/tabularis-web-ui.json`; apply `vendor/tabularis-web-ui.patch` to the recorded base commit when reconstructing it. Build the web assets with `pnpm build` and the Rust server with `TABULARIS_BUILD_COMMIT=<sourceCommit> cargo build --release --manifest-path src-tauri/Cargo.toml --bin tabularis --locked`. Configure `TABULARIS_EXPECTED_COMMIT` to that same source commit. Debug builds without a stamped commit intentionally fail the production gateway check.
7. Start Tabularis in proxy authentication mode (`tabularis web --help` documents the exact options). Configure its public origin, allowed gateway origin, data directory and a unique proxy secret. Terminate HTTPS on its private origin. Never expose the proxy-authenticated backend directly to customers. Store its saved connection and database credentials only in that runtime's protected storage.
8. In Studio → tenant → Databases, register the project database and assign access to the intended customer or operator using its isolated runtime origin, connection ID and proxy secret. The operator verifies isolation and database-role privileges before activation. Secrets are encrypted by Auth and are never returned in directory/launch DTOs.

Registered project databases continue to use explicitly provisioned runtimes. Managed PostgreSQL uses the automatic process lifecycle below. Neither path provisions database roles, deploys infrastructure or executes production migrations. Hand the runtime deployment requirements to the GitOps deployment workstream. For the repository's existing Vercel applications, install the Vercel CLI (`npm i -g vercel`) when managing their environments or deployments; it is not required to run this standalone gateway.

## Automatic Managed PostgreSQL viewer

Every ready Managed PostgreSQL database offers its viewer to the tenant's current
administrator. No project binding, per-user grant, runtime origin or connection
secret must be entered for each database. Operators using customer preview cannot
launch it. The existing project database workflow above remains separate.

Deploy the API migration `legacy/control-plane/drizzle/0022_managed_database_viewer.sql` through
the normal migration runner, then install the matching platform RLS policies with
`legacy/control-plane/scripts/install-platform-rls.ts`. Deploy the API routes before activating
the gateway feature. `WEBDOCK_AUTH_ORIGIN` must point to the **Auth hostname** serving the API-owned
legacy route `/api/databases/gateway` (normally `https://auth.webdock.dev`).
The API implementation preserves this host boundary; using `api.webdock.dev`
for this route returns 404. When Auth shares the gateway's cluster, public HTTPS
is translated to Traefik's private pod port 8443 before network-policy evaluation.
The renderer permits only the gateway to reach kube-system's Traefik pods on that
port, in addition to public HTTPS; it retains normal hostname/TLS verification. API and gateway use
the same gateway secret. Configure `https://console.webdock.dev` and any legacy
Studio origin in `WEBDOCK_DATABASE_BROWSER_ORIGINS`.

Build the gateway image with the pinned Tabularis runtime image already available
locally. Its Dockerfile copies Node 24 into `webdock-tabularis:production-fdd6c0a6`,
retaining the runtime's Linux libraries and web assets. Override `TABULARIS_IMAGE`
only with the matching verified source commit/image digest. The runtime process
still runs as UID/GID 65532 with no extra capabilities; it needs a writable private
`/tmp` volume. Set `WEBDOCK_DATABASE_MANAGED_RUNTIMES=true` to enable it, and set
`TABULARIS_EXPECTED_COMMIT` to `vendor/tabularis-web-ui.json`'s full source commit.
A missing executable, incompatible build or exhausted capacity fails closed.

The default limit is eight simultaneous processes; configure
`WEBDOCK_DATABASE_RUNTIME_CAPACITY` (1–64) to fit the deployment's tested memory
budget. Each gateway session gets one process, one private mode-0700 temporary
directory and exactly one saved PostgreSQL connection. Credentials and its CA are
mode-0600 files; PostgreSQL uses `verify-full` TLS. The loopback listener has its own
random proxy secret, which the browser never receives. Runtime stdout/stderr and
Rust logging are disabled; only the gateway's existing metadata audit is logged.
Gateway secrets and other service environment variables are not inherited.

A session is permanently bound to its authorized user, database and revision.
Credential rotation, database migration/state changes, loss of membership or native
login revocation end access; an old session never switches to a new backend.
Expiry, logout, authorization failure and gateway shutdown close requests/events,
terminate the process and remove its directory. Idle managed sessions are checked
every three seconds. **Saved queries, notebooks, history and editor preferences in
managed runtimes are temporary and disappear when that session ends.** Export or
copy anything needed beyond the session. Registered runtimes retain their existing
persistent storage.

For Kubernetes, pass `--managed-runtimes-config /private/managed-viewer.json` to the
existing renderer with an explicit cluster network inventory:

```json
{
  "authOrigin": "https://auth.webdock.dev",
  "capacity": 4,
  "clusters": [
    {"addressIPv4": "79.143.177.157", "port": 32000, "endPort": 32199}
  ]
}
```

Use the verified public address of each authorized database cluster. The renderer
allows only these `/32` addresses and the managed backend NodePort allocation
range 32000–32199, enabling future databases without individual network changes.
It retains default-deny networking and the registered runtime allowlist, adds a
memory-backed temporary volume and increases the gateway resource budget/quota.
The API sends each database's current CA over the authenticated control channel;
no global database CA mount or certificate-verification bypass is needed. Existing
runtime TLS trust remains unchanged. Without the optional config, managed mode is
disabled and no database egress is added.

Before enabling production, review the rendered gateway/NetworkPolicy/quota changes,
verify cluster firewalls/HBA permit the gateway source, confirm the database owner
role cannot access another tenant, and test the exact pinned image under nonroot,
read-only-root-filesystem constraints. The processes share a container and UID;
private directories isolate application state, not an OS security boundary against
a compromised runtime. Size CPU/memory and node PID limits for the chosen capacity.
For a hard per-process OS boundary, provision separate restricted containers.
No production migration, deployment or firewall change is performed automatically.

## Lifecycle and limits

Launch codes expire after 60 seconds and are consumed atomically once. Browser sessions expire after 15 minutes and remain bound to the native Auth session and grant revision. Every RPC and WebSocket event checks current authorization. The gateway rechecks before returning query data and every three seconds during long requests/events; cancellation is best effort and is never a guarantee that a submitted database write was rolled back.

Revoking access, disabling a binding, removing membership or ending the native session denies existing access. Replacing a grant invalidates its sessions. A runtime origin is permanently reserved to its original binding/user/profile: use a fresh isolated runtime when changing privileges or ownership. Decommission old processes/storage separately after revocation. Do not reuse their origin or data directory.

The gateway limits active RPCs (64 globally, 16 per browser session, 4 query operations per session), request/response sizes (1 MiB/8 MiB), upstream timeouts and event connections. It logs command names, actor/scope, status and duration, not SQL, result rows or credentials. Expired database launch/session records should be periodically deleted according to the installation's retention policy; audit retention and automated cleanup scheduling remain deployment responsibilities. Runtime memory/CPU capacity has not been load-tested.

Host configuration, arbitrary connection creation, extensions, tunnels, uploads/imports and detached windows are unavailable. The embedded UI may retain some upstream controls that return a permission error. Clipboard row editing is separate from bulk file imports. SQL tabs belong to the upstream browser session; opening a fresh Webdock session starts a new tab workspace. Save important SQL as a named query in the isolated runtime before leaving. Theme, locale, focus and fullscreen changes retain the currently mounted editor and its drafts. Partial grid writes acknowledge successful operations and require review/refresh after an uncertain failure; they are not automatically retried.

## Validation

From the repository root with Node 24:

```sh
npm test -w @webdock/database-gateway
npm run check -w @webdock/database-gateway
node --import tsx --test packages/database-contracts/src/index.test.ts
node --env-file=.env.database-test.local --import tsx --test legacy/control-plane/tests/ported-auth/databases/integration.test.ts
```

The Auth integration test intentionally resets schemas only in the dedicated fixture database `postgres://…@127.0.0.1:55441/webdock_admin_test`; it refuses other destinations. Do not point this test at a shared development or production database. Real-runtime tests are opt-in with `WEBDOCK_DATABASE_RUNTIME_TEST=true` and use local reader/writer runtimes on ports 3138/3158, with their protected HTTPS proxies on 3139/3159. See the test sources and the verification record for fixture requirements.

`scripts/database-browser-fixture.ts` supplies deterministic test identities and sealed Studio cookies for the isolated browser check. It does not replace or validate the complete production OIDC login ceremony. Its local secrets are synthetic fixture values; it refuses production mode and any database outside the dedicated test endpoint.

## Container deployment

Build the gateway with `npm run build -w @webdock/database-gateway`, then build its Dockerfile from `apps/database-gateway` after loading/building the pinned Tabularis image. The Tabularis image uses `deploy/Tabularis.Dockerfile` against an archived pinned source tree containing built `packages/web-ui/dist` assets. Stamp the full `SOURCE_COMMIT`. Import the exact images into k3s; pods intentionally use `imagePullPolicy: Never`. Keep a copy of the image digests in the rollout record.

`deploy/render.py` renders restricted pods, per-runtime secrets/state claims, internal TLS, default-deny network policies and the gateway ingress from a private operator inventory. Its JSON output contains secrets and must remain outside Git with mode 0600. CA keys and runtime keys are private; leaf certificates expire after 365 days and must be renewed before expiry. Use a server-side dry run before applying. The SQLite inventory references an existing approved claim; Lunares uses a retained local-PV alias of the same quota-backed data directory as its bot, on the same node, so SQLite locking/WAL are shared. This alias does not allocate another copy of the database. Revoke database grants and stop browser runtimes before decommissioning the bot volume; retain/delete handling must respect both consumers. Existing customer workload quotas are unchanged.

The cluster's admission policy also requires the `security.webdock.dev/ready=true` namespace label. Set it only after applying and inspecting the rendered ResourceQuota, network policies and Restricted Pod Security labels. Build `deploy/TLS.Dockerfile` for the TLS sidecar: it removes Caddy's privileged-port file capability, allowing port 8443 with all capabilities dropped. No admission policy exemption is required.

After authenticated runtime checks pass, the explicitly invoked offline Auth script `legacy/control-plane/scripts/legacy/provision-database-browser.ts <private-inventory.json>` can register the verified schema-access grants. It requires `WEBDOCK_DATABASE_OPERATOR_ID`, an active MFA-enabled operator, and validates each subject's existing customer membership. Use the Auth production environment and keep inventory secrets out of logs/Git. Running it again replaces those grants and revokes their previous sessions through the revision increment.

## Moving one PostgreSQL browser runtime

The optional private inventory fields below enable verified database TLS and replace
only that runtime's public-5432 egress rule. Keep its existing connection ID,
binding, user, profile, proxy secret and state claim. Supply the target database
credentials privately; the renderer never generates or changes them.

For the existing browser cluster connecting to a separate database cluster:

```json
{
  "databaseTLSCA": "/private/managed-postgres-ca.crt",
  "databaseTarget": {
    "host": "db-103217546630533120.pg.webdock.dev",
    "port": 32000,
    "addressIPv4": "79.143.177.157"
  }
}
```

Use the verified current endpoint/IP from deployment inventory. Egress allows only
that public IPv4 `/32` and port, plus the existing DNS policy. The hostname remains
the TLS verification identity; the renderer does not resolve it or change DNS.
For a runtime in the same cluster, use this alternative target:

```json
{
  "databaseTarget": {
    "host": "postgres-rw.wd-db-103217546630533120.svc.cluster.local",
    "namespace": "wd-db-103217546630533120",
    "podLabels": { "cnpg.io/cluster": "postgres" }
  }
}
```

Both forms require `databaseTLSCA`, a public PEM certificate file (never a private
key), and exactly one saved PostgreSQL connection. Its existing `params` must have
matching `host` and `port` (5432 for internal targets), `ssl_mode: "verify-full"`
and `ssl_ca: "/database-trust/ca.crt"`. Other connection values remain unchanged.
The CA gets a per-runtime ConfigMap and read-only mount. Gateway-to-runtime TLS
trust is unchanged. Incomplete, wildcard or inconsistent targets fail validation.

Internal targets additionally produce `<output>.database-ingress.json`, containing
only a separate database-side NetworkPolicy selecting the supplied database pods
and allowing TCP 5432 from this exact runtime pod in `webdock-databases`. Apply it
to the **database cluster** after review. External targets produce no ingress
manifest. Always use a fresh output path to avoid confusing an older ingress file
with the current render. Existing inventory rows without these fields retain
legacy manifests, including customer runtime connections and network policies.

For a targeted rollout, render to a fresh private mode-0600 path, then select only
the affected runtime's Secret, Deployment, NetworkPolicy and new CA ConfigMap into
a separate private manifest. Do not apply the entire rendered inventory: it also
contains shared gateway resources, quotas and unrelated customer workloads. Run a
server-side dry run and review the selected object names without printing Secret
data. Confirm managed HBA and role lifecycle permit the dedicated browser login;
the renderer does not grant database access. Revoke the affected browser grant,
quiesce its runtime during final copy, apply the selected resources and restart
only that deployment: the init container copies saved connections on pod startup.
Verify authenticated SQL, schema browsing/editing, tenant denial and revocation
before re-enabling the grant with a new revision. Keep the old private config for
rollback; after target writes, rollback requires data reconciliation.

Renderer validation (disposable certificates and synthetic credentials only):

```sh
python3 -m unittest discover -s apps/database-gateway/tests -p 'test_render.py'
```

The gateway implements embedded `test_connection` as a fixed `SELECT 1` through
the runtime's saved connection ID, returning the upstream-compatible success string.
This retains the saved TLS CA and verification settings without sending host-file
parameters through Tabularis's restricted remote connection-test endpoint. Supplied
client parameters are ignored; the probe uses normal query limits, deadlines,
revocation checks and cancellation.

Managed process verification uses an optional local Tabularis binary and creates a
fresh disposable PostgreSQL 18 Docker container with synthetic credentials and a
self-signed test CA. It checks schema/data edits, verified TLS (including rejection
of an unrelated CA), connection scoping, revocation and temporary-state cleanup:

```sh
WEBDOCK_MANAGED_RUNTIME_TEST_BINARY=/path/to/tabularis \
  node --import tsx --test apps/database-gateway/tests/managed-real-runtime.test.ts
```

The regular suite tests process concurrency/capacity, startup failures, expiry,
revision changes, secret redaction, and opt-in behavior without Docker.

To exercise the actual packaged runtime and its stamped commit, build the gateway
image and replace the binary variable above with
`WEBDOCK_MANAGED_RUNTIME_TEST_IMAGE=webdock-database-gateway:managed-viewer-test`.
This variant also verifies UID 65532 with a read-only root filesystem, all Linux
capabilities dropped, no privilege escalation, a 256 PID limit and private tmpfs.
Its test containers use host networking solely to reach the disposable loopback
PostgreSQL fixture; deployment networking remains governed by NetworkPolicy.

## Rust gateway migration

The production gateway executable now lives in `crates/database-gateway`; see its README for boundaries and tests. Build the native image from the repository root:

```sh
docker build -f deploy/docker/database-gateway.Dockerfile -t webdock-database-gateway:rust-local .
```

To run the retained PostgreSQL/TLS acceptance against the native host binary, set `WEBDOCK_MANAGED_RUNTIME_TEST_GATEWAY_BINARY` to `target/debug/webdock-database-gateway`, alongside the existing `WEBDOCK_MANAGED_RUNTIME_TEST_BINARY` and `WEBDOCK_MANAGED_RUNTIME_TEST_WEB_ROOT` for the pinned browser runtime. The image variant uses `WEBDOCK_MANAGED_RUNTIME_TEST_IMAGE=webdock-database-gateway:rust-local`. Both create only disposable local databases. The TypeScript server sources remain a temporary contract-test oracle while all current cases move to native tests; they are not an active production entrypoint.
