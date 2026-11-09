# Native OAuth client and public-key reads

`WEBDOCK_OAUTH_CLIENT_BACKEND=tokio-postgres` opts first-party client validation
and public JWKS reads into direct PostgreSQL. Default `sea-orm` remains unchanged;
unknown values fail startup. Core reuses the same bounded max4/min0 native
bootstrap pool as identity/browser capabilities when combined. No extra identity
pool, key rotation, schema change or new credentials are required.

Native client validation decodes the same domain configuration and shares the
existing constant-time client-secret verifier. Exact redirect allowlists,
auth-method matching, disabled/DPoP/subject-type refusal, first-party purpose,
enabled organization-free binding and required openid/profile/email scopes are
retained. The canonical client row is locked before its configuration is read in
a fresh statement. Changes committed while waiting are observed.

`validate_portal_client_native_in` consumes a checked native transaction and
returns it only after validation, retaining its canonical client fence until the
caller completes. Client validation is not user/bearer/resource authorization.
Binding reads preserve the existing bootstrap SELECT-only privileges: this port
does not widen them merely to obtain a binding row lock. As with the retained
contract, binding state can change after validation; token/grant/introspection
and resource authorization must perform their own current checks.

Public-key reads select only the public-key column and share retained JWK
normalization. RSA key ID/RS256/signing-use metadata is preserved, and private
parameters are removed even if an accidentally private JWK is stored in the public
column. Key generation, rotation and private signing-key reads are not part of
this runtime mode.

Owned native operations are bounded to30seconds, use checked role/TLS/transaction
scope and fail closed without a retained fallback. Existing caller-owned retained
validation/introspection APIs stay compatible. Token issuance, resource-policy
inspection and other OAuth persistence remain explicit retained paths pending
later ports; this is not a complete OAuth backend switch or performance claim.

```sh
cargo test --locked -p webdock-oauth --test native_clients -- --include-ignored --test-threads=1
cargo test --locked -p webdock-control-plane --test native_runtime --test native_runtime_postgres -- --include-ignored --test-threads=1
cargo clippy --locked -p webdock-oauth -p webdock-control-plane --all-targets -- -D warnings
cargo fmt --all -- --check
```

Owned loopback PostgreSQL18 tests use a disconnected retained DB handle to prove
native client/JWKS operations, wrong credentials/redirect/method and eligibility
refusal, public-only JWK normalization, caller-held client fences, fresh binding
changes after waits and unchanged binding mutation privileges. Actual Core HTTP
observes the native JWKS query under an owned table lock before checking its
public response. No external provider or production values are used.
