# Native durable plugin jobs

## Intent and scope

Continue the Rust-only platform with immediate, durable plugin jobs. Plugins enqueue,
read and cancel work through the authenticated native Host API. Core persists and
leases execution; an enrolled plugin runtime executes the declared handler. No Redis,
Bun worker, new public HTTP API, or customer/project fabricated for infrastructure.

The first milestone supports session-admitted project jobs, not autonomous system
jobs. Delayed starts, recurring schedules, product-specific job migrations, a generic
Console job dashboard, and backups are deferred. No existing Mail/Hosting workflow is
silently moved. This scope is the implementer's choice under the user's instruction
to proceed to the next Core topic.

## Admission and authority

- Enqueue requires a live **write action**, `plugin.jobs.enqueue`, enabled installation,
  a declared immediate job, approved/granted job capabilities and the current job grant.
- Inputs cannot select customer, project, actor, generation, authority, endpoint or lease.
- Persist real subject/session, exact original native/first-party/MCP authority,
  customer acting epoch, parent action/resource policy and full native admission snapshot.
  Reject durable enqueue when there is no retained native authority reference.
- Normal parent invocation cleanup does not revoke admitted jobs. Jobs independently
  revalidate the original session and its write ceiling. Session expiry/revocation,
  membership loss, acting exit/re-entry, grant/config/generation change or parent policy
  denial cancels remaining work. Infrastructure errors fail closed without pretending
  to be a confirmed authority revocation.
- Job host execution supports declared data/http capabilities only. Apps, nested jobs,
  direct platform/database credentials and unsupported adapters remain unavailable.
- Use the real actor-backed signed SQL session scope for job host calls, additionally
  requiring the current execution claim. Do not retain the old actorless worker escape.
- Canonical lock order: auth first, installation shared fence, customer/project,
  installation/configuration/runtime/release/grant, job, invocation. Release discovery
  transactions before acquiring auth. No external awaits inside database transactions.

## Public contracts

Strict camelCase enqueue JSON:
`{name,payload,idempotencyKey,attempts?}`. Attempts default 3, range 1..5. Key is
8..128 ASCII alphanumeric/underscore/dot/hyphen; payload and result at most 32768
UTF-8 bytes. `delaySeconds`, `runAt` and schedule fields are rejected.

- Host `jobs.enqueue` -> `{id}`
- Host `jobs.get` with `{id}` -> `{id,name,state,attempts,result?}`
- Host `jobs.cancel` with `{id}` -> `{id,cancelRequested:true}`

Public IDs retain `job-<64 lowercase hex>`, scoped by plugin/customer/project/key.
Equal canonical request returns the same retained ID; changed name/payload/attempt
policy conflicts. Canonical fingerprints preserve the persisted JSON numeric values;
distinct integers must not collapse through an intermediate floating-point conversion.
Keep terminal rows for idempotency. Foreign IDs reveal no record.
States are `waiting`, `active`, `completed`, `failed`, `cancelled`; there is no public
exception/stack/provider diagnostic. Explicit job output is plugin-owned public data.

`InvocationClaims` gains optional signed `job: {id,attempt}`. Omit it for actions;
existing action serialization and golden signatures remain unchanged. Reject explicit
null, malformed metadata, invalid attempts and unknown fields. A fresh invocation UUID
links each attempt to its private DB claim token. Ordinary `Invoke` rejects job claims;
new `RunJob` requires them and resolves `Manifest::job`, never an action fallback.

Runtime adds opt-in `JobHandler` via `Runtime::with_jobs`. Existing `Runtime::new`
and all product action handlers remain compatible. The native Host client exposes the
three bounded jobs calls; legacy TypeScript HTTP queue contracts remain historical.

## Durable execution

Reuse `plugin_job_execution`; append migration **0044**, leaving 0042/0043 unchanged.
Add bounded native provenance, canonical request fingerprint, max attempts, result,
sanitized reason and attempt invocation binding. Adapt invocation actor constraints and
SQL policy/helpers. Cancel provenance-less legacy rows and revoke their job invocations;
do not invent authority. Populate job grants for current declarations without reviving
revoked grants or silently expanding a retained revision.

Core claims atomically, increments attempt and rotates claim UUID; two workers cannot
own one live lease. Token lifetime 60 seconds, runtime attempt limit **45 seconds**,
fixed DB lease **90 seconds**. No lease renewal. Transport deadlines must exceed the
job execution budget while ordinary action application deadlines remain 30 seconds.
Revalidate every 250ms and before result release, using an absolute attempt deadline.

Retry only transient unavailable/deadline failures within max attempts, with exponential
backoff 1,2,4,8,16 seconds. Expired abandoned claims are reclaimable after 90 seconds;
old invocation/claim/completion cannot publish effects or advance the new attempt.
Completion checks the exact live lease and original authority. Terminal errors and
cancel requests do not retry. Results and internal conditions remain bounded/redacted.

Execution is **at least once**. Stable `operationID = jobID` across attempts permits
existing data receipts to deduplicate committed host effects. External provider effects
need their own idempotency; cancellation cannot undo an accepted effect. A running
cancel invalidates subsequent host calls/results and drops its handler on revalidation.

Existing pending jobs/retries may finish in `draining`; new enqueue is denied. Existing
continuation SQL must keep disable/update blocked until jobs and invocations terminate.
Exhausted/cancelled/invalidated pending work must be terminalized even if not executable.

## Integration and acceptance

Advance a job from the native plugin controller using the existing generation/pinned
runtime connector. Keep transport outside transactions. Resolve exact runtime generation
before/after execution and feed only sanitized retry classifications to durable finish.

Prove behavior with a disposable PostgreSQL18 native bootstrap and actual UDS Host and
Runtime RPCs using a synthetic jobs plugin. Include real scoped document writes, two
claimers, restart/reclaim, stale completion, cancellation, retry exhaustion, partition
denial, session/acting/grant/policy invalidation, pending drain and parent cleanup.
No k3s/OCI rebuild is necessary for a pure Core/runtime job protocol. Leave existing dev
services stopped and clean only test-owned containers/files. Run full repository quality,
fresh independent review and any fixes before signed branch integration.
