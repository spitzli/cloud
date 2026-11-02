# Native database gateway

The gateway is a Rust executable using Axum, Tokio, Reqwest, Rustls and Tungstenite. It preserves the existing `/session` and `/s/{scopeID}/api/v1/*` browser contract and the canonical Auth-host `/api/databases/gateway` authorization endpoint. The existing pinned Rust database-browser runtime remains responsible for database drivers, SQL execution, schema browsing and its frontend assets.

Every browser request and event frame revalidates the current server-side grant. Scoped HttpOnly/SameSite=Strict cookies contain only opaque handles. Saved connection identifiers are translated at the boundary; credentials and host details are removed from public connection DTOs. SQL text is not used as an authorization heuristic: the isolated runtime's actual PostgreSQL role enforces SQL privileges. Arbitrary runtime configuration, uploads, plugins, host-file browsing and unknown RPCs are denied.

I/O and concurrency are bounded. Query execution has deadlines and explicit cancellation on revocation, timeout, disconnect or shutdown. Ordered WebSocket forwarding preserves event order and translates connection IDs. Managed runtime error details redact their confidential connection material. HTTP retries and redirects are disabled. Request bodies are bounded before handler extraction and have a finite read deadline.

Managed PostgreSQL uses an isolated process and mode-0700 directory per session/identity/revision, with mode-0600 saved connections and verified database TLS. Concurrent acquisitions share a single owned start task; cancelling one waiter does not create a second process. Expiry, revocation and shutdown stop the owned process and remove its own temporary state. Registered PostgreSQL/SQLite runtimes retain their separate explicit provisioning and grant contracts.

Run with the environment described in `apps/database-gateway/.env.example`:

```sh
cargo run -p webdock-database-gateway
```

The executable does not load environment files implicitly. `TABULARIS_EXPECTED_COMMIT` must match the retained stamped runtime. Optional `WEBDOCK_DATABASE_RUNTIME_CA_FILE` supplies an operator-owned public CA bundle for private HTTPS/WSS runtime and Auth endpoints; the old `NODE_EXTRA_CA_CERTS` name is a bounded configuration alias. Hostname verification stays enabled. Managed PostgreSQL uses its separately authorized database CA, not this gateway trust bundle.

Build from the repository root with `deploy/docker/database-gateway.Dockerfile`. The image's entrypoint is Rust; no first-party Node process is installed or started. Compose health checks use the native `--healthcheck` option. Kubernetes runs the gateway without a service-account token and with a read-only root filesystem, dropped capabilities and private temporary storage.

Verification includes Rust contract/HTTP/WebSocket tests, real private-CA HTTPS and WSS (unknown CA and wrong hostname denied), a real managed process concurrency/cleanup test, and the existing PostgreSQL acceptance suite against both the native binary and the restricted native image. That suite executes schema/data edits over verified TLS, rejects another connection ID and an unrelated database CA, and removes session state on revocation. All database/container fixtures are disposable and use synthetic credentials.

The Core database binding/viewer authority is still being ported separately; this gateway can use its retained compatible endpoint during that work. Managed database provisioning/helpers, browser integration and complete platform migration remain separate parity gates. This component is not a claim that the complete Webdock rewrite is finished.
