# Native performance runtime

Owner direction: execute the complete Axum/tokio-postgres backend rewrite and
measured S3/storage improvements. The owner reiterated “Los”; execute locally in
reviewable increments without further planning menus. Deployment and credential
boundaries remain unchanged.

## Final target

Use Axum directly for HTTP and Tokio for service lifecycle. Move database access
from SeaORM/SQLx to explicit parameterized tokio-postgres operations across the
actual service boundaries. Preserve PostgreSQL roles, tenant RLS, transaction-local
authority, pooled connection cleanup, durable queues, migration history,
idempotency, MFA/passkeys, API/gRPC/SSE contracts and safe shutdown.

Measure throughput, tail latency, CPU, allocation/memory, idle/connection behavior
and Neon compute/egress before claiming improvements. Benchmark the existing
object_store streaming paths before selecting replacement S3 libraries. Preserve
multipart cancellation, range reads, scoped credentials and storage admission.
Apply improvements through the existing small Core/plugin boundaries.

## Increment 1: direct Axum startup

Discovery: request routing is already Axum. Loco wraps startup/configuration in
four Core source files; SeaORM spans 14 crates. Remove the Loco wrapper first,
retaining the established router and service initialization. Do not mix this with
the 14-crate driver conversion or a database/schema migration.

Replace framework configuration with a small native configuration containing the
validated database target and HTTP binding. Keep existing env names, production
mode compatibility (including LOCO_ENV), pool bounds/timeouts and sanitized errors.
Initialize the same restricted-role pools and services, without startup schema
mutation, seeding, a framework queue or new database polling.

Serve the existing Router through axum::serve. Register SIGTERM/SIGINT before
accepting traffic, support graceful request completion and use bounded shutdown
so an open SSE/keep-alive client cannot prevent service termination indefinitely.
Use existing tracing conventions and JSON startup logs; never log configuration
objects, connection strings or secret values.

Remove the unused framework and framework migration dependencies from the native
Core closure. Existing explicit offline migration/bootstrap commands remain the
only schema/initial-account authorities. SeaORM remains temporarily in service
constructors until the subsequent tokio-postgres increments; do not describe this
first increment as the completed database rewrite or a measured speedup.

## Proof

Native config tests preserve TLS/target authority and hide credentials. Real
loopback HTTP tests prove existing health/config/security behavior, graceful
shutdown and bounded forced completion for a held request. An owned PostgreSQL
fixture must start the actual binary with restricted runtime roles and unchanged
migration history; test readiness and process shutdown. Run the existing HTTP,
identity/authorization and Core regression gates before release. Production
rollouts retain digest/rollback and approved secrets; no schema change or key
rotation is part of this increment.
