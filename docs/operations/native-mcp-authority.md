# Native MCP authority references

`WEBDOCK_MCP_AUTHORITY_BACKEND=tokio-postgres` opts the owned bearer-to-MCP
reference boundary into direct PostgreSQL. Default `sea-orm` stays unchanged;
unknown values fail startup. It selects native client/introspection/public-key
reads using Core's shared max4/min0 bootstrap pool. The capability adds to an
already enabled native OAuth lifecycle and preserves its write capabilities.
No schema, grant, key, credential, provider or production/default change occurs.

A durable MCP reference requires an exact active access-record row. A bare legacy
JWT, ID token, refresh token, cached role or raw identity cannot create it. The
native reader validates client credentials, current caller/issuer linkage,
canonical/provider/MFA/tenant proof, resource/grant policy, scope namespace,
role/write ceiling and audience. MCP access lifetimes remain at most300seconds.
Preview authority is never admitted. Fingerprints share retained JSON/hash
normalization, including code/refresh linkage and original timestamps.

`authorize_mcp_reference_native_in` consumes a checked native transaction and
returns it with `McpAuthentication`. `revalidate_mcp_authority_native_in` does the
same for a trusted server-owned stored reference and current principal. It checks
exact access ID/hash/fingerprint, caller/issuer/subject/session/resource, original
role/write/iat/exp and current policy. Client/canonical/access/resource/grant
fences remain held in the returned transaction through domain completion. Current
expiry and canonical proof are checked after waits. Cancellation/uncertainty
fails closed without a retained retry/fallback; owned/consuming operations are
bounded to30seconds with existing checked transaction/lock/acquisition cleanup.

Stored references are server-owned proof, not authenticated request DTOs. Domains
must keep the transaction and apply their own current permission checks and
signed tenant RLS. Claims alone are not durable authorization. Existing binding
and provider/policy snapshot limits remain; `app_binding` stays SELECT-only.

This change does not convert the existing retained `McpGuard`, registry/hosting/
plugin/database consumers or their retained transactions. Those remain explicit
until ported. Native transactions borrow their checked connection; callers keep
that connection and returned transaction in scope rather than creating an unsafe
self-reference, another transaction manager or a second connection replacing
caller-owned proof. No full-MCP/domain driver migration, production activation,
public-build readiness or performance/Neon compute gain is claimed.

## Local verification

Docker runs exclusively owned loopback PostgreSQL18 fixtures. No production
connection, live provider or interactive credential is required.

```sh
CARGO_BUILD_JOBS=2 cargo test --locked -p webdock-oauth --test native_introspection --test native_clients -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p webdock-browser-session --test native_store -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p webdock-control-plane --test native_runtime --test native_runtime_postgres -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p webdock-oauth -p webdock-browser-session -p webdock-identity -p webdock-control-plane --lib --tests -- --test-threads=2
CARGO_BUILD_JOBS=2 cargo clippy --locked -p webdock-oauth -p webdock-browser-session -p webdock-identity -p webdock-control-plane --all-targets -- -D warnings
cargo fmt --all -- --check
cargo run --locked --manifest-path tools/quality/Cargo.toml -- contracts "$PWD"
```

Tests mint actual native MCP access tokens with a disconnected retained reader,
compare retained fingerprints and old-guard interoperability, prove caller-held
fences, and exercise wrong proof/namespace/lifetime, immutable/tampered references,
current role/client/resource changes, linked-client boundaries, expiry and cancel.
Actual Core HTTP observes the native access-row fence at the bearer resource
gateway, then an authenticated routing refusal and revoked401 challenge. Domain
execution beyond that gateway remains outside this port's claim.
