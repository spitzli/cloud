# Native browser login-flow storage

`SPITZLI_BROWSER_FLOW_BACKEND=tokio-postgres` opts durable first-party browser
sidecar storage into the checked native PostgreSQL runtime. The default remains
`sea-orm`; unknown values fail startup. Native mode requires the existing
first-party SSO client configuration. It uses one max4/min0 native bootstrap pool,
shared with native identity sessions when those capabilities are enabled. It does
not create another identity pool or activate production by itself.

The native domain operations persist encrypted login/logout flows, validate and
consume a one-use PKCE flow, write encrypted browser-session sidecars, remove the
matching sidecar on logout, read/consume logout confirmation and prune expired
sidecars. Opaque browser references remain hashed and payload encryption uses the
existing keys. No bearer/refresh token is returned by the storage boundary or
logged. The corresponding retained operations remain compatible.

PKCE, constant-time state, nonce, issuer/client/origin, ID-token/access-token hash,
role/scope/expiry ceilings and canonical-session checks remain at their established
authorities. OAuth client/code/token operations and caller-owned browser
reference/introspection/revalidation transactions remain explicit retained paths.
This storage mode is not the completed OAuth/identity driver migration and does
not skip those checks.

Flow state and origin are verified while the row is locked. Wrong state, client
or origin leaves the valid flow unconsumed; successful consumption is confirmed
before code exchange. The final DELETE also checks current expiry, origin and
client, so a flow expiring during a lock wait cannot complete. The retained
callback receives the same final-consumption expiry/row-count guard. Parallel
callbacks yield one success. Cancellation before the consumption COMMIT discards the transaction without
consuming the nonce or exhausting a one-connection pool. Once COMMIT starts, a
lost acknowledgement can leave the nonce consumed; the request returns no success,
discards the connection and never falls back. Restart sign-in after an unknown
outcome rather than assuming the previous nonce remains usable. Logout confirmation
remains bound to the exact current canonical user/session and is single-use.

Native operations use a 30-second budget and fail closed without a retained
fallback. Expired housekeeping keeps the existing bounded per-table limit and
SKIP LOCKED behavior, never removing active authority. No background SQL polling
is introduced. No schema, encryption-key rotation or provider network dependency
is required; existing role and verified-TLS/file-authority checks still apply.

```sh
cargo test --locked -p spitzli-browser-session --test native_store -- --include-ignored --test-threads=1
cargo test --locked -p spitzli-control-plane --test native_runtime --test native_runtime_postgres -- --include-ignored --test-threads=1
cargo clippy --locked -p spitzli-browser-session -p spitzli-control-plane --all-targets -- -D warnings
cargo fmt --all -- --check
```

Owned loopback PostgreSQL18 tests use the actual internal OAuth/PKCE/OIDC engine,
server-side client credentials and cryptography. They cover encrypted persistence,
wrong-state/client/origin refusal, parallel replay, expiry during row-lock wait,
pruning, owner-bound logout and retained compatibility. The actual Core HTTP test
checks the established 303 login redirect, PKCE/state/nonce parameters, HttpOnly
cookie and a native backend write audit. No external provider or live credentials
are used. These are correctness/security gates, not a latency or Neon CU-hour
improvement claim.

The [parent Core security rollout](../verification/2026-11-09-native-core-security-rollout.json)
ships source a403 and keeps native backend flags disabled. It is a different
artifact from this subsequent browser-store implementation.
