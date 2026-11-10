# Native OAuth token exchange and refresh

`WEBDOCK_OAUTH_TOKENS_BACKEND=tokio-postgres` opts code exchange, token persistence,
refresh and private signing reads into direct PostgreSQL. Default `sea-orm` stays
unchanged; unknown values fail startup. Core composes native grants/security,
coordinated IDs, cookie/OAuth/client/public-key reads using the existing shared
max4/min0 authentication pool and allocator. Browser sidecar authority remains a
separate compatible opt-in. Manually constructed services need matching native
cookie reads and coordinated allocator.

The existing OAuth engine still owns client/PKCE/redirect negotiation, scope and
refresh semantics. Code discovery binds exact client and immutable request
subject/session/payload/expiry; canonical owner/session precede the code lock.
The writer reloads live state after waits, applies current resource TTL minima,
revalidates canonical/provider/MFA/tenant/billing authority, and persists the same
hashed opaque access/refresh records and ID-token claim format. Code consumption
and all token writes commit together before credentials are returned. An invalid
verifier or failed signing leaves the code available for a valid retry.

Refresh acquires the canonical owner exclusively before session/refresh locks,
compares immutable discovery, and checks current identity plus original scope
ceilings and resource TTLs. It atomically rotates the old refresh and persists
replacement tokens. A rotated-token replay revokes the same client/user family
and confirms that commit before returning `invalid_grant`, including parallel
replay. Scope, session, ownership or expiry failures never rotate a fresh record.
Canonical and old code/refresh deadlines are rechecked after pending writes;
late failures roll back consumption/rotation and all new token rows.

Only offline-provisioned encrypted signing keys are used. Native selection is
deterministic, fences the chosen key and reloads encrypted material/eligibility
after key-lock waits. The key remains fenced and its clock expiry is checked again
before commit. Pure ID claims and encrypted RS256 signing are shared with the
retained backend: issuer/audience/subject/session, nonce, auth_time, at_hash,
profile/email and encryption envelope are preserved. Private material never
leaves the signer. Non-OpenID public PKCE clients need no private key; requests
never generate or rotate keys.

Owned operations are bounded to30seconds, with existing checked-role/TLS,
transaction/lock/acquisition budgets and cancellation cleanup. There is no
credential success on an uncertain commit, automatic retry or retained fallback.
Native ID reservations may leave gaps after rollback; IDs are never reused.
Existing binding/provider/policy snapshot limits remain; `app_binding` stays
SELECT-only. Domain permissions and signed tenant RLS remain authoritative.

This capability completes the owned OAuth HTTP lifecycle. Offline provisioning
and retained caller-owned validation/introspection/MCP/domain consumers remain
explicit until their ports, as do other identity/product stores. It does not
claim full-platform driver removal, production activation, public build readiness
or a measured latency/Neon compute improvement. No schema/grants/keys/credentials
or provider/default/public-build settings change.

## Local verification

Use Docker for exclusively owned loopback PostgreSQL18 fixtures. No production
connection, live provider or interactive credential is required.

```sh
CARGO_BUILD_JOBS=2 cargo test --locked -p spitzli-oauth --test native_introspection --test native_clients -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p spitzli-browser-session --test native_store -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p spitzli-control-plane --test native_runtime --test native_runtime_postgres -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p spitzli-oauth -p spitzli-browser-session -p spitzli-identity -p spitzli-control-plane --lib --tests -- --test-threads=2
CARGO_BUILD_JOBS=2 cargo clippy --locked -p spitzli-oauth -p spitzli-browser-session -p spitzli-identity -p spitzli-control-plane --all-targets -- -D warnings
cargo fmt --all -- --check
cargo run --locked --manifest-path tools/quality/Cargo.toml -- contracts "$PWD"
```

Owned tests use a disconnected retained token writer and actual native PKCE/JWT
verification, covering one-use and invalid proof, rotation/replay family commit,
MCP TTL/scope ceilings, current revocation, immutable/expired/canceled requests,
corrupt-key rollback, key eligibility after waits and signatures, code/refresh
late deadlines, and public non-OpenID issuance with no key. Actual Core HTTP audits
native code/access/refresh inserts through login/authorization/exchange/signing/
callback/profile/refresh and revocation. Fixture-only failure/delay triggers are
not application migrations or runtime hooks. Account/journal state is unchanged
across startup/shutdown; explicit test actions mutate only their owned records.
