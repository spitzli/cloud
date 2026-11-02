# Native plugin jobs

The implemented scope is defined in [the jobs design](../superpowers/specs/2026-10-24-native-plugin-jobs-design.md).
Verification and its limits are recorded in [the native acceptance record](../verification/2026-10-24-native-plugin-jobs.json).

Plugins declare immediate jobs in their signed manifest and opt into a Rust
`JobHandler`. An authorized action can enqueue work through `HostClient`; Core stores
the work in PostgreSQL and the native plugin controller dispatches it to the pinned
current runtime. Redis and the historical TypeScript job HTTP service are not involved.

## Plugin contract

Declare `jobs` version1 in required/optional services and the capabilities used by each
action. A write action needs `plugin.jobs.enqueue` to queue a job, a status action needs
`plugin.jobs.read`, and a write cancellation action needs `plugin.jobs.cancel`.

A job declaration contains `name`, `capabilities`, and optionally an input JSON schema.
Job capabilities in this milestone are `plugin.data.read`, `plugin.data.write`,
`plugin.http.read`, and `plugin.http.request`, subject to their existing service and
grant restrictions. Scheduled declarations (`intervalSeconds`), apps capabilities and
nested jobs remain unsupported by the native lifecycle.

The native Host Call service `jobs` has these methods:

| Method | Input | Result |
| --- | --- | --- |
| `enqueue` | `{name,payload,idempotencyKey,attempts?}` | `{id}` |
| `get` | `{id}` | `{id,name,state,attempts,result?}` |
| `cancel` | `{id}` | `{id,cancelRequested:true}` |

Inputs are strict: a plugin cannot supply an actor, project, customer, lease, image or
endpoint. Enqueue keys contain 8–128 ASCII letters/digits/underscore/dot/hyphen.
Attempts default to3 and may be1–5. JSON payloads and results are limited to32KiB.
`runAt`, `delaySeconds` and recurring schedules are not accepted.

Store the returned ID (`job-` followed by64 lowercase hex digits) for status reads.
States are `waiting`, `active`, `completed`, `failed` and `cancelled`. Results are
explicit plugin output; never put credentials into them. Exception text/provider
diagnostics are not exposed as status. Foreign project/customer/plugin IDs are hidden.

## Authority lifetime

A job survives normal cleanup of its initiating action. It retains the original real
actor, session and exact first-party/native/MCP authority. Each claim and host effect
checks them again, including project membership, write ceiling, action/resource policy,
grant/configuration/runtime generation and any customer acting epoch.

Session expiry or revocation cancels work. Leaving and re-entering “Act as customer”
cannot revive a job admitted in the previous scope. Autonomous system jobs require a
separate authority contract and are not part of this milestone.

Migration0044 cancels historical pending/running jobs without captured native admission
authority and revokes their linked attempts. Their records remain retained; they are
not automatically replayed. Enqueue the intended work again through an authorized
native action if it is still needed.

## Retries, crashes and cancellation

Each attempt has a45-second handler budget, a60-second signed token and a90-second
database lease. A crashed controller leaves retained work; another controller may
reclaim it after lease expiry. Only the current claim can publish completion. An old
runtime request cannot advance a replacement attempt.

Transient unavailable/deadline failures retry within the admitted attempt limit, with
bounded exponential backoff. Invalid input, unavailable handlers, authority loss and
cancellation do not retry. Terminal rows remain retained for idempotency: enqueueing
the same canonical request/key returns the same ID; changing the request conflicts.

Execution is **at least once**, including when a controller crashes after an external
effect but before recording completion. The signed job ID and host `operationID` remain
stable across attempts. Use stable idempotency keys/receipts for effects; provider calls
need provider-specific deduplication. Cancellation cannot undo an accepted effect.

Pending cancellation prevents dispatch. Running cancellation invalidates subsequent
host calls and result release and drops the job future on revalidation. A request
that already reached an external service may still have taken effect.

## Controller and availability

Run the existing `webdock-plugin-controller --config /absolute/private/plugin-config.json`
with the normal restricted native database environment. It advances jobs alongside
availability and installation operations. `--once` advances one controller pass and
waits for its bounded job attempt. No extra queue service or privileged database login
belongs in the runtime/controller configuration.

Disabling first enters `draining`: new enqueue is denied, while accepted pending jobs
and bounded retries may finish. Existing continuation checks hold disable and a later
update/rollback until work becomes terminal. A job never migrates into another runtime
generation. Controller shutdown drops in-flight execution; retained leases recover it.

## Verification

`bun run test:plugin-jobs` runs the native disposable-PostgreSQL/RPC
acceptance. It owns its fixture containers/files and needs local Docker, not the dev
database, k3s or real provider credentials. This acceptance is separate from the
historical BullMQ/SDK test suite, whose scheduling contracts are not yet native.
