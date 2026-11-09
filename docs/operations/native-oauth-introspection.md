# Native OAuth token introspection

Set `WEBDOCK_OAUTH_INTROSPECTION_BACKEND=tokio-postgres` to opt owned standard,
first-party Portal and MCP introspection, plus UserInfo, into direct PostgreSQL.
The default remains `sea-orm`; unknown values fail startup. This also selects the
native client/public-key reads and reuses Core's shared max4/min0 native bootstrap
pool. Other native flags may be combined without adding another authentication
pool. This change adds no migrations, credentials, signing keys or grants.

Opaque access and refresh tokens use parameterized typed queries. Discovery
precedes client → user → session → token ownership locks; immutable ownership is
compared with a fresh canonical read. Inactive discovery stays inactive. A
missing/replaced discovered access record cannot fall back to JWT compatibility.
Refresh rotation and replay are still performed by the existing issuer; their
revocation is observed by the native reader.

RS256 compatibility tokens share the existing public signature, issuer and time
checks. ID tokens cannot become access tokens. Current client, canonical session,
provider credentials/MFA, tenant membership, application binding, CMS billing and
project grants remain authoritative; embedded role claims do not grant access.
The first-party Portal alone retains the narrow operator tenant-preview behavior.
Public introspection, MCP and UserInfo cannot use that preview exception.

Resource policies and issuer/introspector grant sets are locked in deterministic
order. Canonical identity and permission checks run again after resource waits;
token expiry is checked again before returning active. Native caller-owned APIs
`introspect_native_in` and `introspect_portal_native_in` consume a checked native
transaction and return it with the claims, retaining client, identity, token and
resource/grant fences through domain completion. Claims alone are not lasting
resource authorization: the domain must retain its transaction and apply its
existing authorization/RLS checks.

Application bindings and membership/CMS policy reads preserve existing privileges
and snapshot semantics. In particular `app_binding` remains SELECT-only for the
bootstrap role, without an invented update grant to obtain row locks. Resource
and application actions must still enforce their own current permissions.

Owned operations and consuming introspection are bounded to30seconds; the checked
pool also retains its lock/acquisition/transaction budgets and cancellation
cleanup. Errors and uncertain completion fail closed without retained fallback.
JWT revocation tombstones are checked again after waits; a concurrent tombstone
may still arrive after a completed inspection, as with other subsequent
revocations. Existing caller-owned retained `_in` APIs, browser revalidation and
MCP authority guards keep their explicit retained transaction boundary until
ported. Token/code issuance, refresh, revocation writes, consent and logout remain
retained paths. This is not the complete OAuth driver migration, a production
activation or a measured performance/Neon compute improvement.

## Local verification

Docker must be available for exclusively owned loopback PostgreSQL18 fixtures.
No provider account, production database or interactive credential is needed.

```sh
CARGO_BUILD_JOBS=2 cargo test --locked -p webdock-oauth --test native_introspection --test native_clients -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p webdock-control-plane --test native_runtime --test native_runtime_postgres -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p webdock-oauth -p webdock-browser-session -p webdock-identity -p webdock-control-plane --lib --tests -- --test-threads=2
CARGO_BUILD_JOBS=2 cargo clippy --locked -p webdock-oauth -p webdock-control-plane --all-targets -- -D warnings
cargo fmt --all -- --check
cargo run --locked --manifest-path tools/quality/Cargo.toml -- contracts "$PWD"
```

Tests issue actual OAuth codes/access/refresh tokens using PKCE, then inspect with
a disconnected retained reader to prove native execution. They cover current
revocation, expiry after lock waits, replacement/disappearance, refresh replay,
provider credential/session invalidation, preview separation, tenant/project
permissions, real RS256 compatibility/tombstones, MCP cross-client grants,
caller-held fences and cancellation. Actual Core HTTP observes the native token
query under an owned table lock, then verifies active, UserInfo and revoked
responses without startup schema changes.
