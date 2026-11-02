# Core BullMQ operations (historical)

This describes the retained TypeScript implementation. The active Rust job path and
`bun run test:plugin-jobs` are documented in [Native plugin jobs](native-plugin-jobs.md).

Core owns BullMQ 6.3.16 and ioredis 6.0.0. `@webdock/core/jobs` exports explicit Redis configuration and durable per-domain dispatch workers. `@webdock/core/plugin-jobs` owns the scoped plugin service and HTTP adapter. `@webdock/plugin-sdk/server` is the plugin-facing client; it has no Redis dependency.

## Platform worker composition

`apps/control-plane/src/worker.ts` uses BullMQ directly for the configured Mail/Hosting/Storage/Billing domains. Each domain has one durable scheduler (`dispatch`, every 5 seconds) and a domain-owned queue. Replicas share the scheduler identity. PostgreSQL `api_job` records, slot uniqueness, claim tokens and lease renewal still guard existing business work; Billing retains its own accepted-work journal. No in-memory timer polling loop is used as the platform scheduler.

Redis is now required by every platform worker. Set `WEBDOCK_JOBS_REDIS_URL` (redis:// or rediss://) in the worker's existing Secret/environment and optionally `WEBDOCK_JOBS_PREFIX` (default `webdock`, unique per installation). Use a persistent Redis with AOF, backups and `maxmemory-policy noeviction`, no public port, authenticated/TLS access across trust boundaries and explicit NetworkPolicy egress. Do not share this persistence with an evicting cache. Compose provisions the isolated `jobs-redis` service/profile and persistent volume; Helm expects operator-provisioned Redis and configuration in `components.worker.existingSecret`.

Start refuses missing Redis configuration. Unavailable Redis fails its readiness deadline and force-closes initializing sockets. Runtime errors log domain/event only. Graceful termination stops scheduling new business claims, drains active BullMQ workers, then closes database pools. Keep the existing PostgreSQL journal when upgrading or rolling back; no data reset, queue flush or credential rotation is part of this implementation.

## Plugin authority boundary

The Core service requires `authority.authorize(request, capability)` and `authority.canRun(scope, jobName)` plus an isolated `execute` port. There is no permissive default. The production adapter must verify the short-lived invocation, current actor/membership, grant revision, runtime generation, enabled state, write intent and per-grant quota before creating a JobGrant. It must recheck current execution authority while a job runs. Requests cannot choose scope, arbitrary queue names or arbitrary BullMQ options.

The production container installer/invocation transport is still unfinished. Therefore the platform's public API does **not** mount the plugin-job adapter unconditionally and the plugin worker is not started with fabricated production grants. The local fixture binds these ports to explicitly labeled, short-lived local grants and an independent HTTP plugin process. This is real BullMQ/Redis/HTTP verification, not production authorization evidence.

Cancellation is cooperative; execute implementations must honor AbortSignal and fence every host-side effect against current authority. Deadlines stop waiting for a handler but cannot undo committed effects. Jobs and cancellation tombstones are retained; production composition must add the quota/admission and retention policy before exposing this to untrusted authors. No queue-wide admin operations or raw Redis commands are exposed to plugins.

## Verification

`bun run test:plugin-jobs` provisions its own pinned Redis container and verifies Core scheduling/restart/domain ownership, Redis-down startup cleanup, scoped job API, retries, idempotency conflicts, read-only denial, foreign-project denial, delayed/running cancellation and revocation. It also checks packed SDK/CLI in an empty directory and an actual separate plugin process.

The actual restricted-role `worker.ts` storage profile was run against a new isolated PostgreSQL fixture and real Redis; it completed its business job and exited cleanly on SIGTERM. Production deployment has not been performed.
