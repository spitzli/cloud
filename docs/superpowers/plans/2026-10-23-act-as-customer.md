# Act as customer implementation plan

> **For agentic workers:** Execute inline using superpowers:executing-plans; parent owns review and final gate. Never commit or push this shared checkout.

**Goal:** Let a live platform operator explicitly act as the selected customer’s owner, keeping the existing read-only preview default.

**Architecture:** Store `mode = read_only | owner` on the existing exact-session scope. Retain the real operator identity and preview marker; SQL independently requires a live native operator, active customer, matching organization, local authority and unexpired scope. Owner operations use ordinary customer permissions and plan limits; operator APIs, account changes, API tokens, delegated OAuth/MCP stay blocked; first-party launches bind customer, mode and fresh scope epoch.

**Tech stack:** Rust/SeaORM/Postgres RLS; React/Vite; Bun. No new dependencies.

**Spec:** Delegated task prompt in this thread, 2026-10-23.

## Global constraints

Preserve TODO.md and both untracked handovers. Never read demo-owner.json or change operator credentials. Only disposable restricted fixture databases; no real providers, real mail or production actions. Parent owns the full quality gate. Persistence migration 0038 is access mode; 0039 is identity helper. Do not edit generic workload files.

## Review focus

- Expired/revoked scope and changed root role must fail closed, including a previously captured principal.
- A selected customer never admits a foreign customer/project or restores platform powers.
- Original operator remains the audit actor; no customer credentials or synthetic membership.
- Browser authority cannot retain owner permission after exit or carry it into OAuth/MCP/API tokens.
- Owner operations retain normal plan limits and native local-authority restrictions.

### 1. Scope, identity and RLS

- [x] Add a restricted-fixture regression for read-only default, explicit owner create/profile/domain/member permissions, foreign customer denial, plan limit, audit actor, expiry/revocation/root changes and delegation denial; run it red.
- [x] Add access migration 0007 / persistence0038 with constrained mode, customer-owner policies and exact-scope helper; add identity0005 / persistence0039 with restricted boolean owner-mode helper. Preserve existing default and policy grants.
- [x] Extend Principal with explicit owner-mode/read-only methods while retaining `in_tenant_preview()` and suppressed `is_operator()`.
- [x] Run focused access/identity fixture tests green.

### 2. First-party and service/plugin authority

- [x] Extend browser-session regression to prove first-party owner ceiling and public OAuth denial; run red.
- [x] Change only read-only guards at workspace owner boundaries. Preserve account, token, delegated browser/OAuth/MCP and global operator guards. Recheck SQL customer permissions on every admission/continuation.
- [x] Update plugin-host authority signing to use project DB customer_id for required InvocationClaims.customer_id, coordinating protocol construction with parent.
- [x] Run focused browser-session/plugin-host/service tests green.

### 3. Console

- [x] Add explicit preview/acting contract and rendering regressions; run red.
- [x] Add customer record start controls, persistent target/mode banner and exit; show ordinary workspace owner controls in owner mode while keeping account/operator/delegation restrictions.
- [x] Run Console focused tests and typecheck; report exact test commands/results and any remaining limitations to parent.


## Execution evidence and limits

Product source frozen for parent integration review. No commits, resets, pushes, production/provider operations, real mail, or new dependencies.

- Persistence0038 (`access/0007_customer_acting.sql`) adds explicit mode, immutable random scope epoch, customer-only owner policy and exact first-party launch proof. Persistence0039 (`identity/0005_customer_acting.sql`) exposes restricted boolean/fingerprint helpers, never tenant rows to authentication.
- Red-green: Access fixture first failed on missing mode column; browser fixture failed on owner read-only ceiling; Webmail fixture failed on explicit scoped operator mailbox grant; Console contract failed when owner state was stripped. Final focused checks below pass.
- Restricted native PostgreSQL fixtures: Access acting1/1; browser-session persistence1/1 (including public OAuth inactive while acting); plugin owner admission1/1; databases persistence1/1; Mail Webmail1/1. Database and Webmail launch/exchange/inspect work under bound customer authority, then fail after exit and same-customer re-entry. No provider network or real mail used.
- Console focused API/cache/hosting/mail state tests39/39; typecheck and scoped ESLint pass. Existing read-only preview tests remain green. `git diff --check` passes.
- Real browser account acceptance passed with native password setup, TOTP, recovery, virtual passkey, consent and PKCE. `acting-browser.mjs` then passed same-user/session owner entry from the operator customer record, project create, foreign404/global403, explicit read-only controls/write403 and operator restoration on exit.
- Synthetic native UI screenshots were inspected at `.runtime-test/acting/owner-375.png` and `owner-1280.png`.
- Scope keeps global operator/account-security and public OAuth/MCP rights unavailable. API-token creation is explicitly unavailable in owner mode and disclosed in the banner. Normal mailbox content still requires an explicit grant; acting is never a customer credential or synthetic membership. Mail lifecycle and configuration-retry actions that are operator-only for ordinary customers remain operator-only. Plan limits remain active.
- Native Mail directory readiness remains `desired_enabled && state=ready && revision=applied_revision`; acting owner management never overrides readiness. The parallel Mail plugin implementer owns engine suspension observation evidence.

## Private acceptance fixture custody

Current private configuration is `/tmp/webdock-acting-fixture.json` (0600), with `{name,env}`; never print values. `/tmp/webdock-acting-run.ts` invokes a command with those variables. Current DB was freshly initialized by the native offline bootstrap and has successful journal entries through0040. Independent account data is `.runtime-test/account-browser-acting-20261023.json` (0600), never `.dev-local` accounts.

API3130 and Vite3121/3125 were launched through `/tmp/webdock-acting-start.ts`; owned PIDs are in `/tmp/webdock-acting-processes.json` (0600). The implementation agent owns eventual cleanup of labelled `webdock-acting-fixture-*` containers, owned processes, private temporary roles/config files and its unique account fixture only. They remain running at the parent's explicit request until the integrated final gate is complete. Parent owns broad tests, independent review and final formatting.

## Invitation dispatch correction after independent review

The review found that the invitation outbox still rejected every preview principal and did not retain its admitted customer scope. This allowed a previously scoped job to be considered with restored global operator authority after exit.

- `access/0008_invitation_scope.sql`, registered as persistence **0041**, adds `admitted_customer_scope` and `admission_bound`. New admission derives the fingerprint from signed SQL, binds the exact actor session, and protects admission fields against runtime changes. No accepted0038/0039 bytes changed.
- Dispatch reloads the native principal, requires the exact admitted scope (including target/mode/epoch), and checks current signed SQL organization/customer management permission. It retains session and preview-row locks through the delivery call. Restored operator authority cannot substitute for a job's former customer scope.
- **Upgrade impact, explicitly approved by parent:** pre-migration jobs default to `admission_bound=false`. Queued unbound jobs are canceled when dispatched; administrators must explicitly resend them. Previously delivered jobs are unaffected. Newly admitted ordinary owner/operator invitations keep their existing behavior. Deployment must rebuild the API and account-delivery worker with0041 before resuming dispatch.
- Red-green restricted regression: the original implementation failed `live-acting: delivered` (0 instead of1). The corrected test passes13 scenarios: normal owner/operator and live acting delivery; read-only admission refused; exit, expiry, demotion, session revocation, same-customer re-entry, local organization authority loss, customer archival, ordinary membership loss and legacy unbound work prevent delivery. It checks original actor/session attribution, persisted fingerprints and denial of runtime attempts to erase a scoped admission. Activated fixture recipients and an in-memory Delivery sink only; no real mail.
- `bun --no-env-file /tmp/webdock-acting-run.ts cargo test -p webdock-access --lib --test contracts --test acting_invitations -- --include-ignored`: **4 tests passed**, including the restricted database regression (3.53s). The regression creates and drops its own uniquely named database on the guarded disposable server, leaving the shared browser DB untouched.
- `cargo clippy -p webdock-access --all-targets -- -D warnings`: passed. Scoped rustfmt and `git diff --check` passed.
- The shared browser fixture remains at its previously applied journal until the parent upgrades it. API/Vite processes and shared private fixture data remain running and unchanged. Parent owns0041 journal application, API/worker rebuild and the final integrated gate.

Final integration: root journal-upgraded only the owned loopback browser fixture through0041,
rebuilt/restarted its API (including the embedded account-delivery loop), reran the real acting
browser and the13-scenario invitation regression, and passed the complete `bun run quality` gate.
The independent review's queued-invitation finding is addressed; ordinary owner/operator jobs
remain verified. API-token management and public OAuth/MCP delegation are intentionally unavailable
inside acting mode; return to operator mode for those operations. No production migration or mail.
