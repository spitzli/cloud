# Native Core HTTP runtime

The native Core binary serves the existing Axum router directly. Loco startup,
framework queues/tasks and framework migration hooks have been removed. Explicit
offline migration and account-bootstrap commands remain separate authorities.

This is the first performance-rewrite increment. Service persistence still uses
SeaORM/SQLx until the subsequent tokio-postgres migration. No higher request
throughput or lower Neon compute usage is claimed from changing startup alone.

## Configuration and lifecycle

Existing environment names remain valid. `WEBDOCK_DATABASE_URL` is validated
before connecting and must authenticate as the restricted
`webdock_auth_bootstrap` login. Production retains verified TLS/target policy.
`NODE_ENV=production` and the existing `LOCO_ENV=production` compatibility alias
both enforce production rules. Connection strings/config objects are not logged.

The bootstrap pool retains 1–10 connections, a five-second connect limit and 30-second acquisition limit
and a 30-second idle timeout. Other service pool constructors and the exact router
remain unchanged. There is no new polling loop.

HTTP binds to `WEBDOCK_BIND_ADDRESS` (default `127.0.0.1`) and `PORT` (default
`3130`). Both raw and bracketed IPv6 bindings are accepted. SIGTERM and SIGINT listeners are registered before accepting traffic.
Startup initialization and binding are cancellable by those signals, including a
stalled database connection. Shutdown drains in-flight requests for at most 20 seconds; if a held request/SSE
stream exceeds that grace, the process exits with a timeout rather than hanging.
Tokio executor teardown is additionally bounded to one second, including pending
blocking resolver tasks.
Allow at least 30 seconds in the surrounding container/service termination policy.
The Kubernetes default 30-second grace accommodates this bound.

JSON tracing defaults to `info`; `RUST_LOG` may select an operator-managed filter.
Do not add configuration/credential dumps for startup diagnostics. `/healthz`,
`/readyz`, API, OAuth/MFA, SSE and UI routes retain their existing contracts.

## Verification

```sh
cargo test --locked -p webdock-control-plane --test native_runtime
cargo test --locked -p webdock-control-plane --lib --tests
cargo test --locked -p webdock-control-plane --test native_runtime_postgres -- --ignored
cargo clippy --locked -p webdock-control-plane --all-targets -- -D warnings
```

The PostgreSQL test provisions its own local PostgreSQL 18 container and restricted
roles. It starts the actual Core binary, checks readiness and unchanged migration
history, sends SIGTERM and confirms a successful exit. A second process proves
that a privileged runtime login is refused before serving traffic. No external
database URL, production identity or 1Password access is used.

Qualify an immutable release image and retain the prior digest before rollout.
Do not include uncommitted local schema drafts in the image source. Startup
refactoring is not permission to reinitialize an existing database.

References: [Axum serving](https://docs.rs/axum/0.8.9/axum/fn.serve.html) and
[Tokio Unix signal registration](https://docs.rs/tokio/latest/tokio/signal/unix/fn.signal.html).

Tokio documents why [blocking tasks need bounded runtime teardown](https://docs.rs/tokio/latest/tokio/runtime/struct.Runtime.html#method.shutdown_timeout).
