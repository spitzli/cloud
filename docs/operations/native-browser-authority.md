# Native browser session authority

`WEBDOCK_BROWSER_AUTHORITY_BACKEND=tokio-postgres` opts browser reference reads,
revalidation and canonical principal checks into direct PostgreSQL. The default
remains `sea-orm`; unknown values fail startup. It implies native browser flow
storage, OAuth introspection and OAuth client/public-key reads. Core requires its
existing first-party SSO configuration and reuses the same max4/min0 bootstrap
pool. No schema, grants, signing/cookie keys or credentials change.

The signed browser cookie still contains a random reference, never OAuth tokens.
The reader validates the exact first-party client and origin, discovers and
decrypts the protected sidecar, then obtains current OAuth/canonical identity
proof. It locks client → user → session → token before the browser sidecar.
Canonical session deletion cascades to browser records, so taking the sidecar
first could invert revocation locks. After the sidecar lock, ownership and
encrypted payload must still match discovery; disappearance/replacement cannot
reuse the earlier proof. Provider/OAuth identity and expiry are rechecked after
waits. A final expired bound is rejected by the shared pure finisher in both
native and retained modes.

Pure claim/principal and role/scope/read-only/customer ceiling rules are shared
with the retained reader. Exact subject/session/client matching, MAC validation,
origin binding, disabled/revoked authority and current canonical/provider/tenant
permissions remain authoritative. Operator promotion cannot escape a saved
customer ceiling. Preview mode changes invalidate the old exact opaque
customer scope. Within the same live scope, a saved read-only browser authority
retains its read-only ceiling. The bounded retained-cookie bridge keeps its
original envelope/payload/transition deadlines and adds no longer-lived authority.

`BrowserSessions::revalidate_native_in` consumes a checked native transaction
and returns it with the authentication result. `revalidate_stored_authority_native_in`
does the same for trusted server-owned LocalSession, CustomerSession and
FirstPartyBrowser proofs. First-party browser validation precedes outer canonical
reads to preserve client-first ordering. Caller transactions retain identity,
client, token and sidecar fences until domain completion. Stored IDs/references
are not authentication input and must never be accepted from an unauthenticated
request. Domain authorization and signed tenant RLS still apply; callers must
preserve the stored read-only restriction when acting on a returned principal.

Owned operations and consuming revalidation are bounded to30seconds, with the
existing checked transaction/lock/acquisition budgets and cancellation cleanup.
Uncertainty fails closed without a retained retry/fallback. Existing binding and
provider/policy snapshot limits remain; `app_binding` keeps SELECT-only bootstrap
privileges. Claims alone are not lasting resource authorization.

Existing retained transaction-aware browser APIs and domain consumers remain
explicit pending their ports. OAuth code/token issuance, refresh, revocation and
logout authority still use retained operations. This opt-in adds fresh proof
checks; it does not claim lower request latency, fewer Neon compute hours, full
driver removal or production activation. Public build admission is unchanged.

## Local verification

Use Docker for exclusively owned loopback PostgreSQL18 fixtures; no provider
account, production database or interactive credential is required.

```sh
CARGO_BUILD_JOBS=2 cargo test --locked -p webdock-browser-session --test native_store -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p webdock-oauth --test native_introspection --test native_clients -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p webdock-control-plane --test native_runtime --test native_runtime_postgres -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p webdock-oauth -p webdock-browser-session -p webdock-identity -p webdock-control-plane --lib --tests -- --test-threads=2
CARGO_BUILD_JOBS=2 cargo clippy --locked -p webdock-oauth -p webdock-browser-session -p webdock-identity -p webdock-control-plane --all-targets -- -D warnings
cargo fmt --all -- --check
cargo run --locked --manifest-path tools/quality/Cargo.toml -- contracts "$PWD"
```

Tests obtain actual signed cookies through internal OAuth/PKCE and inspect them
with a disconnected retained reader. They cover tamper/origin/duplicate refusal,
current token and ceiling checks, caller-held fences, replacement/deletion,
expiry while waiting in both modes, cancellation/timeouts, canonical revocation
lock order, operator preview/customer/read-only ceilings and the bounded legacy
bridge. Actual Core HTTP performs login → authorize → callback → signed cookie →
native session read under an observed sidecar lock → profile → revoked sidecar.
Migration and account counts remain unchanged across process startup/shutdown.
