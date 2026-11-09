# Native account recovery

`WEBDOCK_ACCOUNT_RECOVERY_BACKEND=tokio-postgres` opts the four owned recovery
operations into direct SQL: reset request, account activation, delivery dispatch
and local reset completion. The default remains `sea-orm`; unknown values fail
startup. Core reuses its bounded max 4/min 0 bootstrap pool and enables the native
durable ID allocator. Other enabled identity/OAuth capabilities are preserved.
This source increment does not activate the flag in production.

Recovery uses the existing restricted `webdock_auth_bootstrap` role and migrations
through version 75. It changes no schema, privileges, keys or credentials. Native
execution requires the existing canonical identity/provider policy functions;
missing state fails closed without an ORM fallback.

## Authority and durability

The canonical user row is locked before proof admission or consumption. Requests
for an unknown or banned local account still acknowledge without creating a job.
Concurrent local requests are throttled to one per account per minute. A managed
mapping uses the fixed configured Neon reset endpoint; provider failure never
creates a local reset. Disabled mappings also cannot silently become local
identities. Local activation refuses a managed mapping rather than creating a
link that local reset cannot accept.

The mapping lookup uses SELECT-only access. The canonical user lock prevents a
new mapping's foreign-key insertion from racing local admission; it does not
require UPDATE privileges on immutable Neon binding metadata.

Local proof is bound to the current email and exact credential digest. Reset
rechecks local/provider authority and expiry after credential-lock waits and hashing,
atomically consumes the
proof, changes the password, retires that user's sessions/MFA challenges and
removes outstanding recovery proof/jobs. MFA configuration and passkeys are not
disabled. Other users' sessions remain intact. No session or cookie is granted by
reset itself; the user signs in through the existing first/second-factor flow.

Only the token digest is stored as proof. Delivery payloads remain encrypted with
the existing key ring, and token/job admission commits together. Activation's
one-use URL is returned only through the existing trusted provisioning boundary.
Never expose decrypted jobs, proof URLs or diagnostic database rows publicly.

Dispatch retains stable delivery IDs, SKIP LOCKED claims, a 60-second lease,
five-attempt ceiling, bounded provider calls and generation-fenced receipts. It
releases its database connection before delivery, including with a max 1 pool.
The sink must deduplicate by request ID: delivery remains at-least-once. A stale
or unconfirmed receipt returns an error; it never proves that an external side
effect did not happen. A crashed/canceled worker leaves its lease for recovery.

Each native DB phase has a 30-second budget and uses existing checked transaction
cleanup. Cancellation, failed commit or uncertain persistence never retries
through the retained connection.

## Local qualification

Use an explicit clean source archive excluding unrelated local draft migrations.
The test harness provisions and removes owned loopback PostgreSQL 18 containers
with the actual restricted roles; it needs local Docker access. No production
database URL, provider key or 1Password session is needed.

```sh
CARGO_BUILD_JOBS=2 cargo test --locked -p webdock-identity --test native_recovery -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p webdock-control-plane --test native_runtime --test native_runtime_postgres -- --include-ignored --test-threads=1
cargo test --locked -p webdock-identity -p webdock-browser-session -p webdock-oauth -p webdock-control-plane --lib --tests
cargo clippy --locked -p webdock-identity -p webdock-control-plane --all-targets -- -D warnings
cargo fmt --all -- --check
cargo run --locked --manifest-path tools/quality/Cargo.toml -- contracts "$PWD"
```

The native recovery fixture deliberately disconnects the retained database. The
actual Core HTTP case uses the Console facade, verifies native SQL execution,
server-owned redirect and public acknowledgement, and completes/replays a reset
using only its owned fixture's encrypted delivery job. A subsequent sign-in with
the new password must still require the retained MFA challenge. It does not send real mail
or contact Neon, and does not qualify a real customer's provider account/login.

Password sign-in, MFA/passkey ceremonies and other identity/domain stores remain
separate ports. Production rollout, complete customer E2E and measured general
latency/Neon compute improvements are not implied by this local qualification.
