# Native OAuth authorization and consent

`WEBDOCK_OAUTH_GRANTS_BACKEND=tokio-postgres` opts authorization request creation,
consent details and consent decisions into direct PostgreSQL. The default remains
`sea-orm`; unknown values fail startup. Core also selects native security writes,
coordinated IDs, cookie reads and OAuth introspection/client reads, reusing the
existing shared max4/min0 authentication pool and allocator. Manually constructed
services need matching native cookie reads/allocator. Browser sidecar authority
has its separate opt-in and can be combined with this capability.

The existing OAuth engine still owns PKCE, redirect negotiation, scope validation
and one-use request state. The native writer persists the same hashed request ID,
context, canonical subject/session/client and five-minute database expiry, so the
retained code exchange remains interoperable. Login/prompt/max-age, nonce/state,
issuer query parameter, consent scope/resource containment and denial redirects
are preserved. OAuth scopes are sets; their response-array order was already
unspecified. Tenant-preview sessions cannot issue new grants.

Canonical authority is derived from signed cookies, then revalidated within the
writer. Client locks precede canonical user/session and request/resource locks.
Consent decisions serialize their canonical owner before consuming a request.
Discovery must match the current subject/session/client, immutable payload and
expiry after waits. Provider, tenant, billing and resource checks remain current;
canonical and policy checks run again after waits and pending writes. A consent
request that expires while a pending write waits is rolled back, including its
consumption and any new consent/code. No successful response precedes confirmed
commit and there is no retained retry/fallback on uncertainty.

Authenticated operations acquire canonical authority before resource locks to
avoid reversing parent/cascade order. When canonical and resource failures race,
the current permission failure can precede its secondary resource error. Normal
protocol validation and unauthenticated resource checks remain intact.

Expired-request cleanup retains its100-record bound and skips locked rows, so a
request does not wait behind another user's expired record. Approval replaces the
client/user consent with exactly the approved scopes/resources and creates one
new code. Denial consumes the request, preserves original state/error/issuer, and
creates no grant. Parallel approvals cannot consume the same request twice.

Owned operations are bounded to30seconds and use existing checked transaction,
lock/acquisition budgets and cancellation cleanup. Native allocator reservations
can leave gaps after a later failure; IDs are not reused. No schema, grants,
keys, credentials, provider settings, production defaults or public-build
admission change. `app_binding` remains SELECT-only for bootstrap, with existing
binding/provider/policy snapshot limits; claims alone do not replace domain
permissions or signed tenant RLS.

Code exchange, token/refresh persistence, private signing and remaining retained
caller-owned/domain paths still await their ports. This flag does not claim full
OAuth/driver migration, production activation or a measured latency/Neon compute
improvement.

## Local verification

Docker runs exclusively owned loopback PostgreSQL18 fixtures; no production
connection, live provider or interactive credential is needed.

```sh
CARGO_BUILD_JOBS=2 cargo test --locked -p spitzli-oauth --test native_introspection --test native_clients -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p spitzli-browser-session --test native_store -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p spitzli-control-plane --test native_runtime --test native_runtime_postgres -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p spitzli-oauth -p spitzli-browser-session -p spitzli-identity -p spitzli-control-plane --lib --tests -- --test-threads=2
CARGO_BUILD_JOBS=2 cargo clippy --locked -p spitzli-oauth -p spitzli-browser-session -p spitzli-identity -p spitzli-control-plane --all-targets -- -D warnings
cargo fmt --all -- --check
cargo run --locked --manifest-path tools/quality/Cargo.toml -- contracts "$PWD"
```

Tests use real OAuth/PKCE with a disconnected retained grant writer, covering
approval/details parity, scope-set containment, rejection/state/issuer, exact
identity/payload binding, replay/parallel decisions, expiry after waits and
pending writes, rollback, bounded nonblocking cleanup and preview refusal. The
actual Core HTTP flow audits a native code insertion through login/authorization/
callback/profile and revocation, while retained exchange/signing remain explicit.
Owned test-only failure/delay triggers are not application migrations or runtime
hooks. Startup/shutdown preserve account counts and migration history.
