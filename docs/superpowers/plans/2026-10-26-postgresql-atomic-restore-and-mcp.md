# PostgreSQL Atomic Restore and MCP Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Existing ownership and phase gates below are authoritative; no commits or production rollout.

**Goal:** Admit an immutable, explicitly paid-reviewed fresh-target restore atomically, and expose bounded native database management through real OAuth/MCP authority.

**Architecture:** Reuse immutable database offers/orders, existing hosting operations and database jobs. Keep Browser persistence compatible and store only a database-owned typed MCP reference for bearer work. Use narrow worker-only restore functions and existing session/epoch fences; no additional identity, queue or billing platform.

**Tech Stack:** Rust/SeaORM/PostgreSQL, existing OAuth/BrowserSessions and native database agent, existing Console React/Vite contracts.

**Spec:** `docs/handovers/2026-10-25-s3-managed-storage.md`, accepted Part B audit sent to Root on 2026-10-26 (44-source aggregate `1691a08e86398e8d7138907dcf71a4ecb51588f716150b2feb372c2ee6d68fe5`).

## Global Constraints

- No commits, push, production activation, old-store migration/deletion, fabricated sessions/Ready/allowances, provider secrets in tools, or shared protocol rewrites.
- Migrations 0046–0053 and 0055 are frozen. 0054 remains owned by the recovery implementer. Reserve 0056 for the three functions below; do not materialize it before epoch coordination and final 0054 input.
- Initial dependency baseline: Root Cargo.lock `c6f2848cdeeeb1f63ebf38bf9c8faba12cbb9247b336c1b999427e643901a576`, 804 registry packages. Add only the local databases → oauth edge; preserve every registry version/checksum. Archives owner updates its standalone lock; runtime standalone is unaffected.
- Public and operator Console Restore remain disabled until atomic admission and genuine qualification pass. Existing immutable prices/offers, separate recurrence/WAL/retention and false/null recovery commitments remain unchanged.
- Original nongrant authority is never silently replaced by a same-offer retry. Only explicit current-operator reconciliation can bind a new authority/generation to the same owned target/intent/checkpoint.
- Runtime/private source material stays encrypted or ephemeral; no provider writer credentials or plaintext in public results, jobs, logs or MCP tools.

## Review Focus

- Same-offer retries with another key/actor must reuse one order/job without renewing original authority.
- Scope changes between discovery and commit must serialize with the actual native session, including another customer's preview.
- Token expiry/revocation during long work must prevent renewal/promotion and preserve uncertain target reservations.
- Epoch contention must fail/restart without an auth/worker lock cycle or two retained epoch locks.
- Malformed private MCP references must fail closed without falling back to legacy Browser/LocalSession authority.

## Ownership and execution gates

Root owns dependency resolution and OAuth private access-row locking/expiry API. PG owner owns pure contracts and database-domain implementation. Main owns recovery54/Agent source fences and reports shared lock constraints. Shared Control Plane/AgentControl wiring needs a narrow file notice first. All current native PG qualification stays held until Root's explicit phase release. Pure/unlinked tests can proceed after dependency graph stability.

### Task 1: Local dependency edge — Root and Archives owner

**Files:** `crates/databases/Cargo.toml`, Root `Cargo.lock`, Archives `Cargo.lock`; runtime lock unchanged.

**Interface:** `webdock-oauth = { path = "../oauth" }`, no features or registry additions.

- [x] Snapshot Root/Archives/runtime registry tuples and manifest hashes.
- [x] Add the single path declaration; run Root `cargo metadata --offline --all-features --format-version 1`.
- [x] Assert exact registry tuple equality and only intended local package dependency changes; verify `--offline --locked` metadata.
- [x] Archives owner resolves its lock offline and proves all 620 registry pins unchanged; runtime 400-pin lock remains byte-identical.
- [x] Announce graph stable to Main/PG before new shared builds/pins.

### Task 2: Canonical MCP reference locking and expiry — Root

**Files:** `crates/oauth/src/service.rs`, `crates/oauth/src/mcp_authority.rs`, focused existing OAuth tests.

**Interfaces:** Keep public authentication/revalidation signatures and serialized `McpAuthority` unchanged. Add a typed, non-secret `McpAuthority::expires_at() -> i64` Unix deadline; callers use it only after verification.

- [ ] Test discovery/reload ownership and expiry behavior before modification, using existing ordinary tests/component facilities; no prohibited Host race fixture or interception.
- [ ] Reuse one private access-record lock/reload implementation: discover without access lock, lock canonical issuer client → native user/session → exact access row, reload/check identity and reference hash, then existing scope/resource/preview validation.
- [ ] Use the same implementation in current `inspect_scoped` and MCP reference authorize/revalidate; preserve legacy JWT/refresh behavior and do not require raw token for stored-reference validation.
- [ ] Verify ordinary OAuth/Browser and scoped caller tests, lock order and error classification; report actual coverage, then independent review.

### Task 3: Pure immutable restore intent and authority wire — PG owner

**Files:** new `crates/databases/src/{restore_intent.rs,restore-order.schema.json,database_authority.rs}` and finite tests `{restore_intent,restore_order_contract,database_authority}.rs`, initially path-included only.

**Interfaces:** Database-owned Browser/Mcp(Box<actual McpAuthority>) wrapper; legacy Browser/null wire unchanged. Prepare produces an immutable restore-purpose offer and review hash; accept consumes exact offer/hash/full reviewed price and literal true confirmation.

- [ ] RED tests bind source UID/database/backend/policy revisions, exact backup point/time/timeline/historical credential/hash/key/destination and target plan/name/price/offer/expiry.
- [ ] Implement canonical Rust hashing and strict command/receipt inputs; authority cannot be supplied through ManagedCommand.
- [ ] Prove ordinary/upgrade purpose separation, changed review fields rejection and malformed MCP no-fallback behavior.
- [ ] GREEN finite pure tests after Root graph-stable notice; no public routing or native fixtures in this stage.

### Task 4: Coordinated epoch/session admission and migration 0056 — PG owner after Root release

**Files:** new `0056` migration; `service/{control,agent,maintenance}.rs` and agent delivery/completion as required. No changes to 0054 by PG.

**Proposed SQL interfaces:**

- `webdock_rls.database_restore_epoch(expected_epoch uuid DEFAULT NULL) RETURNS uuid`
- `webdock_rls.database_restore_source(offer_id text, expected_intent_sha256 text) RETURNS jsonb`
- `webdock_rls.record_database_restore_admission(operation_id text) RETURNS void`

All VOLATILE SECURITY DEFINER, owner platform owner, `search_path=pg_catalog`, PUBLIC revoked, EXECUTE only worker runtime. No new tables/columns/general policies.

- [ ] Agree the actual partial order with Main before writing SQL/claim wiring. Main currently begins worker agent SHARE, operation UPDATE, source policy/operation SHARE, then epoch UPDATE and Mail installation SHARE.
- [ ] PG's one worker transaction obtains active/matching epoch SHARE NOWAIT before installation/customer/cluster/operation locks. Failure rolls back/releases that transaction before taking auth/business locks; never wait/retry with old guards or duplicate epoch SHARE on another connection. Do not lock Main recovery operation/policy or upgrade shared agent while holding it.
- [ ] Discover/release candidates, acquire business `native-session-context:{actual session ID}` lock, fresh retained authority, mapped role/member/customer/project rows, then worker cluster/operation reread/CAS. Do not add a late session fence while holding worker operation locks.
- [ ] Source helper verifies signed database-order scope, immutable restore offer/hash and exact source bindings; return a bounded explicit safe spec plus exact encrypted historical role credentials only.
- [ ] Audit helper verifies the newly queued, unissued direct-Restore job/target/order/bindings and writes only fixed admission audit. No arbitrary subject/action/details or success acknowledgement.
- [ ] Actual owned PG tests cover unauthorized function calls, mismatch/replay, source and target lock contention and controlled Main fencing overlap. No assertion of global deadlock freedom from arithmetic/unit tests.

### Task 5: Atomic fresh-target allocation — PG owner

**Files:** new `service/restore_orders.rs`; existing `orders.rs`, `backups.rs`, `management.rs`, `control.rs`, `managed-command.schema.json`.

**Interfaces:** `prepare_restore_order` and `accept_restore_order`; extract `allocate_order_target` separately from Create enqueue and share existing backup source/reservation checks.

- [ ] RED concurrent-poll test: ordinary accepted create target races restore; new path must expose zero Create jobs.
- [ ] Prepare generates offer ID/expiry before one immutable insert; hash includes complete source/target/review terms, never UPDATE a prior offer.
- [ ] Accept revalidates authority/scope/source/availability/capacity/current bindings, allocates/reserves and queues direct Restore in one worker transaction with audit and receipt.
- [ ] Reject restore-purpose offers in ordinary acceptance before prior-order reuse and in ordinary Create. Preserve legacy absent-purpose offers and upgrades.
- [ ] GREEN double accept/same-offer different keys/lost reply yields one order/job/reservation; failures roll back first acceptance and never release the source. Issued uncertainty remains isolated/reserved.

### Task 6: Real bearer/MCP lane and lifetime — PG owner after OAuth/lock readiness

**Files:** new `control-plane/src/resource_databases.rs`; narrow `resource_hosting.rs`, `first_party_mcp.rs`, `app.rs`, agent-control database-service builder; database-owned authority/service/claim/maintenance paths.

**Interfaces:** Preserve Browser `execute_managed`; add MCP entrypoint and additive `with_oauth` builder. Existing constructor and native JobSpec/protobuf remain unchanged.

- [ ] Dispatch fixed safe database actions before Hosting parser with actual OAuth MCP guard and Database feature checks; fixed discovery filters mutation tools for read-only scope.
- [ ] Exclude secret registration/reveal/material from bearer/MCP, return bounded public metadata only; use explicit pagination if exposing bulk lists without changing existing Console response semantics.
- [ ] Retain original non-grant Browser/MCP authority through commit, claim, renewal, guard, checkpoint, promotion and explicit reconcile; manual backup is not automatically durable merely because it has a policy ID.
- [ ] Cap MCP-backed leases at min(normal deadline, verified original expiry); expiry/denial cannot promote Ready or release uncertain capacity. Explicit fresh operator reconcile alone may replace proof/generation while retaining exact target/intent/checkpoint lineage.
- [ ] Test genuine REST/MCP consent/scopes/member/operator/customer/preview ceilings and token/client/session/membership revocation. Durable committed policy recurrence/observation remains separately UID/revision/resource/revocation-bound.

### Task 7: Independent reviews and actual native journey

- [ ] Stop writes at source-ready checkpoints; Root arranges NEW independent reviews with prior findings and exact source inventories.
- [ ] After final54/56 schema and shared compiler/pins are stable, run actual Console → Core → Native/TLS/SQL fresh-target restore, source retained, historical credential/key and expected data hash verification.
- [ ] Preserve all prior evidence, exact owned fixture cleanup and workload-only timing; no public recovery SLA/window or production/provider approval.

## Execution notes

Existing delegated execution is already authorized. No user confirmation or commit step is required. Each owner must announce shared source/compiler/lock checkpoints and use bounded owned fixtures, `-j2`, private diagnostics and exact cleanup. The independent source reviews and actual native proof remain required; this plan is not their substitute.


## Observed stage checkpoints (not whole-feature approval)

- Task1 complete: Root lock `e65e941b5e8f50b6faf6c54c43b455c3b541540debbdc4bbdcab429d7d44e5bf`; Archives lock `7129686b2bfe088f1e145697f86f742745283201d1804472f765b7b67aae1466`; runtime unchanged. Registry804/620/400 tuple equality proven; only local databases dependency records changed. See `docs/verification/2026-10-26-pg-oauth-local-dependency-edge.json`.
- OAuth prerequisite source/API review passed without confirmedP1/P2; typed expiry getter is live. Existing owned current-code auth DB component passed8.452s, one test, no pre-fix SQL RED. See `docs/verification/2026-10-26-oauth-mcp-owned-postgres-component.json`; component setup/overlay/proof limits remain explicit.
- Pure unlinked intent/authority/schema stage11tests and narrow current-path allocation/restore refactor35tests completed; refactor independentreview noP1/P2. Actual allocation rollback test compiledbutunexecuted.
- Next released PG stage: complete0056 bodies ONLY in external plan draft (not migration directory/not executed) and new INTERNAL/cfgtest restore-order logic. Public parser/routes/claim/epoch activation and actual0056 materialization remain held pending draft review/final54 coordination. Existing active paths must not query undefined functions.
- Shared epoch constraint clarified: acquire one worker epoch SHARE NOWAIT before retained business/auth and installation/customer/cluster/op locks. Failure drops entire worker attempt before further locks. After success, business session fence then fresh retained authority and scoped locks; never upgrade agent or lock Main recovery job/policy/op while holding epoch SHARE. Actual overlap tests are still required.
