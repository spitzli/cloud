# Native API and worker cutover

The implementation is in `apps/api`; Studio and Auth are HTTP-only Next.js frontends. This is a rollout runbook, not evidence that production was switched. See [implementation verification](../verification/2026-10-14-api-rewrite.md) and [Polar operations](polar.md).

## Reproduce locally

Use Bun 1.4.3, the version pinned in `package.json`. From the repository root:

```sh
bun install --frozen-lockfile
bun run --filter @webdock/api test:setup
bun run --filter @webdock/api test:setup:rls
bun test -w @webdock/api
bun run --filter @webdock/api test:rls
bun test -w @webdock/admin
bun test -w @webdock/auth
bun test -w @webdock/api-client
bun run --filter @webdock/api build
bun run --filter @webdock/admin build
bun run --filter @webdock/auth build
bun --no-env-file apps/api/scripts/rehearse-cache.ts
```

The fixture scripts create a labeled, disposable local PostgreSQL18 Docker container and private ignored environment files. Role tests use signed contexts and constrained runtime principals. The harness blocks non-loopback fetches; it never inherits deployment/provider credentials. Four privileged fixture regressions are opt-in in the basic run and run explicitly in `test:rls`. Teardown only removes the owned fixture and checksum-matching files; modified files are preserved. The real Polar sandbox rehearsal uses a separate database and is not part of `bun test`.

## Required production evidence

Before applying anything, record the actual target host/cluster, EU region, CPU/RAM/disk headroom, private networking, verified TLS, backup ID and successful restore. No target capacity or production restore was certified by local tests. If the selected API host is Kubernetes, translate the reviewed process/network/secret profiles into the existing cluster deployment; do not introduce a second production Docker control plane. Managed incoming Mail remains isolated Stalwart CE on the approved Contabo/k3s infrastructure. Outgoing-only customers do not receive a Stalwart instance.

Compare restored production schema, defaults, quoted column names, indexes, functions, grants and FORCE RLS against the source-derived local baseline. API migrations are additive and checksummed; they are not a replacement for that comparison. Runtime startup must never run DDL. Preserve the old migration journals and all existing Snowflake IDs.

Capture exact old API/UI image or immutable Vercel deployment IDs before removal of any old writer. Source at `95fc3b2ed4317f4d19bdba2c05530ae217924574` is the pre-rewrite reference, not a claim that a particular production artifact is available.

## Database and CLI authority

API receives separate `webdock_auth_runtime`, `webdock_auth_bootstrap`, `webdock_worker_runtime` and `webdock_registry_runtime` credentials. Worker receives only `DATABASE_WORKER_URL` and the keys required by selected handlers. Runtime principals have no owner membership, DDL, superuser or bypass-RLS authority. Readiness verifies actual principal identity, ownership/membership, schema isolation and the signed-context key.

`DATABASE_POOL_MAX` and `REGISTRY_POOL_MAX` are explicit, bounded1–4. At1 the API has at most four pools; a worker has one. Measure the full PgBouncer budget including old processes, gateways and rolling overlap before increasing replicas or maxima. A historical database budget of ten is not a current capacity guarantee. Connection application names distinguish API, registry and worker for drain inspection.

Run offline migrations with a dedicated direct migrator URL, verified CA and exact `host:port/database` argument:

```sh
# DATABASE_URL_UNPOOLED / DATABASE_CA_CERT come from the private migrator environment.
bun run --filter @webdock/api migrate -- exact-host:5432/exact-database
# Explicit private target file; never the API runtime environment:
bun --no-env-file apps/api/scripts/install-platform-rls.ts /private/rls-target.env
```

The installer allows only its reviewed target or the explicit local fixture, classifies every table, and refuses unknown unowned schema. It acquires the complete table lock set with NOWAIT before changing grants; drain old writers first, and treat a busy-lock refusal as an incomplete cutover. Rehearse with the dedicated non-superuser migrator, not only the restore owner. Provision a missing registry login with `scripts/provision-registry.ts PRIVATE_ENV EXACT_TARGET NEW_PRIVATE_OUTPUT`; it refuses existing-role password rotation. `scripts/admin/map-operator.ts` maps an existing verified central operator with MFA; it creates no password or independent role authority. Inspect each CLI's argument validation before running it. Old duplicate `scripts/legacy/migrate*.ts`, owner/identity bootstrap, obsolete frontend-env fixtures and hardcoded showcase SSO registration commands fail closed. Existing identities/clients must be reused; new business provisioning goes through the current operator API instead of synthesizing a session from an email address.

Moved administrative seed/showcase/register scripts require `WEBDOCK_OPERATOR_SUBJECT` and `WEBDOCK_OPERATOR_SESSION_ID`, check the current session, and use constrained API services. Mutating scripts require `--apply`; no email address is an authority substitute. Historical Studio DDL lives in `apps/api/legacy-migrations/admin` for comparison, not automatic execution.

## Process, secrets and ingress

`infra/api/compose.yaml` is the reviewed single-host candidate. It defines one image for API/worker, read-only non-root runtime, dropped capabilities, bounded memory/CPU, graceful shutdown and a private disposable cache. It publishes no API/cache port. Validate with:

```sh
docker compose --env-file infra/api/env.example -f infra/api/compose.yaml config --no-env-resolution --no-interpolate --quiet
```

Supply an immutable image digest, the existing controlled ingress network and separate0600 API/worker/cache environment files. `infra/api/env.example` is a name-only guide. Keep Auth issuer `https://auth.webdock.dev/api/auth`, passkey RP-ID `auth.webdock.dev`, host-only cookies, existing Better Auth/SSO encryption keys and Mail encryption key unchanged. No privileged migrator URL belongs in an image or runtime secret set. The frontend receives public origins only.

The Traefik file-provider template routes Auth protocol/discovery and Studio SSO/provider/MCP aliases to Bun, UI pages to reviewed immutable UI origins, and the API host exclusively to Bun. Replace the `.invalid` UI upstreams; never use public canonical hosts as upstreams. Direct TLS ingress must discard untrusted forwarded headers, set the canonical host/protocol, and supply the shared ingress secret. Bun also verifies the actual peer CIDR and accepts only a single validated client IP. A chained proxy requires separate normalization testing; do not enable universal header trust.

Reference: [Traefik forwarded-header trust](https://doc.traefik.io/traefik/reference/install-configuration/entrypoints/). Test real TLS before DNS cutover: multiple cookies, native login/MFA/passkey, PKCE/OIDC issuer and redirects, Server Actions origin checks, provider callbacks, raw signed webhook bytes, MCP audience, streaming and cancellation. If immutable Vercel origins cannot preserve these semantics, use the existing UI containers; do not rewrite the UI into a SPA. Vercel CLI is not installed in this environment; installing it with `bun add -g vercel` is strongly recommended for frontend deployment work and enables origin/deployment/env inspection where applicable.

Disable or redact query-string and sensitive-header logging on auth, SSO, billing-return, offer and webhook routes. OAuth codes, provider customer-session tokens, checkout/portal URLs, cookies, raw event bodies and encryption keys must not enter logs. New checkout success URLs target the API-owned `/api/billing/return`; it drops provider tokens and redirects to the correct environment. Existing immutable checkout URLs remain readable. UI form pages use `same-origin` referrer policy, retaining normal Origin/CSRF checks while suppressing cross-origin referrers.

## Cache and failure behavior

Dragonfly2.0.2 digest is pinned in Compose. The real local rehearsal passed with non-root read-only filesystem, authentication,256MiB data/512MiB container limits and one proactor thread. These limits still need target load testing. This exact binary uses case-sensitive `DFLY_requirepass`; its advertised `DFLY_PASSWORD` flag was rejected. Mandatory password validation happens before startup. No port6379 is published, no persistence is required, and version-check telemetry is disabled. [Pinned upstream flag parser](https://github.com/dragonflydb/dragonfly/blob/v2.0.2/src/server/dfly_main.cc) and [release](https://github.com/dragonflydb/dragonfly/releases/tag/v2.0.2).

Only public presentation branding currently uses the optional cache. TTL60s,100ms budget, bounded entries and a ten-second failure circuit fall back to the database. Roles, sessions, admission/quota decisions, billing state, jobs, leases and deduplication remain PostgreSQL authority. Cache degradation does not fail API readiness.

## Transfer and rollback

For each ownership unit, freeze the old writer, drain requests/pools, record queue cursor and leases, apply additive schema/policies, start and verify the new owner, then switch the exact ingress routes. Do not run old cron and new worker ownership concurrently. Mail/storage/billing worker domains are explicit; billing environments are explicit too. Storage/Mail production scheduler transfer was not performed here.

Before new writes, rollback may restore the recorded previous routes/artifacts. After new writes, retain all new records, financial history, audit, sessions, encryption keys and lease/checkpoint state: use a compatibility rollback or forward fix. Never restore an old database over accepted billing agreements. Keep webhook/termination/reconciliation processing running while sales are disabled. Lost external-write responses are reconciled by readback, never blindly repeated.

Rehearse API stop, worker SIGKILL/restart and expired-lease recovery, DB failure, cache failure and restore independently. Observe route latency/error rate, pool waits, oldest pending job, webhook lag, conflict counts and provider read failures. Configure external alert delivery separately; this change sends no operational notifications.

The new Auth UI artifact intentionally has no Vercel cron. Its deployment belongs after worker ownership transfer; the currently deployed old cron was not changed by editing this repository. Archive its old schedule/deployment in the cutover record before deploying the UI artifact.

## Deployed Kubernetes profile

See [production evidence](../verification/2026-10-14-api-production.md). Apply `infra/api/kubernetes.yaml` (namespace, API, quota and policies) before marking the namespace `security.webdock.dev/ready=true`; the existing admission policy requires Restricted Pod Security, quotas, policies and complete probes/resource bounds. Supply `api-runtime`, `worker-runtime`, `cache-runtime` and `ui-proxy-config` as private Kubernetes Secrets. Then apply `cache.yaml`, `worker.yaml`, and `ui-proxy.yaml` from the same directory. The worker Secret owns its explicit domain list.

`ingress.yaml.template` contains only the `${WEBDOCK_INGRESS_SECRET}` placeholder. `ui-proxy.conf.example` uses `${AUTH_UI_HOST}`, `${STUDIO_UI_HOST}`, `${AUTH_UI_BYPASS_SECRET}` and `${STUDIO_UI_BYPASS_SECRET}`; render only those named variables, preserving NGINX's own `$vercel_origin` variable. UI hosts must be immutable deployment hostnames. Store the resulting `nginx.conf` in `ui-proxy-config`, not Git. A proxy Secret change requires a rolling proxy restart because its configuration uses a subPath mount. Keep the UI bypass credentials out of response headers and reject bypass-cookie query parameters.

The API trusts the actual node pod CIDR plus the shared ingress credential; its NetworkPolicy permits incoming requests only from the Traefik pods. API upgrades use Recreate to bound database connection overlap. The UI-only proxy can roll with one extra pod inside the namespace quota. Do not enable ExternalName Services or relax cluster admission policies for this routing.
