# Native hosting service

The crate stages and completes real retained hosting operations. It does not own
Docker/Kubernetes access; the agent executes typed, authorized commands.

## Host API

`HostingService::new(session_db, bootstrap_auth_db, worker_db, rls_secret,
environment_secret, Arc<IdAllocator>)` is asynchronous and requires the exact
restricted `webdock_auth_runtime`, `webdock_auth_bootstrap` and
`webdock_worker_runtime` roles. The ID allocator must use its own pool.

- `execute(&Principal, Value)` accepts retained `action` command envelopes.
  `execute_with_ceiling` also requires explicit read/write OAuth scope ceilings.
- `authenticate_agent(cluster, raw_credential)` verifies the retained 43-character
  bearer credential against its live hash, revocation state and generation.
- `authenticate_enrolled_agent(cluster, generation, credential_hash)` is a
  **trusted backend integration**, not an HTTP authentication endpoint. Core must
  first prove UDS UID or a verified TLS certificate against its persisted transport
  enrollment. Its expected hash comes from that enrollment record, never a header
  or a client-declared Hello field. Rotation/revocation is checked again in each
  operation. The returned `AgentIdentity` has private fields.
- `agent_request(&AgentIdentity, action, Value)` retains `heartbeat`, `claim`,
  `guard`, and `complete` envelopes for the existing executor path.
- `get_read(&Principal, operation_id)` returns a typed `ReadView` for logs,
  inspection or metrics. It checks persisted module and live target permission;
  only successful, action-shaped results are exposed. Execution envelopes,
  receipt digests and internal failure details stay private.
- `claim_runtime(&AgentIdentity)` selects eligible native Docker work, acquires
  its lease and derives `webdock_supervisor::Command` in one transaction.
  Unsupported commands remain queued without acquiring a lease; polling never
  marks them failed. Unsupported oldest rows do not prevent selection of eligible
  work in other projects. Retained Kubernetes work uses its dedicated executor.
- `runtime_command`, `guard_runtime_command`, `complete_runtime` and
  `complete_runtime_failure` bind native dispatch and receipts to the persisted
  desired command, live lease/generation, enrollment and MAC. Success and failure
  acknowledgments can be replayed after a lost acknowledgment. Failure replay
  additionally requires the original error code and stored command fingerprint.

Core owns API/gRPC adapters and its dispatched-command/receipt journal. Keep the
original dispatched `Command` for completion retries; a running-only command
lookup cannot reconstruct authority after completion. Runtime selection,
installation/name derivation and expected prior UID come from database records,
not client-provided namespace or container names.

## Native Console acceptance

With the explicitly owned restricted-role fixture environment, the reviewed
BusyBox image already present in local Docker, Bun/Playwright available and
the Console built, run:

```sh
WEBDOCK_HOSTING_BROWSER_TEST=1 cargo test -p webdock-hosting \
  --features docker-test --test postgres \
  native_browser::console_drives_real_native_hosting_and_docker \
  --locked -- --ignored --exact --nocapture
```

This serves the built UI through the real Core router on an ephemeral loopback
port and runs the actual enrolled daemon against uniquely owned Docker resources.
It checks navigation, lifecycle commands, durable results, sanitized logs and
settings changes through the browser. The private synthetic session descriptor
is temporary; there are no mocked HTTP responses or production environment
files. Cleanup verifies ownership before removing the fixture container/network
and deletes only its recorded rows. Account setup/SSO has separate acceptance
coverage; this fixture does not repeat those ceremonies.

## Preserved behavior

Implemented command paths include app list/detail, prepare/create/deploy/update,
scale/start/stop/restart/rollback, retained executor logs, deletion/storage plans,
retained executor delete/purge and operator reconciliation. Project list/detail
and configuration, cluster list/detail/register/activate/revoke, limits/usage,
limit updates, and operator operation overview/detail are backed by retained data.

All session operations install a transaction-local signed scope, check current
identity/customer/project permission and managed/self-service mode, and recheck
before commit. Installation disable/drain state is checked under the shared
lifecycle fence. Mutation receipts, app revisions, queued commands, reservations
and audit records commit together. Existing project deletion fences remain active.

Quota checks preserve customer-wide and provider/project ceilings, subscription
revision CAS, per-app replica maxima and active/reserved concurrency accounting.
The retained `capacity_demand(...,'all',...)` helper counts shared application,
Mail and database allocations without exposing other customers' rows or ignoring
disabled products. No Mail deployment, activation or retained-data behavior changes.

Agent claims serialize project work, increment operation generation, and issue
bounded leases. Expired writes become `needs-reconciliation`; reservations are
never released merely because a lease expired or execution failed. Completion
checks target revision, namespace, UID and convergence/storage proof. New commands
persist originating sessions. Operator reconciliation records fresh authority in
separate columns while preserving the original actor and idempotency scope.
Historical commands without persisted session authority require explicit review
and reconciliation before execution.

Environment encryption preserves the existing AES-256-GCM `v1` envelope and
`hosting-env:<appID>` AAD, using the retained environment/Mail encryption key.
Names are returned to browsers; values are supplied only to the authorized agent.
Log completion bounds and redacts current, previous, observed and historical
application secrets, including truncated edge fragments. A Node golden fixture
checks retained ciphertext compatibility.

## Runtime migration and remaining parity

Apply `migrations/0001_runtime_kind.sql` through the offline migration runner's
transaction. It is additive: existing `k3s` values remain Kubernetes, `docker` is
an explicit legacy-wire backend value, and cluster `runtime_kind` is separate from
the infrastructure provider label. It adds a narrowly authorized runtime-kind
helper and reconciliation authority columns. Existing capacity helpers already
aggregate by ownership and cluster ID and therefore retain cross-product capacity
accounting for either runtime.

The complete persisted no-volume Docker flow is tested against an owned local
Engine: create → stop → start → update → rollback → completion/failure replay.
An additional real Core UDS → Daemon → Docker test discards a Poll response and
injects a lost completion acknowledgment. The daemon persists and flushes the
exact receipt before polling again; Core/hosting receipt replay is idempotent.
Docker byte quotas for persistent volumes, environment delivery, multiple replicas,
log-operation mapping and deletion mapping remain unavailable in this slice; new
requests fail closed. The existing generic Helm adapter must not silently adopt
retained `wd-<projectID>` resources. A dedicated retained Kubernetes executor and
its volume/environment/service adapters remain separate agent work; the database
and legacy proof path are tested here without calling a live Kubernetes cluster.

Other remaining ports: cluster enrollment issuance/consumption endpoints; Git/build
source publication; provider/BYOK adapters; setup/relocation orchestration;
service-binding and plugin-provisioning authority adapters; exact Orama tokenizer
search parity and shared read-cache integration. Existing plugin-bound operations
are denied until their live provisioning authority adapter is connected. Unsupported
commands return an explicit unavailable error, never a fabricated successful result.

## Validation

```sh
cargo test -p webdock-hosting
cargo clippy -p webdock-hosting --all-targets --all-features --no-deps -- -D warnings
```

Database tests require explicit loopback URLs for `webdock_rls_test` from
`WEBDOCK_TEST_MIGRATION_DATABASE_URL`, `DATABASE_URL`, `AUTH_DATABASE_URL`, and
`WEBDOCK_PLUGIN_WORKER_DATABASE_URL`, plus `DATABASE_RLS_SECRET` and
`BETTER_AUTH_SECRET`. Apply the additive migration offline first; the full RPC test
also requires the native agent-control enrollment/dispatch migration. Fixtures use
unique records and clean only those records; they do not reset schemas or shared
data. The test-created installation singleton is serialized by a fixture lock.

```sh
cargo test -p webdock-hosting --test postgres -- --ignored --test-threads=1
WEBDOCK_HOSTING_DOCKER_TEST=1 cargo test -p webdock-hosting --features docker-test -- --include-ignored --test-threads=1
```

The Docker test explicitly uses `/var/run/docker.sock`, requires its reviewed image
to be cached beforehand, and removes only its exact container/network names. It
never provisions customer infrastructure or pulls images.

The JSON schemas capture the provider-neutral TypeScript command slice for parity; runtime
validation and all domain behavior are Rust. Regenerate intentionally with
`bun crates/hosting/tools/capture-contracts.mjs`; Rust retains the
additional cross-field/ID/environment checks that JSON Schema cannot express.

## Stale native receipt custody

Core verifies current transport enrollment and loads the exact persisted dispatch
before decoding a receipt. Issued command MACs bind the same agent credential and
generation, operation generation, lease and workload fingerprint. Receipt hashes
are retained both in the transport journal and in bounded typed operation records.
Every live native mutation/read success or failure stores the exact transport hash
in the same transaction as its domain result, before the separate transport ACK.
A crash in that gap therefore cannot accept a changed replay or poison stale
custody. Existing semantic read/failure receipts remain authoritative for older
domain commits without a transport hash; native mutation successes now also have
a keyed semantic receipt. A successful historical outcome without verifiable
receipt provenance fails closed rather than being relabeled stale. Unknown provenance, changed receipts and malformed stored custody
fail closed. Authority database failures remain pending; they are not revocations.

An expired, revoked or superseded delivery records custody only. The same still
running operation/revision may become `needs-reconciliation` (reads become failed).
It cannot set Ready, alter observed state, renew a lease, settle usage or release
reservations. Operator reconciliation supplies a fresh session and generation;
late old receipts cannot overwrite the renewed operation. New custody records stop
at 256 per operation rather than discarding exact-replay history.

When Core Hosting execution is disabled, previously recorded exact receipts still
ACK, but unseen stale receipts remain pending. Heartbeats and independent domain
work continue; new Hosting dispatch waits for that receipt to be resolved after
Hosting is enabled. Enrollment revocation remains fatal. This implementation does
not adopt receipts across agent generations.

The owned PostgreSQL/Unix-daemon acceptance in `tests/support/delivery.rs` uses a
controlled runtime adapter and real Core, signed scopes, dispatches, journals and
RPCs. It exercises expiry, session revocation, reconciliation, retained allocation,
read delivery, database unavailability, malformed/changed/forged callbacks, disabled
Core policy, higher-generation observations and enrollment revocation. No cluster
or provider is provisioned by that acceptance.

## Completion deadlines

Live mutation/read success and failure, including retained agent completion calls,
make a final PostgreSQL `clock_timestamp()` decision immediately before commit,
after blocking projection, audit, authority and receipt work. The already-locked
operation must retain its exact generation and persisted lease; native commands
also retain their signed deadline. The originating/reconciliation session or
verified browser grant supplies the authority ceiling. A newer unrelated session
cannot extend it. Expiry rolls back projections, quota settlement, audits and
accepted receipt hashes together. The final read introduces no cache-trigger write
or new lock order. Previously committed exact replays and stale receipt custody
remain separate from live-outcome admission.
