# Clerk Console release and rollback

Status: PR preparation only. **Do not merge or deploy until the owner returns and
confirms the production Clerk instance is configured.** This is a manual release;
no auto-merge, automatic production migration or new deployment pipeline is enabled.
The code review/test evidence is in [the checkpoint](../verification/2026-10-15-clerk-console.md).

## Release boundary

- Console: `apps/admin`, canonical `https://console.webdock.dev`, existing Vercel project.
- API and worker: `apps/api`, existing Kubernetes/Contabo workloads. Reuse their
  current ingress, immutable image deployment and restricted database roles.
- Better Auth continues to serve CMS/OIDC/MCP and legacy clients. Keep its keys,
  registered origins, cookies and credentials. Clerk is the Console browser identity.
- Customer teams use Clerk's native `org:admin` / `org:member` rules. Webdock still
  checks local user bans, active customer, resource relationships and explicit grants.
  Direct Clerk calls are not subject to the extra local invitation/last-active-admin
  rules that the owner explicitly retired.
- Incoming Mail remains CE per explicitly enabled customer on the existing cluster.
  This release does not provision Mail or change relay, storage or encryption keys.

## Before the owner's production switch

1. Keep this PR open against `main`; no merge/deploy now. Record its reviewed SHA
   and fresh test results. Run builds with Bun 1.4.3
   (`bun run …` uses the pinned local binary, unlike `~/.bun/bin/bun`).
2. Inspect the existing Vercel Git integration before merging: a main push may
   start a production deployment. Prepare the API/worker first, and control Console
   promotion explicitly so the merge cannot accidentally activate Clerk early.
3. Keep the current immutable API/worker image references and previous Console
   deployment ID for recovery. Inventory configuration by variable names only.
   Preserve all existing signing/encryption/provider keys and retained Mail volumes.
4. Prepare a private reviewed inventory of the real four local identities (one
   operator, three customers) and two organizations. Do not copy the temporary test
   IDs or emails. No real inventory or production migration has been executed yet.
5. Rehearse the additive schema migrations and policy installer on an isolated
   restored database, using the same non-superuser offline role as the release.
   Compare existing row counts, migration checksums, FORCE RLS and legacy access.
   Do not connect a restored dataset to a production Clerk key for an apply run.

## When the owner returns

### Verify the production target

Use the Clerk CLI to independently verify that the selected **production** instance
belongs to app `app_3KjGKUGb7dYUMgkjKC0eCWghnOB`. Record the app/instance/issuer tuple
in the private manifest and release evidence. Never reuse the development instance
`ins_3KjGKQnAATh2cA4ddJBR2cg3NXu` or its mappings for production.

Verify restricted signup, optional organization selection, disabled user-created
organizations and self-deletion policy, enabled organization slugs, and standard
roles/entitlements. Development settings are not evidence of production settings.
Check the actual production cookie domain and exact Console origin. Verify the
production publishable key resolves to the intended issuer. No wildcard origins.

Clerk's Backend instance endpoint does not establish application ownership; the
separate CLI application/instance verification is therefore mandatory. The importer
checks the actual instance ID/environment and key/issuer/environment consistency,
and rejects a development provider with a nonlocal database.

### Configuration and build

Use [Console examples](../../apps/admin/.env.example) and
[API/worker examples](../../infra/api/env.example). Store private values directly in
the existing secret stores, never in Git, PR text or terminal output.

| Runtime | Required Clerk configuration |
| --- | --- |
| Console build/runtime | `NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY`, server-only `CLERK_SECRET_KEY`, `WEBDOCK_CONSOLE_SERVICE_KEY`, `WEBDOCK_CONSOLE_AUTH_PROVIDER=clerk`, existing API/public origin |
| API | `CLERK_ISSUER`, `CLERK_PUBLISHABLE_KEY`, `CLERK_SECRET_KEY`, `WEBDOCK_CONSOLE_SERVICE_KEY`, exact `WEBDOCK_CONSOLE_ORIGIN`, activation flag |
| Worker handling Clerk jobs | Same Clerk verifier settings and Console origin/service key; retain restricted worker DB credentials |
| Offline importer | `CLERK_ISSUER`, `CLERK_PUBLISHABLE_KEY`, `CLERK_SECRET_KEY`, `DATABASE_URL_UNPOOLED`, verified `DATABASE_CA_CERT` |

`NEXT_PUBLIC_` keys are embedded during the Console build. Build a fresh production
artifact with production values; do not promote the local development-key artifact.
The delegation key is random, at least 32 bytes, shared only by authorized runtimes.

The Vercel CLI is not installed in the current environment. Install it with
`bun add -g vercel` before the Vercel release steps to use `vercel env pull`,
`vercel deploy` and `vercel logs`. Confirm project/team/environment before commands;
never dump pulled secret files. This does not replace the existing Kubernetes rollout.

### Schema and identity migration

Take a private recoverable database backup. Run the existing guarded offline
migration tooling, with a dedicated non-superuser migrator and verified TLS. Pause
conflicting workers/writers as required by the existing policy install procedure;
never change runtime users into database owners.

From `apps/api`, with a private environment file (mode 0600):

```sh
# Exact target is reviewed host:port/database, not a connection string.
bun --env-file=/private/migration.env scripts/migrate.ts REVIEWED_HOST:PORT/DATABASE
bun --no-env-file scripts/install-platform-rls.ts /private/rls-installer.env
```

The installer file uses `DATABASE_URL`, `MIGRATION_TARGET_HOST`,
`MIGRATION_TARGET_DATABASE`, `DATABASE_CONTEXT_SECRET` and `DATABASE_CA_CERT`.
Retain the existing context secret. Inspect checksums and policy/readiness results
before starting new writers. Do not rename or rewrite historical migrations.

Before linking production identities, deploy the migration-aware API/worker code
with Console activation still disabled and the correct production verifier settings.
Stop older workers/writers during the identity-linking window. Otherwise the old
binary could continue trusting stale legacy memberships after the mapping exists.
Only then run the importer; the later deployment step activates the Console mode.

The private importer manifest contains `environment: "production"`, the exact
`applicationID`, `instanceID`, `issuer`, four `accounts` (`localID`, `kind`, optional
reviewed `clerkUserID`, optional explicitly reviewed `firstName`/`lastName`) and two `teams` (`organizationID`, `customerID`, optional
reviewed `clerkOrganizationID`, `members` with `localID`/`role`). Its Zod schema in
`apps/api/scripts/migrate-clerk-users.ts` is authoritative. Operator is never a team
member. Resolve duplicate-email collisions manually; do not merge by email.

```sh
bun --env-file=/private/clerk-import.env scripts/migrate-clerk-users.ts \
  --manifest /private/clerk-manifest.json \
  --app app_3KjGKUGb7dYUMgkjKC0eCWghnOB --instance REVIEWED_PRODUCTION_INSTANCE \
  --database REVIEWED_HOST:PORT/DATABASE
# Inspect dry-run report, then repeat the exact command with --apply.
```

The importer creates no invitations, passwords or sessions. Its mailbox-grant report
must be reviewed too: eligible existing explicit grants are rebound to the current
provider membership ID and queued for configuration reconciliation. Disabled grants
stay disabled. Suspended/unverified identities, removed memberships and customers
outside the reviewed teams are not automatically regranted; the report explains
those exclusions. Enabled legacy grants outside the reviewed teams block the entire
import before identity linking; resolve them explicitly rather than losing access
silently. Existing provider-qualified grants are not rebound on retries,
so a removed/reinvited member cannot regain access from an old manifest. Wait for
the corresponding configuration revisions to apply before checking mailbox content.

The schema/policy install must include the claimed-setup revalidation helper and
provider-qualified Mail grants. Workers keep restricted database credentials;
they must never receive the auth-bootstrap or offline-owner login. Inspect any partial
provider writes before retrying; external API mutations are not rolled back with SQL.
Stable external IDs/slugs reconcile retries. A rerun refuses to restore memberships
removed or demoted after linking. Run a final dry-run to verify the resulting mapping.
Disable the temporary migrator and remove temporary HBA/network/secret resources.

### Deployment and acceptance

1. Deploy the reviewed API/worker artifact and additive policies first; retain
   legacy routes. Verify `/readyz`, legacy CMS/OIDC/MCP, job processing and logs.
2. Merge the approved PR only after the owner has confirmed the production target
   and the above gates. Deploy/promote the Console artifact with the matching
   issuer/key/flag configuration. Avoid mixing development and production issuers.
3. Verify with real approved identities: operator registry, customer A/B isolation,
   Admin/Member rights, pending access, logout/revocation and local bans/customer
   archival. Exercise a stale form after an org switch and in a second tab.
4. Check account recovery/invitation with an explicitly approved recipient; no
   real invitation email is authorized by the temporary fixture approval.
5. Exercise fixed integration callbacks, read-only preview/expiry, database viewer
   renewal/revocation, mailbox explicit grants, queued-job revalidation and provider
   failure/recovery. A healthy home page is not full acceptance.
6. Record actual image digests, frontend deployment ID, migration versions,
   app/instance tuple, test outcomes and rollback decision in release evidence.
   Temporary development fixtures may be removed separately after acceptance;
   they must never be promoted or included in production manifests.

## Rollback

- Prefer a forward fix or disable the Console activation flag on Console/API while
  keeping this migration-aware backend and additive schema. Keep Clerk verifier
  configuration on API/worker: linked legacy clients must still consult current
  provider memberships. Missing/wrong issuer must fail closed.
- Do **not** restore a pre-Clerk API binary after linking identities: that can
  restore stale local membership authority. Do not delete mappings or copy provider
  membership back into local membership rows to make login work.
- Existing imported users retain explicit legacy-login eligibility. Newly projected
  Clerk-only users deliberately have no legacy password/session eligibility; a
  rollback may leave them unable to log in until Clerk access is restored. Do not
  enable password reset as a workaround.
- Preserve all additive tables, grants, user/customer history and retained Mail data.
  Provider changes and SQL changes are separate; restoring a DB backup alone does
  not restore provider state. Reconcile before restarting writers.
- Recheck revoked/demoted users, customer isolation, viewer/job denial and legacy
  authentication before declaring recovery complete.
