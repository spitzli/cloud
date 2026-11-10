# Clerk Console: execution preflight for the API architecture

Date: 2026-10-15. Baseline: `8f92389`.
Status: expanded implementation authorized; migration in progress. See
`docs/verification/2026-10-15-clerk-console.md` for the implementation checkpoint
and outstanding requirements. The setup/preflight findings below are historical.

The owner approved implementation of `2026-10-15-clerk-console.md` and its
design through the request “setz das um”. Production cutover, real-user
migration, test-user creation and invitation delivery have not been performed.
The two original documents were copied from the main checkout unchanged.

## Completed setup

- Existing Clerk application `app_3KjGKUGb7dYUMgkjKC0eCWghnOB` linked by CLI.
- Existing development instance `ins_3KjGKQnAATh2cA4ddJBR2cg3NXu` verified by
  `clerk doctor --json`. No production instance is configured.
- CLI authentication was already valid; no further login is required.
- CLI installed its eight Clerk agent skills for Codex.
- Console dependency `@clerk/nextjs` 7.9.15 and API dependency
  `@clerk/backend` 3.23.2 installed using the npm workspace lockfile.
- Next.js 16.3.8 and React 19.2.8 pins preserved.
- CLI reports development keys present in Console `.env.local`. Existing
  environment files and credential values were not opened or printed.
- The generated top-level sign-in/sign-up routes and blanket Clerk middleware
  were removed: they do not belong in the existing Console layout and would
  activate part of a migration before backend authorization is ready.
- No application runtime authentication or remote application settings changed.

The initial CLI invocation used the shell's Node 22. Subsequent SDK installation
and typechecks use the available Node 24.21.0 installation.

## Source-backed differences from the supplied plan

| Original assumption | Current source | Required adjustment |
| --- | --- | --- |
| Business backend is `apps/auth` | `apps/auth/src/lib/studio-client.ts` delegates to `apps/api`; business modules are under `apps/api/src/modules` | Put Clerk verification, business principals and migrations in `apps/api`; keep `apps/auth` as the legacy UI |
| Console delegates directly to the old Studio endpoint | `apps/admin/src/lib/studio-client.ts` calls `/v1/studio`; `apps/api/src/protocols/portal.ts` verifies the existing portal cookie | Add an explicitly separate Clerk transport to the existing API mount/ingress architecture; retain the legacy protocol |
| Console registry calls `payload.auth()` | `apps/api/src/modules/registry/service.ts` uses Drizzle, signed RLS context and immutable `authSubject`; integrations use `currentRegistryActor()` | Preserve that registry and projection; do not restore Payload or invent a Payload login |
| A service-level principal removes the dependency on Better Auth sessions | `apps/api/src/db/database-rls-schema.ts` joins `webdock_auth.session` in `webdock_rls.actor()` and local `member` in customer access | Implement a distinct, server-verified Clerk database scope with negative RLS tests; do not disable RLS or manufacture legacy sessions |
| Only three new mapping/preview tables are needed | `apps/api/src/modules/databases/schema.ts` and `drizzle/0022_managed_database_viewer.sql` have session foreign keys to Better Auth | Provider-qualify viewer launch/session records and retain legacy referential integrity; additive tables alone cannot make Clerk viewer access work |
| Hosting and lifecycle are the only downstream identity changes | Billing uses session-only scope in `modules/billing/context.ts`; setup workers join local sessions in `db/database-setup-rls.ts` | Cover billing, queued setup authorization, registry and viewer as first-class migration dependencies |

## Proposed execution adjustment

Keep the approved product decisions: four separate identities, Clerk Free,
unchanged business IDs and permissions, no Better Auth session fabrication,
live membership checks, and independent legacy CMS/MCP protocols.

1. Replace task 2's `apps/auth/src/lib` targets with API authentication modules.
   Create the three mapping/preview tables through the API offline migration
   journal. Preserve the exact issuer and app/instance targeting.
2. Extend the database-runtime scope union with a Clerk-specific context that
   only the authentication boundary constructs after live provider verification.
   It must carry provider identity, the verified organization and bounded
   verification freshness; browser input cannot supply these assertions.
3. Teach SQL authorization to validate the signed context, immutable mappings,
   local suspension, resource relationships and preview state without joining a
   Better Auth session. Local role remains the operator authority. Migrated
   memberships must never fall back to legacy member rows.
4. Extend the viewer's launch/session schema with explicit provider identity.
   Keep the old session foreign key for legacy records; add constraints that
   prevent ambiguous provider records. Recheck Clerk session and access on
   exchange and subsequent viewer requests. Do not drop the foreign key and
   treat arbitrary session strings as authentication.
5. Adapt registry, billing, hosting, lifecycle and mail contexts together.
   Queued setup work revalidates the stored provider identity before privileged
   actions and stops when authorization is no longer available. It must not
   retain a request-scoped proof as permanent job authority.
6. Add Clerk Console transport and UI only after the zero-legacy-session tests
   pass. Preserve existing ingress host classification, callback state checks,
   safe errors, size limits and resource-specific permission checks.
7. Continue the supplied team-management, four-row dry-run migration and rollout
   tasks on this architecture. The remote development configuration still needs
   review before activation; do not silently change production or send emails.

This extends the original minimal schema scope and changes the database trust
boundary. It is a substantive design correction, not a filename replacement.

## Required acceptance coverage for the correction

- Clerk operator and customer requests succeed with no Better Auth session rows,
  using the real restricted database roles, not just a privileged fixture pool.
- Invalid signature, stale verification, wrong issuer, disabled mapping, local
  suspension, wrong organization and removed membership deny data access.
- SQL customer, project and registry authorization cannot use stale legacy
  membership or a caller-supplied operator flag.
- Preview remains read-only after expiry and organization switching, including
  billing, secrets, logs, database viewer and background actions.
- Viewer launch/session exchange cannot cross identity providers or replay a
  revoked session. Existing legacy viewer access retains its checks.
- A queued setup loses authority on revocation, even if it was enqueued before
  removal; provider outage does not reactivate legacy access.
- Legacy CMS/MCP retains protocol/MFA checks and respects migrated membership
  revocations. Rollback cannot reopen removed access.

## Remote configuration findings

Read-only `config pull --keys organization_settings` shows Organizations enabled,
forced organization selection enabled, organization creation defaults enabled,
administrator deletion enabled and a current membership maximum of 5. These
settings have not been changed. In particular, forced organization selection
needs review for the operator who must not be placed in every customer team.
The observed maximum is instance configuration, not proof of production Free
entitlement. The plan's assumed limit of 20 is not verified for this app.

No production instance, cookie-domain isolation, provider permission bypass
tests, four-account inventory or end-to-end migration acceptance is verified.

## Checks actually run

- `npm run check -w @spitzli/admin` with Node 24.21.0: passed.
- `npm run check -w @spitzli/api` with Node 24.21.0: passed.
- `git diff --check`: passed.
- Clerk doctor: existing app, valid authentication and development keys passed;
  production instance is not configured.

No runtime auth behavior was implemented, so no browser acceptance or migration
test is claimed. Before later Vercel environment/deployment/log work, installing
the missing CLI with `npm i -g vercel` is strongly recommended; it is not needed
for the local SDK preparation above.
