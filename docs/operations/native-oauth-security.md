# Native OAuth revocation and logout

`WEBDOCK_OAUTH_SECURITY_BACKEND=tokio-postgres` opts token revocation and
end-session into direct PostgreSQL. Default `sea-orm` remains unchanged; unknown
values fail startup. Core also selects native coordinated IDs, cookie session
reads and OAuth introspection/client/public-key reads. It reuses the existing
shared max4/min0 authentication pool and existing coordinated allocator. Manually
constructed services must supply the matching native cookie reader and allocator.
No schema, grants, keys, credentials or public-build admission change.

Revocation authenticates the same client secret/method, discovers only that
client's matching access/refresh records and verifies legacy JWT claims using
the shared RS256/public-key/issuer/time checks. It acquires client, sorted
canonical owner-exclusive locks, session locks and token locks in deterministic
order before reloading immutable ownership. Owner-exclusive serialization matches
refresh and account/session deletion. Disappearing records can remain an
idempotent unknown-token result; new/replaced ownership after discovery fails
closed. Only confirmed updates return success, without retained fallback/retry.

Valid own-client legacy access JWTs receive revoked access-record tombstones.
Their canonical subject must exist and have been fenced; a subject appearing
during another owner's lock wait cannot be used without a fence. Session IDs are
stored only when they belong to that subject, otherwise the retained NULL-session
behavior is preserved. Scope/resource/time fields and client-guarded conflict
handling remain intact. Invalid/unknown/foreign tokens do not mutate another
client's records. ID-token revocation does not implicitly end a canonical session.

Logout still accepts an expired signed ID-token hint, validates enabled client
end-session configuration and an exact post-logout redirect allowlist, and bounds
state to2048bytes. It requires the current signed cookie and hint to identify the
same canonical subject/session. The owner is serialized before revalidation and
exact session deletion, preserving cascade order and avoiding simultaneous
logout lock upgrades. Setup-only sessions can log out. Missing/mismatched proof
requires confirmation; the session-clear cookie and completion redirect are
returned only after confirmed deletion. No request generates or rotates keys.

Native owned operations are bounded to30seconds and use the existing checked
pool/transaction/lock budgets and cancellation cleanup. Failure after a pending
update rolls the security transaction back. Coordinated IDs may be reserved even
when a subsequent operation fails; gaps are permitted and IDs are never reused.
Existing provider/binding policy snapshot limits remain. Caller-owned retained
APIs, consent/code/token issuance, refresh and private signing-key operations
remain explicit until ported. This opt-in is not full driver removal, production
activation or a measured latency/Neon compute improvement.

## Local verification

Use Docker for exclusively owned loopback PostgreSQL18 fixtures. No live provider,
production database or interactive credential is needed.

```sh
CARGO_BUILD_JOBS=2 cargo test --locked -p spitzli-oauth --test native_introspection --test native_clients -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p spitzli-browser-session --test native_store -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p spitzli-control-plane --test native_runtime --test native_runtime_postgres -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p spitzli-oauth -p spitzli-browser-session -p spitzli-identity -p spitzli-control-plane --lib --tests -- --test-threads=2
CARGO_BUILD_JOBS=2 cargo clippy --locked -p spitzli-oauth -p spitzli-browser-session -p spitzli-identity -p spitzli-control-plane --all-targets -- -D warnings
cargo fmt --all -- --check
cargo run --locked --manifest-path tools/quality/Cargo.toml -- contracts "$PWD"
```

Tests issue actual OAuth codes/access/refresh tokens, then perform security writes
with a disconnected retained database handle and native allocator. They cover
revocation/tombstones, foreign client/invalid credentials, missing-owner races,
immutable changes, cancellation, rollback after an update, exact logout identity,
redirects, expired hints and simultaneous/setup-only logout. Actual Core HTTP
observes the native owner fence during revocation before checking the token is
inactive; startup/shutdown preserve account counts and migration history.
