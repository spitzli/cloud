# Customer-owned plugin workloads implementation plan

> Execute with superpowers:executing-plans; independent Act-as-customer and Mail work uses delegated implementers.

**Goal:** Support infrastructure owned by a customer across projects, without tying retained data to an arbitrary project.
**Architecture:** Keep the authenticated runtime workload boundary. Apply accepts exactly one project or customer; Core pins a distinct namespace and customer ownership, uses customer limits and verified domains, and preserves the selected cluster for existing units. Signed action claims carry the server-derived customer ID. Product logic remains in plugins.
**Tech stack:** Rust, PostgreSQL/RLS, existing React Console and Bun tooling.
**Spec:** Owner's 2026-10-23 implementation instruction and `docs/architecture/mail-deployment-decision.md`.

## Constraints

- Existing isolated worktree and Rust-only active API; no new dependency.
- No production changes, real mail, credentials in output or reading `.dev-local/demo-owner.json`.
- Preserve retained Mail data; suspension retains PVCs. A customer's engine survives project deletion.
- Migration 0040 belongs here; 0038/0039 belong to the parallel Act-as-customer task.
- Root owns protocol/claim fixtures and the final gate; the workload implementer owns workload admission, migration 0040 and null-project usage/notices. Auth owns `authority.rs`; Mail owns `plugins/mail`.

## Tasks

- [x] Add required signed `customerID` to invocation contracts and update producers/fixtures. Reject missing/invalid customer identities and revalidate against stored admission.
- [x] Reproduce customer-scope apply rejection; implement exclusive scope parsing, independent customer FK and namespace, customer allocation/domain checks, stable cluster and ownership.
- [x] Preserve project workload behavior; make usage samples, notices and logs support customer units and display workspace-wide services honestly.
- [x] Verify project deletion preserves customer workloads; customer deletion stays blocked by retained units. Verify cross-scope reassignment, unknown/archived customers and foreign domains fail.
- [x] Run restricted PostgreSQL and agent tests plus Console/browser contracts, integrate Mail and acting mode, independent review, `cargo fmt --all` and `bun run quality` before signing/pushing.

## Review focus

Unit ownership must never change between customers/projects. Runtime-selected clusters must be verified and not dedicated to another customer. Customer-wide usage cannot disappear from limits, notices or history. Signed customer identity must be checked again at execution. Existing project-scoped workloads keep their namespaces and retained objects.

## Workload implementation evidence

- RED: `apply_accepts_exactly_one_numeric_owner` failed on the customer input before implementation.
- `cargo test --locked -p webdock-plugin-host --lib`: 12 passed.
- Disposable PostgreSQL 18, native bootstrap/journal through 0040 and restricted runtime roles: four new customer workload tests passed, including concurrent customer/project budget admission and project owner-change rejection. Three existing project workload/limits/domain regressions passed.
- Focused command for the new database tests: `cargo test --locked -p webdock-plugin-host --test customer_workloads customer_ -- --include-ignored --skip shared:: --test-threads=1`. The imported shared fixture includes its own tests; the explicit skip prevents running them here.
- Fixture environment names: `WEBDOCK_TEST_MIGRATION_DATABASE_URL`, `AUTH_DATABASE_URL`, `DATABASE_URL`, `WEBDOCK_PLUGIN_WORKER_DATABASE_URL`, `DATABASE_RLS_SECRET`. The checks used separately owned disposable containers and removed them afterward; Auth's running fixture was not changed.
- Notices browser regression failed before nullable-project support; notices and plans browser checks pass after it. Root owns the domains regression.
- The implementer observed the parent's intentionally red zero-replica regression. Root fixed it: stopped workloads require the controller's observed generation and zero running/ready/available/updated/current/terminating replicas. All three agent workload unit tests and the real k3s apply/prune/metrics/scale-to-zero/remove test pass.
- Root reran all four customer-workload restricted PostgreSQL tests after integration through0041; all passed. The signed-customer contract passes26 Rust tests,6 transport tests,26 SDK tests and CLI/CMS runtime checks. Domains, usage and notices render workspace units without a project.
- Final `cargo fmt --all`, `bun run quality` and whitespace checks passed after the review fix pass. The Plans fixture now uses one exact hourly timestamp anchor, matching database samples and avoiding millisecond-created false gaps.

## Integration review

One independent review covered the three connected plans and found three Important issues: acting invitation jobs lacked the admitted scope, interrupted Mail bootstrap could return ready before final configuration, and synchronous suspension exceeded the runtime action deadline. All were reproduced and fixed with regression evidence. Migration0041 binds invitation delivery to the admitted epoch and requires explicit resend for legacy unbound queued jobs. Mail finishes configuration/recovery removal on every reconciliation and suspends asynchronously; the UI polls pending completion. The final Mail suite passes15 tests including the real two-customer k3s lifecycle. This is local acceptance, not production rollout or complete Mail product parity.

Ruling: project ownership changes are refused while project workloads are retained; customer units do not block unrelated empty-project moves. Project/resource name joins also match customer ownership. Existing project namespaces and placements are retained.

## Research handoff for subsequent slices

Native availability reconciliation, authenticated transports, workload provisioning and plugin-owned stores already exist. The next installer slice should join signed-release verification to native runtime configuration/enrollment and retained plugin database provisioning; the older Python installer deploys the previous HTTP/env runtime contract. Add generation-based update/rollback with schema compatibility next. Generic native jobs and installation-scoped operator actions remain separate gaps; project-scoped operator actions already work. Core extraction waits for plugin parity of Git builds/releases, Tabularis gateway sessions, and durable Mail/Webmail queue/handoff semantics.
