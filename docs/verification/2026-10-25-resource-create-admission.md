# Resource create count admission — 2026-10-25

This is a Core prerequisite fix, not acceptance of the Managed Object Storage product.

## Reproduction and change

The original concurrent admission test accepted both last-slot creates before either
plugin returned (2 accepted; expected 1). Allocation counted only `plugin_resource`.
The tests also reproduced job fan-out, missing pending counts and premature count
release on a queued delete, including enqueue committed while settlement waited on
the customer budget lock.

Admission now persists a reservation in the existing `plugin_invocation.session_context`
under the existing customer limits lock. Unreleased migration **0047** updates allocation,
reservation, job binding and indexing functions and adds three indexes. It adds no table
or dependency; 0046 and earlier migrations are unchanged.

## Invariants

- One slot per idempotent create operation, bound to its original invocation, actor,
  request hash, customer, project, plugin and offering. Retries retain that association
  across token expiry, revocation and runtime promotion. Pending synchronous retries conflict;
  they never receive permission to run another external create. Retry authorization is current.
- A create delegates to at most one exact durable job. Its live attempt, claim and
  invocation must match before job completion indexes a resource. Create jobs run at most
  one attempt: unavailable responses, deadlines and expired leases can hide an external
  resource and cannot authorize another attempt. Generic jobs retain their retry behavior.
- Completed synchronous creates replay only Core's resource ID/name. Delegated creates
  replay the exact retained job ID, including after completion and before the original
  acknowledgement was recorded. Both browser and MCP dispatch bypass the runtime on replay
  after current admission fences. No plugin output, credential reveal or job secret is
  copied into a replay cache; successful create-job results retain only resource ID/name.
- Index insertion and reservation settlement commit together under the customer
  limits lock, inside the job completion transaction when applicable. No double count.
  Another resource ID, ownership collision or stale job attempt cannot settle the slot.
- Policy changes cannot rewrite the admitted offering. Invalid output, lost responses,
  revoked authority, cancellation and exhausted attempts do not prove no external resource
  exists and retain the pending slot. Failed admission before commit rolls back the slot.
- An enqueue acknowledgement cannot release a delete's count. Delete fan-out is rejected
  across invocations of the same operation. The action must acknowledge its exact job
  before that job becomes dispatchable; a first bookkeeping job cannot run or free count
  when a later enqueue fails. Only that designated job's live attempt may settle deletion.
  Enqueue and settlement lock the existing resource row. Deleted creates cannot be
  replayed into the runtime.
- Existing auth/session/MCP, RLS, grant, runtime and installation fences remain in place.
  Existing runtime DELETE denial retains the invocation ledger; do not purge reservation
  roots or their job provenance as expired-token cleanup.

## Accepted review reproductions

All three accepted issues were reproduced RED before their fixes:

1. Real dispatcher, Host RPC and runtime, with a deliberately non-idempotent handler
   writing a fresh UUID to an independent external-effects table: both completed-response
   replay and lost external response produced **2 effects, expected 1**. The create-job
   retry reproduced the same duplicate external effect. This models PostgreSQL
   `create_shared`'s fresh UUID behavior; the actual handler was inspected but not run.
2. One delete action accepted a bookkeeping job followed by a second actual-delete job.
   The regression also reproduced claiming the first job after rejecting fan-out; the
   acknowledged-job gate prevents this. A new invocation of the same operation cannot
   enqueue another job, and stale deletion attempts cannot free capacity.
3. Re-enqueueing an exact completed/indexed create job returned `NotFound`. Existing-job
   lookup now precedes the pending-only guard for new insertion; fingerprint, operation,
   actor, resource action and reservation identity still must match.

The OAuth/MCP regression uses a fresh owned client, real PKCE/consent/token authority,
real runtime dispatch, a retained-result replay and access-token revocation. Sync replay
also checks grant revocation and stale-principal session revocation. Public metadata-only
replay and create-job result persistence are asserted with synthetic private result fields.

## Verification

Commands from the repository root:

- `cargo test --locked -p webdock-plugin-host --test resource_admission -- --ignored --test-threads=1` — **14 passed**, using exclusively owned PostgreSQL 18 fixtures and real Host/runtime RPC where applicable.
- `cargo test --locked -p webdock-plugin-host --test jobs -- --ignored --test-threads=1` — **18 passed**, including real private Host/runtime RPC, source authority, leases and generic-job retry fences.
- `cargo test --locked -p webdock-plugin-host -p webdock-control-plane` — passed; infrastructure suites remain explicitly ignored by default.
- `cargo clippy --locked -p webdock-plugin-host -p webdock-control-plane --all-targets --no-deps -- -D warnings` — passed.
- `rustfmt --edition 2024 --check` on the owned Rust source files, dispatcher and test; scoped `git diff --check` — passed.

Full dependency Clippy (`--all-targets -- -D warnings`, without `--no-deps`) is blocked
by the concurrent out-of-scope `clippy::collapsible_if` at
`crates/agent-protocol/src/database.rs:591`. Earlier verification attempts also encountered
transient concurrent compile failures (missing `url`, `DatabaseMaterial.object_store`,
and imports in `databases/src/service/backups.rs`); these were left untouched, and the
normal tests and scoped Clippy subsequently passed after the shared tree stabilized.

Fixtures use random container names/ports and private temporary directories; their
Drop cleanup removes only the exact owned container name. No existing private
operational database, retained service, production or provider resource was used.
Post-run `docker --host unix:///var/run/docker.sock ps -a --filter name=wd-plugin-jobs- --format '{{.Names}} {{.Status}}'`
returned no containers.

## Remaining limits

Historical pre-0047 invocations have no trustworthy create/index association. Quiesce
old create dispatch and reconcile unknown historical outcomes before rollout; this
migration does not guess a backfill. Pending uncertain slots intentionally remain held.
Unacknowledged deletion jobs stay undispatched and retain their resource index until
an exact acknowledgement or reconciliation. A conservative create-job failure can
require reconciliation even when dispatch never reached the provider. There is no new
reconciliation or generic failure-based release API. Only declared resource creates
are counted, not arbitrary plugin HTTP effects or S3 byte quotas. Broader retained-fixture
self-service tests remain ignored. No full Managed Object Storage, provider, Console
or production acceptance is claimed.
