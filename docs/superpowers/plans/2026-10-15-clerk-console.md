# Clerk Console implementation plan

> Release stand: the native-team-policy amendment at the end of this document
> supersedes earlier strict team invariants. Current architecture is API-owned
> Drizzle/RLS, not Payload. Use the preflight adjustment and
> [release guide](../../architecture/clerk-console-release.md) for execution.

> **For agentic workers:** Use `superpowers:executing-plans` for inline implementation, task by task. No subagent execution is selected. Steps are unchecked because this is a plan, not implementation evidence.

**Goal:** Move Webdock Console and its four existing accounts to Clerk Free, with customer teams and preserved business access.

**Architecture:** Clerk owns login, sessions and organization memberships. Console passes verified Clerk identity to the existing business backend through new, explicitly separate endpoints. Webdock retains stable local IDs, operator permissions, customer/project data and a narrow legacy authorization bridge.

**Tech stack:** Node.js 24.x, Next.js 16.3.8, React 19.2.8, npm workspaces, current Clerk Next.js/backend SDKs, existing PostgreSQL and Payload registry.

**Spec:** [Design and four-account runbook](../specs/2026-10-15-clerk-console-design.md).

## Global constraints

- Clerk application: `app_3KjGKUGb7dYUMgkjKC0eCWghnOB`.
- Clerk plan: Free; no paid add-ons, MFA, passkeys, custom roles, satellite domains or Billing.
- Runtime: Node.js 24.x; preserve Next.js 16.3.8 and React 19.2.8.
- Package manager: npm workspaces with root `package-lock.json`.
- Console origin: `https://console.webdock.dev`; preserve Studio/OAuth/MCP compatibility URLs.
- Preserve decimal Snowflake IDs as strings; store Clerk IDs separately.
- Do not read or print existing environment files or credentials.
- No live migration, test-user creation, invitation email, deployment or auth replacement during planning.
- Customer Payload CMS login is deferred; Console's internal Payload registry remains in use.
- Four separate users; never merge the operator and Dominik/Tabularis identities.

User-approved operator policy: email/password allowed without MFA. First-login password setup is acceptable in the proposed migration; no password conversion machinery. Existing local user roles are retained, never inferred from names or emails.

## Review focus

1. An old valid JWT after membership/session removal must fail the next protected request, including relevant legacy access (tasks 2–3, 7).
2. A stale customer-A form after switching to B must not mutate B or bypass tenant checks (tasks 3–4, 7).
3. Operator and customer accounts belonging to the same person must remain separate through migration, callbacks and rollback (tasks 4–6).
4. Hosting/lifecycle/preview must not depend on a fabricated Better Auth session or accept a user-supplied operator flag (tasks 2–4).
5. Clerk development features, direct provider APIs and sibling-domain cookies must not bypass the Free production policy (tasks 1, 5, 7).

## Task 1 — Approval, exact application and workspace setup

**Files:** `apps/admin/package.json`, `apps/auth/package.json`, root `package-lock.json`; later integration files belong to task 4. No changes to `apps/web` or `apps/cms` for Clerk setup.

- [ ] Obtain approval of the supplied setup checklist and this migration plan. Clarify that approval covers code/setup; production cutover and outgoing invitation emails remain separately reviewable actions.
- [ ] Have the user run `npx -y clerk@latest auth login` in their host terminal. Dashboard sign-in does not authenticate the CLI.
- [ ] Inspect exact app/instance status and installed Clerk versions without opening env files. Do not run `apps list`, create an app or use accountless mode. Abort on a link to another app instead of replacing it.
- [ ] Preserve the dirty working tree. Create isolation only if it contains the relevant current source; do not base a clean worktree on stale HEAD while excluding needed uncommitted features.
- [ ] Read the installed Next.js authentication/proxy guides and the current Clerk setup/CLI skills. After approval, install Clerk's agent skills once; use `--no-skills` only if already available or declined.
- [ ] Inspect `init --help` for monorepo targeting. From `apps/admin` (the Next.js app root), run the following, keeping the existing app ID:

```bash
npx -y clerk@latest init --app app_3KjGKUGb7dYUMgkjKC0eCWghnOB
```

- [ ] Review generated changes; retain current Next/React pins, route groups, translations and npm lockfile. Restore unintended scaffold edits without touching user work. Configure the backend SDK in `apps/auth` for server verification only; do not scaffold a second login application there.
- [ ] Confirm the selected **development** instance and secret destination without printing values. In the already-integrated branch, use `link --app …`/`env pull` as needed instead of rerunning init.
- [ ] Check Free-equivalent settings: email/password + email codes, invite-only access, Organizations enabled, user organization creation disabled, only standard roles, no MFA/passkey requirement. Preserve remote settings not in scope; review configuration diffs before applying.
- [ ] Plan exact Console application domain/cookie scope and origin restrictions. Google is optional; its production credentials are not a prerequisite for password launch.

**Deliverable:** correct existing Clerk app linked to the correct workspace, with reviewable setup changes and no auth cutover.

## Task 2 — Stable mappings and trusted principals

**Create:** `apps/auth/src/lib/clerk-identity.ts`, `apps/auth/src/lib/clerk-schema.ts`, `apps/auth/src/lib/principal.ts`, `apps/auth/tests/clerk-identity.test.ts`, `apps/auth/tests/clerk-schema.test.ts`.
**Modify:** `apps/auth/scripts/migrate.ts` to include additive schema in its existing explicit migration flow.

The server-only interfaces are:

```ts
export type ConsolePrincipal = Readonly<{
  provider: 'clerk'; issuer: string;
  userID: string; clerkUserID: string; sessionID: string;
  operator: boolean; organizationID: string | null;
  customerID: string | null; role: 'admin' | 'member' | null;
}>;
export type LegacyPrincipal = Readonly<{
  provider: 'legacy'; userID: string; sessionID: string;
  operator: boolean;
}>;
export type BusinessPrincipal = ConsolePrincipal | LegacyPrincipal;
// Only authentication-boundary modules construct these values.
export async function requireConsolePrincipal(request: Request): Promise<ConsolePrincipal>;
export async function requireLegacyPrincipal(headers: Headers): Promise<LegacyPrincipal>;
export async function requireTenantAccess(
  principal: BusinessPrincipal,
  customerID: string,
  write: boolean,
): Promise<{ organizationID: string; role: 'admin' | 'member'; operator: boolean }>;
```

Implement interfaces in their named modules, rather than adding ambient declarations. Never deserialize a `BusinessPrincipal` directly from an API body.

- [ ] Add the three tables in the spec: identity links, organization links and Clerk-scoped preview. Require unique issuer/user and issuer/local-user mappings, foreign keys to existing local IDs, and explicit enabled state. No session/token/password storage.
- [ ] Add failing tests for conflicting mapping, unknown Clerk ID with an existing email, disabled local account, unverified email, foreign issuer/authorized party, expired/revoked session, missing service credential and wrong token type.
- [ ] Implement Clerk SDK verification and live session/user lookup. Validate `sub` against the returned session user. Pin exact deployment issuer/origins; do not trust caller-supplied issuer/JWKS URL. Validate the dedicated service key before accepting confidential backend context.
- [ ] Resolve local operator permission and customer mapping. Fetch current membership for customer requests, paginating where needed. Distinguish invalid credentials (401), unavailable provider (503/429), forbidden action (403) and inaccessible resource (404).
- [ ] Limit memoization to a single request. Do not accept stale claims as the sole membership/suspension check.
- [ ] Run the focused tests and additive migration on an isolated local test database. Verify existing local IDs/constraints remain unchanged.

**Deliverable:** a Clerk identity can be mapped and authorized without a Better Auth session.

## Task 3 — Business API, hosting, preview and legacy policy

**Create:** `apps/auth/src/app/api/console/route.ts`, `apps/auth/src/app/api/console/hosting/route.ts`, `apps/auth/src/app/api/console/lifecycle/route.ts`, `apps/auth/src/lib/console-api.ts`, `apps/auth/tests/console-api.test.ts`.
**Modify:** existing `studio-api.ts`, `access-management.ts`, `tenants.ts`, `plans.ts`, `tenant-mail.ts`, `mail-domains.ts`, `mail-keys.ts`, `mail-tracking.ts`, `storage-usage.ts`, `tenant-preview.ts`, `lifecycle.ts`, `authorization.ts`, `tenant-policy.ts`, `mcp.ts`; hosting `bridge.ts`, `authorization.ts`, `clusters.ts` and BYOK code that checks session identity. Adapt `packages/hosting-contracts/src/index.ts` only where the actor type requires provider information.

- [ ] Add tests for a verified Clerk operator/customer operating without any Better Auth session row, plus wrong customer, inactive customer, removed member and attempted role escalation.
- [ ] Extract the existing operation dispatch so both explicit boundaries can call business services. Preserve existing request-size bounds, allowlisted operations, safe errors and response limits.
- [ ] Replace business-service cookie reads with `BusinessPrincipal` checks. Legacy pages/actions resolve their existing principal explicitly before calling services. Find all callers of changed signatures and update them together.
- [ ] Add the new endpoints using `Authorization: Bearer` + `X-Webdock-Service-Key`. Keep legacy endpoint schemas and token interpretation separate. Carry trusted identity through hosting and lifecycle, preserving scopes and destructive-action confirmation.
- [ ] Adapt preview storage by identity provider: Clerk preview uses the new table; old clients keep their existing table. Preserve expiry, explicit exit, read-only restrictions, audit actor and denial of live secrets/logs/actions.
- [ ] For linked users/organizations, apply the shared live Clerk membership/local-disable check to legacy central authorization. Disable legacy membership writes for migrated teams. Keep old token/audience/MFA checks intact.
- [ ] Test old-token/new-endpoint and Clerk-token/legacy-endpoint rejection, direct confidential-context submission, stale preview, attempted preview escape via org switch and legacy access after member removal.
- [ ] Run affected auth/hosting/lifecycle/MCP tests against the isolated fixture. A zero-Better-Auth-session Console request must succeed for an authorized actor and fail for an unauthorized actor.

**Deliverable:** existing business capabilities work through Clerk and no stale legacy membership reopens removed access.

## Task 4 — Console screens, guards and provider callbacks

**Create:** `apps/admin/src/proxy.ts`, `apps/admin/src/lib/console-identity.ts`, `apps/admin/src/app/(console)/login/[[...login]]/page.tsx`, `apps/admin/src/app/(console)/sign-up/[[...sign-up]]/page.tsx`, `apps/admin/src/app/(console)/account/[[...account]]/page.tsx`, `apps/admin/src/components/account-controls.tsx`, `apps/admin/tests/clerk-console.test.ts`.
**Modify:** replace existing `apps/admin/src/app/(console)/login/page.tsx` when introducing the catch-all; Console/workspace/portal layouts, `studio-shell.tsx`, `navigation.tsx`, `studio-client.ts`, `hosting-client.ts`, `auth.ts`, `server.ts`, `preview-guard.ts`, `github-routes.ts`, `vercel-routes.ts`, `project-deletion.ts`, `project-deletion-actions.ts` and affected contracts/types.

- [ ] Add tests for protected Console routes, permitted invite/login routes, denied cross-customer mutations, operator/customer separation and safe same-origin return URLs.
- [ ] Place current `ClerkProvider` inside `<body>` in the existing Console root layout. Preserve i18n, fonts and theme. Use current async `auth()` and Next.js `proxy.ts` conventions from installed/live docs.
- [ ] Implement login/invitation/profile using Clerk components; put UserButton and OrganizationSwitcher in both navigation variants. Remove Console links to old account/logout endpoints without altering raw CMS endpoints.
- [ ] Implement `getConsoleIdentity()` in `console-identity.ts` for the browser/server boundary. Forward its Clerk token only to the fixed new backend origin and endpoints. Add the server-only service credential; never expose either in UI/logs.
- [ ] Update `studio-client.ts`/`hosting-client.ts` without weakening their timeout, payload-size and error behavior. Keep external OAuth/MCP token callers on their legacy path; do not change `hostingTokenCall` globally to assume every token is Clerk.
- [ ] Replace Console `requireOperator()` and GitHub/Vercel guards with Clerk/local operator validation followed by an exact `authSubject` Payload-user projection. Keep Local API calls using `overrideAccess:false`. Missing mapping denies access.
- [ ] Extend `RegistryActor` or its browser call context explicitly where lifecycle needs Clerk delegation. Never forward a Clerk token as an opaque legacy lifecycle token. Preserve integration callback state/actor binding.
- [ ] Replace preview detection based only on the old SSO cookie with verified Console context. A pending/expired preview still restricts every operator surface.
- [ ] Keep legacy CMS/MCP/registry machine routes and provider callback validation independent of browser Proxy redirects. Test an unauthenticated machine request gets its protocol-specific 401, not login HTML.
- [ ] Verify org A/B switching, stale forms, no-membership state, logout and multiple-tab behavior. Pass an explicit organization context/token for background client calls rather than relying on whichever tab last changed the global cookie.

**Deliverable:** the four account types can use the existing Console UI under Clerk, including registry and integrations.

## Task 5 — Customer teams through Clerk

**Create:** `apps/auth/src/lib/clerk-teams.ts`, `apps/auth/tests/clerk-teams.test.ts`.
**Modify:** `tenants.ts`, `access-management.ts`, `tenant-policy.ts`, corresponding Console tenant/people forms and `studio-contracts.ts`.

- [ ] Add tests for existing owner/admin/member role mapping, default Member invitation, no global operator elevation, invitation into a blocked customer, invalid org/customer mapping and last-admin removal/demotion (including concurrent requests).
- [ ] Implement team reads/mutations with Clerk Backend API; retain local customer data and project grants. Do not mirror membership into a second database authority or create password-bearing accounts for invitees.
- [ ] Disable user-created organizations in Clerk. Verify exact role permissions prevent direct Frontend API bypass of Webdock-only membership mutations; otherwise ship operator-managed mutation controls until revised.
- [ ] Add a simple create/link-team action for operators. Preserve the existing local customer/organization trigger; no outbound calls inside it. Before retrying a partial Clerk create, reconcile the stable customer-linked organization instead of creating another.
- [ ] Keep the last-admin invariant, block customer suspension bypass, and configure controlled self-deletion. Do not add platform operators to tenant teams automatically.
- [ ] Test invite accept/resend/revoke with approved test identities only. Verify role removal is reflected on the next business request even before any browser refresh.
- [ ] Show Free limit errors (20 members) clearly and do not auto-upgrade a subscription.

**Deliverable:** basic team management works with Clerk as the only membership authority for mapped customers.

## Task 6 — One-time migration of four accounts

**Create:** `apps/auth/scripts/migrate-clerk-users.ts`, `apps/auth/tests/clerk-migration.test.ts`.
**Input:** a private reviewed four-row manifest of existing local IDs, desired exact Clerk links and existing customer memberships. No credentials in source or committed manifests.

- [ ] Default to dry-run. Require an explicit instance and apply flag; refuse an app other than `app_3KjGKUGb7dYUMgkjKC0eCWghnOB`. Do not implement all-user discovery/import, password conversion or background synchronization.
- [ ] Add tests for exactly four distinct identities, repeat-run no-op, existing external-ID mapping, duplicate-email conflict, unverified email preservation, existing disabled accounts and unchanged local operator/project IDs.
- [ ] Implement create/link using stable external IDs and explicit identity/organization mappings. Reuse provider objects on repeat runs; stop contradictory mappings. Handle rate limits and partial completion without replaying successful creations.
- [ ] Keep email verification truth explicit. Use email code/Clerk password setup instead of importing password hashes. No account merging and no new global role assignments.
- [ ] Prepare the two team mappings and reviewed memberships; print only counts, safe IDs and discrepancies. Do not log addresses, hashes, tokens or provider secrets.
- [ ] Obtain explicit approval before creating test users in the existing app or sending activation/invitation emails. The dry-run does neither.
- [ ] In development, exercise the import with authorized synthetic fixtures, then repeat it to verify no duplication. Production uses the reviewed real four rows only after rollout approval.

Example assertion table for the migration tests (fixtures are synthetic, not real users):

```ts
const expectedAccounts = [
  { localID: '1001', kind: 'operator', team: null },
  { localID: '1002', kind: 'customer', team: 'tabularis' },
  { localID: '1003', kind: 'customer', team: 'tabularis' },
  { localID: '1004', kind: 'customer', team: 'schattenclan' },
] as const;
assert.equal(new Set(expectedAccounts.map(a => a.localID)).size, 4);
// The integration test also runs the importer twice against a fake provider
// and fixture database, asserting four total create-user calls, stable links,
// zero email deliveries and no change to original role/grant rows.
```

**Deliverable:** a small rerunnable migration and a reviewed account-to-team mapping; passwords are re-enrolled, not converted.

## Task 7 — Verification, production cutover and recovery

**Tests:** new Clerk suites plus existing `studio-api`, `tenants`, `tenant-preview`, `hosting`, `lifecycle`, `mcp`, integration callback and Console permission tests.
**Docs:** update `apps/auth/README.md`, `apps/admin/README.md` and internal auth rollout status after actual implementation; describe what is still legacy accurately.

- [ ] Run focused tests while implementing; then run the workspace typechecks, lint and affected full suites on a disposable local database. Existing test scripts load local test configuration and can reset test tables: verify their local-only guard before execution, never use production.

```bash
npm run check -w @spitzli/admin
npm run check -w @spitzli/auth
npm run lint -w @spitzli/admin
npm run lint -w @spitzli/auth
npm run test -w @spitzli/admin
npm run test -w @spitzli/auth
npm run test:sso
npm run build -w @spitzli/admin
npm run build -w @spitzli/auth
```

- [ ] Run `npx -y clerk@latest doctor` in the Console workspace and start the app. Inspect with T3 `preview_status` then `preview_open` as needed. Use approved accounts or the user-created initial account; never create a test user silently.
- [ ] Browser acceptance: login/reset/code/invite, user/team controls, all four role contexts, wrong-customer URLs, old-token removal, preview expiry/exit, callback state, logout, mobile and second-tab behavior.
- [ ] Inspect actual cookie scope and allowed origins with the real configured domain; require no customer-domain exposure. Verify Free production settings, not just development availability.
- [ ] Prepare production keys through CLI/hosting secret injection without printing or opening env files. `npx -y clerk@latest deploy` concerns Clerk production configuration, not deployment of Webdock code. Inspect current CLI behavior; use `deploy status` to verify. Do not assume dev users or integrations copy to production.
- [ ] Vercel CLI is currently unavailable; installation with `npm i -g vercel` is recommended for subsequent environment/deployment/log operations. No unrelated Vercel integration provisioning is needed for the already-selected Clerk app.
- [ ] Obtain production cutover approval for the reviewed diff, four-user report, affected deployments and rollback steps. Temporarily freeze identity/team edits; back up affected records; deploy backend before Console.
- [ ] Apply the four-user/two-team migration and validate mappings. Switch Console atomically; do not accept legacy browser cookies as fallback. Verify operator, Andrea/Tabularis, Dominik/Tabularis and Schattenclan individually.
- [ ] Rehearse rollback with fixtures first: a Clerk member removal stays removed after reverting Console; local suspensions persist; current business data is untouched. Explain that users must use old credentials if returning to the old login.
- [ ] Update completion evidence with tests actually run and the retained legacy endpoints. Do not delete Better Auth, its tables, keys or deployment while deferred consumers depend on them.

**Done when:** four separate accounts can use Console with correct rights, all business paths work without Console Better Auth sessions, team revocation is consistent, and rollback is documented and rehearsed. External CMS/MCP migration and total legacy-provider retirement remain explicitly outside this release.

## Required setup approval text

Present the user's supplied checklist before any setup mutation:

```text
Here's what I'll do to get you set up with Clerk.

1. Set up Clerk in this project, or scaffold a new app if this directory is empty.
2. Install Clerk's agent skills for the coding agents on this machine, if they aren't installed yet.
3. Start your app with Clerk installed.

Shall I proceed?
```

For this existing monorepo, step 1 means the existing Console workspace and the selected application, not scaffolding a new app. This gate comes from the user's supplied setup instructions. No CLI authentication, account mutation, data migration, email sending or runtime changes have been performed while writing this plan.


## Verbindlicher Nachtrag vom 2026-10-15: native Teamregeln

Der Betreiber hat ausdrücklich „Clerk-Standardregeln übernehmen“ gewählt.
Kundenteams verwenden Clerks native Admin-/Member-Regeln. Webdock-Datensperren
bleiben bestehen. Frühere Forderungen in diesem Dokument nach global garantierter
Einladungssperre bei lokal gesperrten Kunden oder mindestens einem lokal aktiven
Admin gelten damit nicht mehr gegenüber direkten Clerk-Aufrufen. Keine zusätzlichen
Custom Roles oder kostenpflichtigen Rollenfunktionen sind vorgesehen.

Organisations-Slugs sind ausdrücklich gewünscht und in der Entwicklungsinstanz
aktiviert. Vier temporäre Testkonten und zwei Testorganisationen mit eindeutigen
`+clerk_test@example.com`-Adressen sind separat freigegeben, ohne Einladungsmails
und ohne Produktionsdaten. Diese Freigabe umfasst keinen Produktions-Cutover.
