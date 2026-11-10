# Webdock self-hosted API — Implementation Plan

> **Implementation status (2026-10-14):** The owner's later instruction authorized implementation and real Polar sandbox testing. Code/local verification for A01–A15 and B01–B08 is complete. A16 runtime artifacts and local rehearsals are complete; production host/TLS/restore/cutover remain release gates. B09's real sandbox payment, upgrade, scheduled downgrade, portal cancellation, webhook replay and worker recovery ran; live pilot and real future renewal remain outside these results. See the [verification record](../../../verification/2026-10-14-api-rewrite.md) and [all523 source-file dispositions](implementation-inventory.json). Original checklists below remain the approved planning baseline; the verification record is the factual execution status.

> **For agentic workers:** Use `superpowers:executing-plans` for implementation; use `superpowers:subagent-driven-development` only after the owner explicitly selects delegation. This delivery authorizes planning, not implementation or production changes. Execute tasks independently, preserve existing changes, and collect evidence before each cutover.

**Goal:** Move all Studio/Auth business, identity, provider and background processing into self-hosted `apps/api`; preserve existing customers and public protocols, then add Polar billing.

**Architecture:** Modular Bun/Elysia monolith, separate API and worker processes from one image, Elysia-FSR for business routes, explicit protocol mounts. Existing PostgreSQL remains authoritative; Drizzle v1 uses the existing `pg`/signed-RLS boundary. Private Dragonfly holds disposable caches only. Studio/Auth remain Next UI and HTTP transport.

**Tech Stack:** Bun 1.4.3 candidate; Elysia 1.4.30; elysia-fsr 0.1.0; Drizzle ORM/Kit 1.0.0-rc.4; Better Auth/plugins 1.7.7; pg 8.16.3; Dragonfly 2.0.2 candidate; Polar SDK 1.0.2 / API 2026-10. Pin and validate exact artifacts in A01. Existing UI uses Next 16.3.8 / Node 24.

**Spec:** [Original API specification](sources/01-api-rewrite-plan.md), [Polar specification](sources/02-polar-design.md), [Polar tasks](sources/03-polar-implementation-plan.md), [FSR review](sources/04-elysia-fsr-review.md). These are archived handoff evidence, not new test results. This plan's current-baseline corrections take precedence over stale inventory in the handoff; product rules otherwise remain intact.

## Global constraints

- Owner reconfirmed Better Auth during planning. Do not write an authentication engine or provision Zitadel. Keep custom authorization in Webdock.
- Self-hosted API, never a new Vercel backend. No customer CMS or Python hosting-agent rewrite.
- Keep issuer `https://auth.webdock.dev/api/auth`, RP-ID `auth.webdock.dev`, host-only secure cookies, signing keys, user IDs and existing sessions.
- Keep Snowflake IDs as strings, DB defaults, triggers, locks, signed RLS and separate authentication/business/worker credentials. No owner/BYPASSRLS runtime.
- No `drizzle push`, automatic startup DDL, implicit production fixtures, broad DB grants, uncoordinated dual-writes or data deletion during migration.
- Outgoing-only mail uses API → Turbo, no Stalwart provisioning. Incoming only when explicitly activated, isolated Stalwart CE per customer on existing Contabo/k3s. Disable retains data; purge is separate. Preserve Mail key and SSA replica ownership.
- Existing manual/sponsored plans and extras remain valid. Polar sales/publication/enforcement start disabled; sandbox never changes live entitlements or resources.
- Keep npm workspace lockfile as dependency authority. Bun runs API; do not migrate all frontend runners or create competing root lockfiles.

## Review focus

1. A pooled connection reused across tenants or credentials must not carry identity into the next transaction (A02/A05).
2. An old CMS session must lose revoked website access, while Studio/payment access stays available (A03/B07).
3. A worker that times out after a successful external write must not repeat mail, checkout or provisioning (A09/A10/B03/B06).
4. A malformed route module, duplicate path or implicit HEAD must never expose private data (A01/A15).
5. Rollback after the new writer commits must preserve those records, audit, leases and sessions (A16).

## 1. Evidence and corrections to the handoff

Inspected 2026-10-13 at commit `95fc3b2ed4317f4d19bdba2c05530ae217924574`. Worktree was clean before this documentation. No product edits, installations, DB mutations or deployment operations were performed.

| Evidence | Consequence |
|---|---|
| [inventory.json](inventory.json): 64 route files, 35 server-action files, 523 source files, 122 Auth/Admin test files, 59 script files, 77 environment names | Scope exceeds the handoff's 57-test estimate. Inventory includes hidden discovery paths, imports, exports, switch dispatches and 254 static DDL declaration occurrences. These are not 254 distinct live DB objects. |
| GitHub deployments `7042112857` Studio and `7042107920` Auth report success for current SHA | Latest recorded production artifacts match checkout; GitHub status is not proof every public alias currently points there. |
| Studio artifact `https://webdock-admin-e770l8zuf-spitzli.vercel.app`; Auth artifact `https://webdock-auth-mpqi4wq2q-spitzli.vercel.app` | Record these for release comparison; use immutable origin URLs in routing tests, not the public domain as its own upstream. |
| Public Auth discovery returns 200 and issuer `https://auth.webdock.dev/api/auth`; Studio MCP discovery returns 200 and audience `https://studio.webdock.dev/api/mcp` | Both protocol identities are observed live, not inferred from environment defaults. |
| `api.webdock.dev` resolves to `178.104.141.95`; normal TLS verification fails with a self-signed certificate | Not ready for clients/webhooks. Do not bypass verification in production. Host capacity and deployment access remain unverified. |
| `docs/verification/2026-10-12-webdock-platform-migration.json` | Platform DB migration already completed, source fenced, 294 tables reported FORCE RLS. Do not repeat old database migration or revive source credentials. |
| Same evidence: automated backups disabled; isolated restore previously verified | A current recovery point and restore rehearsal are cutover gates; do not claim continuous backup/PITR exists. |
| `packages/database-runtime/src/index.ts`, Auth `db.ts`, `database-context.ts`, `database-rls-schema.ts` | Transaction-local signed scopes and exact audience/session_user are a mandatory Drizzle integration contract. A generic pool replacement breaks authorization. |
| `docs/verification/2026-10-13-customer-plans-navigation.md`, current `plans.ts` | Customer-specific `assign-custom`, immutable history and preserved extras are already implemented. The old reported relocation/MCP mismatch is not assumed to persist. |
| `mail-product/jobs.ts`, `cron.ts`, `infra/mail/cronjobs.yaml` | Durable mail lease/state machinery already exists; reuse it. Cron route checks VERCEL_ENV; YAML is suspended, not evidence of a running scheduler. |
| `hosting/bridge.ts` accepts confidential `vercelRuntime` from Studio | Move credential ownership to API; delete this transport field only after every caller changes. Never accept it from browser commands. |
| `hosting/http.ts` already supports non-Vercel `SPITZLI_HOSTING_EU_VERIFIED` | Preserve location admission, supply evidence for the real host. Do not merely set a flag to bypass it. |
| No node_modules or `.env.test.local` in this worktree; shell Node 22.22.3 versus required 24 | Full app suites/builds not run here. Prior release test results are historical evidence only. No shared-root install or production test fallback. |
| Current kubectl context is `webdock-byok-test` | It does not establish production cluster capacity. No context mutation or cluster provisioning was attempted. |

Local `RTK.md` was absent; available guidance read from `/home/newt/.claude/RTK.md`; RTK 0.38.0 present. Next route/rewrites docs were read from the original checkout's installed package because this worktree lacks dependencies. Re-read the installed version after `npm ci` during implementation.

Registry lookup reconfirmed Elysia 1.4.30, FSR 0.1.0, Better Auth 1.7.7 and Polar 1.0.2. Drizzle v1 remains RC. Bun/Dragonfly candidates here are inherited from the dated handoff, not independently runtime-certified.

## 2. Inventory, authorization and target interfaces

[Route manifest](routes.md) gives every existing route's public host, method exports, policy category, destination and parity test. [Offline script disposition](scripts.md) covers operational entrypoints. [Machine inventory](inventory.json) gives all source imports/exports, action files, script files, test files, environment names and schema declarations. The inventory is a static coverage aid, not a complete runtime call graph. Dynamic dispatch is covered by enumerating authoritative command schemas in A01 and A15.

Use `/v1/studio` for the existing typed operation envelope during migration, not a second rewritten REST surface for every operation. It is an explicit mixed-operation protocol with operation-specific authorization, not a blanket tenant guard. New billing and registry APIs use their named business trees. Preserve legacy route aliases until all callers are verified migrated. Do not expose compatibility bridges to broader hosts just because their code lives in the same process.

| Boundary | Required current identity and checks | Denial contract |
|---|---|---|
| `auth_protocol` | Native Better Auth; exact trusted origin, fixed canonical URL, closed signup, operator MFA, native rate limits | Preserve native protocol errors and redirects |
| `public` | Only explicit discovery/OpenAPI/health content | No private DTOs; unknown host/path 404 |
| `tenant` | Valid session, active account/customer, membership; admin/owner for writes; object ownership; preview restrictions | Anonymous 401; known missing privilege 403; cross-tenant object 404 |
| `operator` | Current operator, verified/setup-complete account, MFA; reject preview, including expired preview until exit | 401/403; recheck inside write transaction |
| `delegated` | Exact confidential client + introspected user token + current session; operation allowlist; RLS actor derived server-side | No actor/customer/role trust from caller hints |
| `mail_key` | `mailOperations` permission (`sending_access` vs `full_access`), active key/customer/connection, domain scope, revocation, RLS | Preserve Mail error body/status, If-Match, idempotency and rate headers |
| `agent` | Specific enrollment/agent/build-worker/gateway credentials, target/cluster binding, replay/revision rules | Never substitute a browser session or generic system scope |
| `mcp` | Exact issuer/audience/token/session, scopes; separate hosting and registry policy | Preserve WWW-Authenticate, transport errors, read/write tool visibility |
| `signed_webhook` | Provider-specific raw-body verification, bounded input, durable dedupe | No global browser login/CSRF; no success before durable acceptance |
| `job` | Authenticated scheduler/dispatcher installs explicit permitted system purpose | No public worker trigger in final API |
| `ui` | Locale preference route can stay UI, same-origin validation | No DB/provider/auth-engine access |

FSR `tenant` and `operator` roots have static guards installed before mount. Host/method/size/request-ID/error/log hooks surround all routes; they must preserve unconsumed body and multi-Set-Cookie for native handlers. Explicit mounts own Better Auth, discovery, MCP, agents and signed webhooks. Host is part of route identity. Normalize parameter names for collision detection; compare BEFORE FSR/Elysia deduplication. Test Next's implicit HEAD/OPTIONS separately.

Actor is request-local. Use the existing `DatabaseScope`, `HostingActor`, `MailActor` contracts, not an all-powerful new identity object. A handler must return/await the entire `withDatabaseScope(scope, () => service())` callback; setting AsyncLocalStorage inside a hook and then leaving that callback before the handler executes is insufficient.

Proposed API contracts (new code, signatures to implement):

```ts
// apps/api/src/app.ts — no listening or migrations on import
export async function createApp(config: ApiConfig): Promise<Elysia>;
// apps/api/src/config.ts — validated server-only config, no frontend export
export function readConfig(env: Record<string, string | undefined>): ApiConfig;
// apps/api/src/db/client.ts
export function createDatabases(config: ApiConfig): {
  business: ScopedPool; authentication: Pool; worker: ScopedPool;
  close(): Promise<void>;
};
// packages/api-client/src/index.ts — no provider/DB/auth-server dependencies
export function apiCall<T>(path: string, init: RequestInit): Promise<T>;
```

`ApiConfig` contains canonical auth/studio/api origins, trusted ingress CIDRs, environment, role-specific DB URLs/CA/signing key, required module set, bounded pool sizes, cache URL and read-only rollout ownership flags. It validates production secrets and flags at startup; no production fallback to local DB or unscoped pool. Error envelopes apply only to new business endpoints. Existing bridges keep their current response contracts while callers migrate.

## 3. Schema and secret ownership

| Objects | Current owner/source | Target and transfer rule |
|---|---|---|
| Better Auth tables, OAuth/JWKS, MFA/passkeys, rateLimit | `apps/auth/scripts/migrate.ts`, `bootstrap.ts`, Better Auth migrations | API auth schema after adapter parity. Preserve exact quoted names/defaults; no identity regeneration. |
| Customers/projects/cms_instances/users/audit_events and Payload relations/history | `apps/admin/src/migrations/*`, `cms/collections.ts` | API registry schema after hook parity; preserve tables/history, retire old Payload migrator. |
| Tenant/access/preview/plans/platform/storage | Auth schema modules and `scripts/migrate-*.ts` | API named schema modules, retain source SQL constraints and revisions. |
| Mail product plus `webdock_mail` runtime | `mail-product/schema.ts`, native mailbox/send schemas, mail-core | API control tables; Mail runtime stays separate. No blanket ownership of entire Mail service. |
| Hosting/git/database control tables | `hosting/*schema.ts`, `databases/schema.ts` | API services own control records; Python agent/database gateway/build-worker remain separate executors. |
| `webdock_rls` policies/functions/key ACL; role grants | `database-rls-schema.ts`, `scripts/platform-database-migration/roles.mjs` | Privileged offline migration only; reviewed SQL alongside Drizzle migration. Runtime cannot redefine policies. |
| `webdock` site data, archived `public`, independent customer databases | Existing site/archive ownership | Excluded from new API DDL unless a separately specified business operation needs access. |
| New `billing_*` | No current owner | API migration only; explicit FORCE RLS/policies/grants, including worker purposes; never rely on the current unknown-table fallback policy. |

A02 produces `apps/api/drizzle/0000_baseline.sql`, `apps/api/src/db/schema/{auth,registry,access,plans,platform,mail,hosting,databases}.ts`, `apps/api/src/db/rls.sql` and `apps/api/scripts/migrate.ts`. Baseline is a structural comparison artifact, not SQL to recreate production tables. Review generated diff against actual schema, defaults, constraints, indexes, functions, triggers, RLS, grants and search paths. Subsequent additive migrations use one migration lock, a direct verified-TLS migrator connection and transaction where supported. Existing migration journals stay intact. Do not let both Payload and Drizzle migrate the same objects.

Retain `@spitzli/database-runtime`, exact role/audience semantics, certificate verification and tenant context. Drizzle uses checked-out ScopedPool clients for transaction-bound SQL; prove no separate pool query escapes a transaction. Better Auth uses its restricted authentication pool. A system purpose is installed only by an authenticated worker/agent dispatcher, never synthesized because user context is missing.

Pool budget: handoff's “pool max 4” is not the whole process. Auth currently creates up to three pools. Prior production evidence reports a PgBouncer database budget of ten. Before selecting maxima, measure all API/auth/worker/legacy/gateway users and rolling-deploy overlap; the sum must fit the actual current budget. Initial API/worker limits are configuration, not a capacity claim.

Secret inventory is `inventory.json.environmentNames` (names and source references only). End-state ownership:

- API: `BETTER_AUTH_SECRET`, DB URLs/CA/RLS key, SSO client/session-sealing credentials, provider OAuth secrets, encryption keys, SMTP/relay credentials and gateway/agent credentials required by its modules.
- Worker: least-privilege worker connection and only secrets for its handlers; no migrator credentials.
- Migrator/offline provisioner: short-lived direct connection and reviewed role authority; not packaged as runtime env.
- UI: public origins/client identifiers and, only if needed for private transport, a narrowly scoped transport credential. No DB, provider, cookie-decryption or Better Auth secret. Browser cookies are opaque to SSR; API owns decrypt/refresh/authorization.
- Preserve all old encryption keys, purpose strings, AAD and ciphertext formats until migrated and tested. New Billing secrets use separate environment/purpose and key version. Rotation has old-read/new-write overlap with bounded retirement; never rotate existing Mail key during this rewrite.
- Do not log env values, cookies, OAuth codes, checkout/portal URLs, raw mail or provider payloads.

## 4. Hosting, ingress and cache decision

Plan default: separate API/worker/Dragonfly OCI services behind controlled Traefik. Prefer reuse of existing self-hosted infrastructure after resource/access/backup checks; no machine purchase. Managed Mail remains on Contabo/k3s regardless of API placement.

Choose a shared controlled ingress for public Auth/Studio host/path routing, forwarding UI pages to immutable UI origins. This preserves canonical hosts and gives one trusted-header boundary. It requires an explicit, reviewable Auth/Studio DNS cutover later. An alternative is exact frontend rewrites, but it is not accepted until header/cookie/raw-body tests prove equivalence. Never trust client-supplied `X-Forwarded-Host`; ingress strips/overwrites it, API verifies trusted peer and allowed canonical host. Never use public Auth/Studio hostname as upstream (loop).

A16 must verify Vercel origin access/protection, original host reconstruction, cookies, redirects, Server Actions, WebAuthn, streaming and cancellation through real TLS. If immutable origins cannot serve correctly behind that ingress, deploy existing UI containers there; no SPA rewrite. That decision is gated by evidence, not guessed here.

Required routing: Auth native `/api/auth/*` and discovery only on Auth host; Studio SSO/callback/MCP/provider aliases only on Studio host; API `/v1/*` and Polar webhook endpoints only on API host. API host must not serve login pages or accept auth-cookie login under a new issuer. SSR calls API with a fixed origin, allowlisted opaque cookies, original origin metadata protected by the trusted transport, and no redirects to user-provided destinations.

Dragonfly: private network only, authentication/ACL, no published 6379. Candidate data cap 256 MiB/container 512 MiB must be load-tested. Start with public catalog and non-authoritative usage views; authorize first, key by environment/customer/resource/revision, bounded TTL. Invalidate after commit. Timeout/miss falls back to DB; cache unavailable does not remove readiness. No sessions, current roles, RLS identity, billing state, locks, dedupe or queues in cache. No Better Auth secondaryStorage.

## 5. Execution method and verification harness

Each task follows: characterize old behavior → fail a focused new test → move minimal service code → pass parity/integration test → inspect exact diff → commit that task. The source file inventory provides the complete caller list; search imports before deleting a source. Do not rename every existing function during the move. Existing DTOs/error classes become shared contracts when needed, without re-exporting server implementations to UI.

A01 creates `apps/api/tests/helpers/{database,identity,http,providers}.ts` and test scripts. DB helper rejects non-loopback hosts and wrong DB names before opening connections, provisions only disposable `webdock_admin_test` and RLS test databases, and has no production fallback. Identity helper seeds two tenants, operator with/without MFA, customer admin/member, revoked session and active/expired preview. Provider helper is an injected local fake capable of recording an external success then timing out. Fake results never count as sandbox/live evidence.

Run API pure/HTTP tests with `bun test <explicit file>`, DB tests via `npm run test:integration -w @spitzli/api -- <explicit file>` (script sets the checked fixture env). Preserve existing Node suites: `npm run test:auth`, `npm run test`, `npm run test:sso`, `npm run test -w @spitzli/database-runtime`. Existing app scripts only match `tests/*.test.ts`; explicitly run nested `apps/auth/tests/databases/integration.test.ts`, PgBouncer integration and component tests too. Four historical opt-in RLS skips are not an RLS pass.

## 6. API rewrite work packages

The headings below are dependency groups. Implement the smaller reviewable slices in [slices.md](slices.md); do not turn an entire Mail/Hosting group into one commit.

### A01 — Runtime and route security proof

**Create:** `apps/api/{package.json,tsconfig.json,Dockerfile}`, `src/{app,index,config,route-manifest,route-preflight}.ts`, `src/plugins/{http-policy,request-context,errors,access}.ts`, `src/protocols/discovery.ts`, `tests/{runtime,route-preflight,http-policy}.test.ts`, helpers above. **Modify:** root `package-lock.json` only for API dependencies; no frontend runtime upgrade.

- [ ] Pin packages and verify npm tarball versus reviewed FSR commit; record runtime/artifact checksums.
- [ ] Add failing startup cases: absent root, throwing import, wrong default export, empty required manifest, duplicate final method/path, `[id]`/`[slug]` equivalence, unsupported bracket syntax, reversed ghost-controller order, hidden discovery.
- [ ] Implement preflight before listening, collision check before merge, explicit protocol mounts, compile before ready, readonly production `types:false`. Small pinned patch only if strict load/collision hooks cannot be achieved externally; retain patch in `patches/elysia-fsr-0.1.0.patch` with reproducible package check.
- [ ] Test each supported method anonymous/authorized/foreign tenant plus HEAD/OPTIONS, unknown host, spoofed forwarded headers, byte-exact body and multiple Set-Cookie headers. Confirm `/users/me` precedence over parameter route.
- [ ] `bun test apps/api/tests/runtime.test.ts apps/api/tests/route-preflight.test.ts apps/api/tests/http-policy.test.ts`; readonly container must start from arbitrary working directory and deny malformed required modules.

**Produces:** `createApp`, validated config and route manifest. No production traffic.

### A02 — Drizzle over the existing security boundary

**Source:** `packages/database-runtime/src/index.ts`, Auth `db.ts`, `database-context.ts`, `database-rls-schema.ts`, `database-{cluster,mail}-rls.ts`, all schema declarations/scripts in inventory. **Create:** DB/schema/migrator files in section 3; `tests/{database-compat,schema-parity}.test.ts`.

- [ ] Restore a protected structural snapshot into disposable PostgreSQL. Record baseline DB object list/hash; preserve prior migration history.
- [ ] Test with real non-owner business/auth/worker roles: no scope denied, forged signature denied, wrong audience denied, expired session denied, tenant A cannot read B, scope change mid-transaction denied, release-before-completion cannot late-commit, PgBouncer transaction reuse does not leak context.
- [ ] Wrap Drizzle around existing pg/ScopedPool; remove `attachDatabasePool` only in API copy. Preserve parameterized raw SQL locks/constraints where clearer than ORM translation.
- [ ] Compare schema before/after an empty migration; no drops, renames or privilege widening. Test failed migration rollback and repeat invocation.
- [ ] `npm run test:integration -w @spitzli/api -- tests/database-compat.test.ts tests/schema-parity.test.ts`; run existing database-runtime unit and PgBouncer suites.

**Produces:** compatible runtime DB access and offline migration ownership, no production schema mutation.

### A03 — Better Auth and protocol parity

**Source:** Auth `auth.ts`, `authorization.ts`, `passkeys.ts`, `cookie-policy.ts`, `lazy-auth.ts`, `tenant-policy.ts`, `offline.ts`, `bootstrap.ts`, `mail.ts`, `mcp.ts`, `account-continuation.ts`, `sign-in-recovery.ts`; native/discovery route files. **Create:** corresponding `apps/api/src/modules/auth/` files, `src/protocols/auth.ts`, `tests/{auth-protocol,auth-adapter}.test.ts`.

- [ ] Port actual Auth tests to native Request/Response, preserve plugin options and all before/after hooks. Password reset/session revocation, invitation verification, MFA enrollment session removal, disabled signup/impersonation/token endpoints must remain exact.
- [ ] First prove existing pg adapter under Bun; separately fail/pass Drizzle adapter tests against identical restored table names/default IDs and all plugin tables. Final gate requires Drizzle parity, no silent permanent fallback.
- [ ] Test existing cookie/JWKS/passkey records, issuer, RP-ID, OIDC PKCE/refresh/introspection, confidential/public clients, registration disabled, client disabled, MFA removal, live membership/site-grant revocation and Studio reader access independent of CMS availability.
- [ ] Replace Vercel IP header dependency with trusted ingress context; preserve database rate limits (10 login, 8 TOTP, 5 backup/60s). Concurrent requests cannot exceed atomically enforced counts.
- [ ] `bun test apps/api/tests/auth-protocol.test.ts apps/api/tests/auth-adapter.test.ts`; real-DB integration and browser passkey test are separate mandatory gates.

**Produces:** unchanged public identity contract. Domain services currently needed by claims are moved with their existing query semantics, not stubbed until later tasks.

### A04 — Studio portal/session and legacy bridge

**Source:** Admin `sso.ts`, `sso-routes.ts`, `studio-client.ts`, `studio-contracts.ts`, `studio-errors.ts`, `auth.ts`; Auth `studio-api.ts`; `packages/payload-sso/src/index.ts`. **Create:** API `modules/portal/{session,oidc,studio}.ts`, `protocols/legacy-bridges.ts`, `packages/api-client/{package.json,src/index.ts,src/contracts.ts}`, `tests/portal.test.ts`. **Modify:** existing SSO route transports and `studio-client.ts`.

- [ ] Preserve cookie names/sealing formats, state/nonce/PKCE, redirect allowlist, refresh and central logout. Snapshot existing package behavior without modifying customer-CMS SSO semantics.
- [ ] Make API own decrypt/introspection/refresh. SSR forwards opaque allowlisted cookies; returns every Set-Cookie on mutating transport. No SSO secret in final UI.
- [ ] Preserve `/api/studio` operation/args/data/error contract and add `/v1/studio` API alias; disallow arbitrary operation names, oversized bodies and caller-controlled actor identity.
- [ ] Until a module transfers, its selected existing backend remains its only writer. Do not mount API handlers that depend on missing modules; readiness and ownership map agree. Bridge availability follows completed domain slices.
- [ ] Test double Set-Cookie, stale refresh, DB/introspection outage (503, not signed-out success), foreign return URL, preview boundary and missing native user mapping. `bun test apps/api/tests/portal.test.ts` plus existing `studio-session-errors`, `sso-policy`, `operator-page-access` suites.

### A05 — Tenant, invitations, grants and preview

**Source:** Auth `access-management.ts`, `tenants.ts`, `tenant-policy.ts`, `tenant-schema.ts`, `tenant-owner-schema.ts`, `tenant-preview.ts`, `tenant-preview-schema.ts`; Admin same-purpose clients and tenant/people actions in inventory. **Create:** API `modules/access/` and `modules/tenants/` preserving source filenames, `tests/access.test.ts`.

- [ ] Characterize operator/admin/member/foreign-tenant rights for every dispatch operation. Test expired preview remains restrictive until explicit exit.
- [ ] Move services with signed scope, owner-protection SQL, organization/customer mapping triggers, invitation security, grants and audit. Keep actor out of browser parameters.
- [ ] Switch Auth and Studio actions for this slice to the shared transport together; leave UI translation/redirect/revalidation local.
- [ ] Test concurrent last-owner removal, revoked session, archived customer, wrong organization, mismatched site grant, preview write/live-provider denial and explicit exit. `npm run test:integration -w @spitzli/api -- tests/access.test.ts`.

### A06 — Registry and Payload hook replacement

**Source:** Admin `cms/collections.ts`, `payload.config.ts`, `lib/{registry,registry-api,actions,server,instance-users,snowflake,snowflake-schema}.ts`, migrations; Payload REST/system routes and all `getCMS` callers in inventory. **Create:** API `modules/registry/{customers,projects,instances,users,audit}.ts`, `tests/registry.test.ts`.

- [ ] Test immutable IDs, uppercase ISO country, HTTPS without credentials, unique instance/project/schema/provider ID, customer archive with active projects, project archive with any non-retired CMS, inactive owner/project rejection, forbidden owner/project reassignment, last-operator/self-role protections and once-only audit.
- [ ] Implement explicit services with current field limits and same transaction for mutation/audit. Preserve audit actor mapping from native operator record, not assumed equality to central subject.
- [ ] Replace Payload local reads/writes, including MCP and provider inventory updates, through registry services. Tenant synchronization trigger remains active.
- [ ] Enumerate `/system`-only operator capabilities from Users/Customers/Projects/Instances/Audit and reproduce needed controls in existing Studio before closing `/system`. General delete remains forbidden; dedicated lifecycle handles deletion.
- [ ] `npm run test:integration -w @spitzli/api -- tests/registry.test.ts`; compare before/after DTO pagination/sort/null relations. Freeze Payload migration/writer only at this slice's actual cutover.

### A07 — Manual plans, private offers and storage contracts

**Source:** Auth `plans.ts`, `hosting/allowances.ts`; Auth/Admin plan/offer/usage actions and forms; `docs/architecture/plans-and-storage.md`. **Create:** API `modules/plans/plans.ts`, `tests/plans.test.ts`; modify both clients.

- [ ] Copy current tests including `assign-custom`; assert no template created by individual assignment, extras retained, immutable history, tenant lock and revision conflict.
- [ ] Port manual template/offer acceptance unchanged: customer acceptance is not operator assignment, and no payment is collected. Paid acceptance will be a separate B06 operation.
- [ ] Test concurrent acceptance/revision change, expired/revoked offer, wrong tenant, preview denial, finite/unlimited quota representation and unchanged hosting allowances.
- [ ] `npm run test:integration -w @spitzli/api -- tests/plans.test.ts`; compare exact base/extras/revision/history before/after.

### A08 — Platform settings, secrets, storage and worker foundation

**Source:** Auth `platform.ts`, `storage-usage.ts`, storage cron; Admin platform/storage wrappers; role policies. **Create:** API `modules/platform/{platform,storage-usage}.ts`, `modules/secrets/codec.ts`, `jobs.ts`, `worker.ts`, `cache.ts`, `tests/{platform,jobs,cache}.test.ts`.

- [ ] Test existing ciphertext decrypts using preserved key/purpose, wrong purpose fails, responses/audit never contain plaintext.
- [ ] Port storage measurements and operator guards; partial/provider failure never writes zero as authoritative usage.
- [ ] Add durable periodic-slot job reservations/lease token/CAS, bounded deadline/shutdown and per-purpose RLS. Reuse existing domain queues; do not migrate mail messages into a generic queue.
- [ ] Test simultaneous claims, crash after claim, lease replacement then late completion, replay and shutdown. Cache down/empty/wrong tenant cannot alter rights or usage authority.
- [ ] `bun test apps/api/tests/platform.test.ts apps/api/tests/jobs.test.ts apps/api/tests/cache.test.ts` plus real-DB claim tests. Storage cron ownership transfers only in A16.

### A09 — Complete mail control/product/API migration

**Source:** every Auth `mail*.ts`, `tenant-mail.ts`, `turbo*.ts`, `mail-product/*`; Admin mail clients/actions, `packages/mail-core`, `packages/mail-contracts`, `infra/mail/cronjobs.yaml`. **Create:** API `modules/mail/` preserving filenames/subdirectory, `protocols/{mail,provider-webhooks,mail-docs}.ts`, `tests/mail.test.ts`; port every existing `mail-*.test.ts`, `tenant-mail.test.ts`, `turbo*.test.ts` to API test equivalents.

- [ ] Freeze `mailOperations` method/path/permission/error contract, all native gateway/mailbox/webmail actions, provider callbacks and OpenAPI. Key/domain isolation and all operation definitions are tested by enumeration, not a manually assumed count.
- [ ] Preserve idempotency, quota reservation/recovery, If-Match, scheduled/batch state, callback inbox, suppression, webhook signing/SSRF protection/replay, encrypted content/retention, native submission and ambiguous-send `outcome_unknown` semantics.
- [ ] Worker calls existing `runMailJob(job, deadline)` under authenticated `mail-cron` purpose. Remove dependency on VERCEL_ENV from the new worker path without opening the legacy cron publicly. Only one scheduler owns dispatch/events/webhooks/maintenance.
- [ ] Test provider accepted then timeout → no resend, signature mutation, chunked size limit, concurrent quota, suspended connection, late callback, key revoked, tenant preview, DNS verification and Stalwart suspend/resume retained data. No real delivery in fixture tests.
- [ ] `npm run test:integration -w @spitzli/api -- tests/mail.test.ts`; ported Mail suites and existing contract tests must pass. Incoming live/private deployment IDs remain untouched.

### A10 — Hosting and git build control

**Source:** Auth `hosting/*` excluding database-specific files handled A11; Admin hosting/git clients/actions; `packages/hosting-contracts`; `apps/hosting-agent`, `apps/build-worker` are consumers. **Create:** API `modules/hosting/` preserving filenames, `protocols/agents.ts`, `tests/hosting.test.ts`.

- [ ] Enumerate `commandSchema.options`, `readActions`, git/OAuth schemas and all actual API/agent/build-worker subpaths. Preserve command DTOs and tool mapping.
- [ ] Move actor checks, reservations, revision locks, operations, environment encryption/redaction, cluster enrollment, BYOK, git source policy, artifact publication/reconciliation and callback dedupe together with dependent SQL.
- [ ] Preserve authenticated system-purpose/cluster context and EU location admission. Do not rewrite Python executors or change their stored endpoint URLs prematurely.
- [ ] Test duplicate operation, revoked agent, stale revision, capacity race, build completion after lease lost, log cancellation/backpressure, app deletion retaining storage, explicit purge and protected control-plane resources.
- [ ] `npm run test:integration -w @spitzli/api -- tests/hosting.test.ts`; run existing hosting/git/deployment contract suites. No infrastructure mutation against real customers for parity tests.

### A11 — Managed databases and native gateways

**Source:** Auth `databases/{http,schema,service}.ts`, `hosting/database-*.ts`, `hosting/databases.ts`, database RLS modules; Admin database clients/actions; `packages/database-{contracts,runtime}`, `apps/database-gateway`. **Create:** API `modules/databases/` and database-specific hosting files, `tests/databases.test.ts`.

- [ ] Preserve billing/order reservation, credentials display, maintenance/migration, observed resource state, gateway tickets and schema access boundaries; platform DB self-maintenance protection is mandatory.
- [ ] Keep gateway and customer data plane separate; credential display remains a current-authorized one-time response, never cache/log.
- [ ] Test real non-owner RLS, other customer/cluster denial, source migration retained volume, deletion/purge separation, gateway expiry/revocation, TLS wrong host and CA failure, archive/platform SQL isolation.
- [ ] `npm run test:integration -w @spitzli/api -- tests/databases.test.ts`; include current nested databases integration and opt-in RLS suites explicitly.

### A12 — Provider OAuth and publication

**Source:** Admin `github*.ts`, `vercel*.ts`, `git-connection-routes.ts`; Auth `hosting/git-oauth.ts`, `git-github.ts`, `git-vercel*.ts`, `byok-vercel.ts`; exact callback/event files in route manifest. **Create:** API `modules/integrations/{github,vercel}/`, `tests/integrations.test.ts`.

- [ ] Preserve original callback URLs, encrypted flow state, cookie/nonce/PKCE as currently used, team/project ownership and region policy.
- [ ] API loads Vercel runtime credentials itself; remove confidential `vercelRuntime` field only once Studio and hosting bridge use the new contract. Preserve old in-flight OAuth flows through expiry/rollback.
- [ ] Test wrong nonce/state, wrong user/customer/team, callback replay, expired flow, provider success then connection timeout, duplicate events, unpublished artifact and pagination truncation.
- [ ] `bun test apps/api/tests/integrations.test.ts` plus current GitHub/Vercel tests; use sandbox/local provider fixtures, not live provider writes.

### A13 — Customer CMS, Canvas, Shop and Calendar bridges

**Source:** Admin `canvas-bridge.ts`, `demo-bridges.ts`, `demo-bridge-policy.ts`, `studio-shop.ts`, content helpers; all `sites/[bindingID]/*/actions.ts`; `packages/cms-ui`, `page-builder`, `lead-bookings`, `instance-kit`. **Create:** API `modules/cms-bridge/`, `tests/cms-bridge.test.ts`.

- [ ] Classify shared packages' server/client exports; retain rendering/editing UI locally but move token exchange, CMS requests and permission decisions into API. No blanket removal of packages used by customer CMS.
- [ ] Preserve current CMS claims, target binding/origin allowlists, upload/body constraints, content validation, shop/calendar/request mutation rights and error semantics.
- [ ] Test forged binding/origin, reader write, role revoked in existing session, response/body limit, image/content escaping and edit conflict. Token must not appear in browser payload.
- [ ] `bun test apps/api/tests/cms-bridge.test.ts`; existing CMS UI/bridge integration plus representative canvas save, shop and calendar operations against isolated CMS.

### A14 — MCP, registry HTTP and destructive lifecycle

**Source:** Admin `mcp-{auth,server}.ts`, `hosting-mcp.ts`, `registry-api.ts`, `project-deletion*.ts`; Auth `mcp.ts`, `lifecycle.ts`; old MCP/introspection/discovery routes. **Create:** API `modules/{mcp,lifecycle}/`, `protocols/mcp.ts`, `tests/mcp-lifecycle.test.ts`.

- [ ] Preserve resource audience/discovery, bearer challenge, current session introspection and separate registry/hosting tool sets. Generate expected tools from actual schemas; do not invent `projects.relocate` if absent in this baseline.
- [ ] Move registry transports to A06 services. Keep preview hash/exact name/revision, isolated target validation and resumable deletion ledger; financial records restrict deletion after B01.
- [ ] Test read token cannot discover/execute write tool, revoked token denied, cross-tenant target denied, malformed input, stale deletion plan, partial provider deletion retry, protected platform DB/host and retained storage.
- [ ] `bun test apps/api/tests/mcp-lifecycle.test.ts`; real external MCP client against test ingress, no production deletion.

### A15 — Finish UI transport conversion and dependency closure

**Modify:** every server-action file and every server-only caller listed in inventory, Admin `server.ts`/`payload.config.ts`/`next.config.ts`/package manifest, Auth package manifest and direct auth/query page imports. **Create:** `apps/api/tests/frontend-boundary.test.ts`, `packages/api-client/src/contracts.test.ts`.

- [ ] Replace remaining server page/layout DB/auth/provider calls with API reads; leave React request memoization and UI validation/translation/redirect/revalidate local.
- [ ] Verify all 523 inventoried source files are assigned either UI/shared or a completed API task; every operation from studio shapes, Mail contract, hosting schema, MCP and inline action has a caller/guard/test/cutover owner.
- [ ] Resolve source imports transitively, including workspace exports and dynamic imports. Fail boundary test for reachable pg/Drizzle/Payload Local API/Better Auth server/provider credentials from either UI server entrypoint. Allow browser Better Auth client and existing customer-CMS packages in their own apps.
- [ ] Remove `/system`/Payload wildcard handlers and `withPayload` only after A06 capabilities pass; retain explicit SSO/refresh/logout aliases in ingress. Unknown former CRUD paths return 410/404, never fallback to a different tenant handler.
- [ ] Run `npm run check -w @spitzli/auth`, `npm run check -w @spitzli/admin`, both production builds, changed-file lint, all ported suites and client contract tests. Remove actual unused dependencies/secrets after traffic transfer, not before rollback image exists.

### A16 — Operational rehearsal and cutover

**Create:** `infra/api/{compose.yaml,traefik-routes.yaml,env.example}`, `docs/operations/api-cutover.md`, `apps/api/tests/ingress.test.ts`; if approved host is existing Kubernetes, use its deployment/service/network-policy manifests instead of a second compose production control plane. **Modify:** Auth cron configuration only when scheduler transfer is verified.

- [ ] Verify target host access, EU location, CPU/RAM/disk headroom, isolated networking, TLS, current backups/restore, immutable UI origin access and full DB pool budget. Readiness is no substitute for these checks.
- [ ] Build immutable API/worker image, read-only non-root filesystem, dropped capabilities, no Docker socket. `/healthz` minimal; `/readyz` internal DB/config/route verification; cache degradation does not hide otherwise healthy API.
- [ ] Rehearse real TLS ingress: OIDC, MFA/passkey, multiple cookies, CSRF/origin, tampered forwarded headers, raw webhook, MCP and log stream. No Host rewrite to a new issuer.
- [ ] Execute section 8 per ownership unit, record exact old/new image, route version, DB migration version, queue cursor/leases and smoke results.
- [ ] Drain on SIGTERM, recover expired leases with CAS; ambiguous external writes remain for reconciliation. Test DB/cache/API/worker failure independently.
- [ ] Only after observation and rollback rehearsal remove unused UI secrets/backend deps and old cron. Keep logs/metrics for request latency/errors, pool waits, oldest job, callback lag, provider conflicts and cache degradation. External notifications need separately configured channel.

## 7. Polar work packages and acceptance

Execute B01–B09 from [archived detailed Polar tasks](sources/03-polar-implementation-plan.md), with the concrete paths below and current RLS corrections. Do not create live products/prices/subscriptions while implementing the code. Each package follows the fail/pass/test/commit cycle in section 5. API/worker is its only backend.

| ID / dependencies | Exact files to create/extend | Required regression and result |
|---|---|---|
| B01 schema; A02/A05/A07 | `src/db/schema/billing.ts`, `drizzle/0001_billing.sql`, `src/db/rls.sql`, `src/modules/billing/types.ts`, `tests/billing-schema.test.ts`, `tests/helpers/billing.ts` | Tables in spec §7; environment-scoped unique/FKs, integer money, immutable version/contract, one active website base per customer. Real non-owner sandbox cannot mutate live plans. Billing prevents cascading customer deletion. |
| B02 connection; B01/A08 | `src/modules/billing/{connection,polar}.ts`, `src/routes/operator/billing/connection.ts`, `tests/billing-connection.test.ts`; Studio `app/(console)/(portal)/admin/billing/{page.tsx,form.tsx,actions.ts}` | Operator+MFA required; preview/member denied. Organisation lock, encrypted env-specific secrets, fixed provider origins, masked output, rotation and separate sales/publish/enforcement switches. |
| B03 catalog; B02 | `src/modules/billing/catalog.ts`, `src/routes/operator/billing/products.ts`, `tests/billing-catalog.test.ts`; Studio `admin/products/{page.tsx,form.tsx,actions.ts}` | Publish reserve→external call→readback. Timeout after successful create yields needs_review, not second product. Immutable price/version; private offers not templates; no automatic live seed. |
| B04 webhook; B01/A08 | `src/modules/billing/webhooks.ts`, `src/protocols/webhooks/polar.ts`, `tests/billing-webhooks.test.ts` | Exact raw bytes, versioned SDK, live/sandbox secret isolation, 1 MiB streaming bound, event+job single commit; 202 only after commit, duplicate 200, invalid signature 403, format 400, oversize 413, DB unavailable 503. ID/hash conflict quarantined, never overwritten. |
| B05 reconcile; B02/B04 | `src/modules/billing/reconcile.ts`, `src/jobs.ts`, `src/worker.ts`, `tests/billing-reconcile.test.ts` | Current provider object reads, bound organization/customer/product, full pagination, stale-read generation rejection, 60s lease+claim token CAS. Events trigger reads, never ordered grants. |
| B06 checkout; B03/B05/A07 | `src/modules/billing/{offers,checkout}.ts`, `src/routes/tenant/billing/checkout.ts`, `tests/billing-checkout.test.ts`; existing Studio `offers/[token]/{page.tsx,form.tsx,actions.ts}` | Explicit immutable acceptance; ready website and single base contract; double click same attempt, fixed price/customer/return URL, no trial/discount. Timeout needs_review. Payment after withdrawal/expiry remains tracked; success URL grants nothing. |
| B07 projection; B05/B06 | `src/modules/billing/entitlements.ts`, `src/modules/auth/authorization.ts`, `src/modules/plans/plans.ts`, `tests/billing-entitlements.test.ts` | Active matching subscription + paid initial order; absolute base under existing lock, extras preserved, revision only on change. Repeated events no inflation. Manual plan not reactivated after cancellation. Sandbox cannot alter claims/live quotas. |
| B08 portal/change; B07 | `src/modules/billing/{portal,subscriptions}.ts`, `src/routes/tenant/billing/{portal,subscriptions}.ts`, `tests/billing-subscriptions.test.ts`; Studio `tenants/[customerID]/billing/{page.tsx,form.tsx,actions.ts}` | Current customer admin only, stored Polar ID; preview denied; portal URLs not cached/logged. Change existing subscription, never second checkout. Invoice upgrade and next-period downgrade; payment failure visible, overquota data retained. |
| B09 operational acceptance; B08/A16 | `tests/billing-e2e.test.ts`, `docs/operations/polar.md`; factual `docs/verification/<actual-date>-polar-billing.md` only after execution | Complete sandbox flow through real TLS and worker crash/restart; live read-only connection then explicit catalog/pilot release. Preserve inbox/worker/termination processing with sales disabled. |

Paths in this table beginning `src/`, `tests/`, `drizzle/` are under `apps/api/`. Studio relative paths are under `apps/admin/src/app/(console)/(portal)/` unless written fully. Each test command: `bun test apps/api/tests/billing-<name>.test.ts`; schema/reconciliation/projection/concurrency also run through isolated DB integration script. Sandbox browser run is additional, not replaced by mocks.

Worker defaults from spec: poll 5s, batch 25, at most two external calls, 60s leases; 429 respects Retry-After; read retries 1/5/15/60 min, escalate after eight failed attempts. Reconcile pending/past_due every 15 min, active at most 6h, catalog daily, unique schedule slots. Stale or partial lists never imply cancellation. Before time-based suspension fetch current state; provider failure warns without new suspension. Audit actor `system:polar:<env>` is a textual audit identity, not itself a valid DB authority: introduce explicit customer-scoped billing worker policies, separately tested for live and sandbox.

Status rules: preserve last confirmed service through outages; cancellation-at-period-end remains active until effective end. Definitive end/unpaid/paused blocks purchased CMS access only, keeps Studio/payment portal and public website/data. Partial refund does not automatically end subscription. Unexpected trial or unmatched direct purchase goes to review. Seven-day grace is a proposed product policy requiring confirmation before enabling enforcement, not a payment fact. Public Mini/Basic/Plus prices in source spec remain proposals; no sales approval inferred from this plan. Human services remain separately billed; further Mail/DB/Apps/Connect families require individual operational/commercial acceptance.

Retention proposal: processed raw webhook 30d/open failure 90d, retain dedupe hash/ID; financial retention policy is separate. Encrypted raw bodies and checkout URLs never enter frontend/logs. Unknown signed event versions may be quarantined only after real signature verification. SDK API version, not remembered examples, determines request types.

## 8. Single-writer cutover and rollback runbook

Ownership units: identity + portal; registry; tenants/access; manual plans/platform/storage; Mail; hosting/git; managed databases; provider integrations; CMS bridges; MCP/lifecycle. Dependencies mean shared tables/policies may require an atomic group: registry and tenant triggers must be compatible before either transfers, identity callbacks and portal must switch coherently. An unported operation stays entirely on its old writer; no silent API fallback to a second writer.

For each unit:

1. Record old/new artifact digests, public aliases, expected DB schema, DB principals and background dispatcher. Make a tested recovery point; restore verified into an isolated target. Do not copy customer secrets into the report.
2. Deploy compatible new code with ownership disabled. Read-only contract probes only; never replay real mutating traffic for comparison.
3. Block new mutations for the unit at ingress and internal dispatch. Drain old requests/jobs; ensure no old process can commit later. Record active leases and unresolved provider operations. Keep authenticated webhook inbox receipt available; if no durable receiver exists, return retryable failure rather than false success.
4. Apply additive migration if necessary, verify catalog fingerprint and role grants. Flip routing and explicit writer ownership together; start exactly one scheduler for that unit. Reuse existing leases/idempotency records.
5. Run operator/customer/member/foreign-tenant/revoked/preview probes and one authorized fixture mutation on an isolated test tenant only when authorized for rollout. Compare records/audit/revisions and execute protocol smoke tests.
6. Observe errors, latency, pool pressure, queue lag and uncertain writes. Evidence records timestamp and result. Failed authorization, schema mismatch, lost cookie/signature, unexpected second writer or data mismatch blocks release immediately.
7. Reopen writes only when checks pass. Old service remains unable to write. Remove old code/credentials after rollback compatibility is demonstrated; preserve an artifact/config manifest, not broad live credentials indefinitely.

Rollback: stop new writes/claims in new owner, drain or mark uncertain operations, retain committed DB/inbox/history. Restore prior compatible image/routing and ownership; keep new additive schema. Never reverse SQL by dropping data or unfence the retired pre-migration platform DB. If old artifact cannot read new state, hold writes and forward-fix; do not claim safe rollback. For Polar disable sales/publication, keep receipt/reconcile/cancellation operational. Both old and new workers cannot own the same provider operation. Restore-backup is disaster recovery with reconciliation of external effects, not the normal app rollback.

Order: A01–A03 prerequisites; A04/A05/A06 coordinated identity/registry/access parity; A07–A08; A09; A10–A11; A12–A14; A15/A16 final closure. Each deployed slice gets the same cutover checks. Polar B01 can begin after A02, but B06 live traffic requires completed identity/access/plan/worker cutovers; B09 live pilot only after A16 ingress acceptance. Never remove Auth/Studio backend access while inventory still contains an active caller.

## 9. Evidence gates and handoff

Planning is complete when inventory has no unassigned route/action, current-baseline differences are explicit, all tasks have files/tests/ownership and open production evidence is named. This is not a declaration that compatibility or production rollout tests have passed.

Before first implementation test: use Node 24, install with locked npm in this worktree, create isolated fixture env and databases, execute baseline suites. No existing test env was present here. Before code edits, read the installed Next guides and any module-local instructions. For remaining Vercel UI inventory/operations the session lacks Vercel CLI; installing `npm i -g vercel` is recommended, but was not performed for planning. GitHub deployment API supplied read-only release evidence.

Before deployment: verify target host resources/access/location, current DB structure/roles/pool budget, protected origin routing and TLS, backups/restore and domain alias→artifact mapping. These are evidence gates that an implementer can investigate, not unmade product architecture decisions. Owner approval for this turn covers the plan only.

Before Polar sales: actual organization/scopes/product eligibility, final catalog amounts/tax/terms, grace/enforcement and financial retention decisions, sandbox purchase/renewal and live pilot acceptance. No live financial/infrastructure change follows merely from this document.

## 10. Sources and alternative considered

- Handoff snapshot: [Gist](https://gist.github.com/NewtTheWolf/9253a1f1b9fda010e884bf0082c10572), revision `38e0bb1412476b1448edcb0da3a1379552e20dbc`, downloaded 2026-10-13. Five markdown files copied verbatim under sources; optional FSR probe not executed here.
- [Elysia scopes](https://elysiajs.com/essential/plugin): hooks are scoped; static composition needs executable denial tests.
- [Better Auth Drizzle adapter](https://better-auth.com/docs/adapters/drizzle): adapter schema mapping must match existing tables; documentation is not proof of this application's plugin compatibility.
- [Polar delivery](https://polar.sh/docs/integrate/webhooks/delivery): durable short request handling and retries inform B04; exact dated product rules are preserved in source spec.
- [Zitadel migration](https://zitadel.com/docs/guides/migrate/introduction), [user/credential migration](https://zitadel.com/docs/guides/migrate/users): independent identity service is plausible for future enterprise federation, but existing client credentials/IDs cannot simply be imported. Webdock's SQL session/user joins and authorization would still need migration. Owner agreed to retain Better Auth; no Zitadel deployment or identity migration in scope.
