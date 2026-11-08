# Native session management

`WEBDOCK_SESSION_MANAGEMENT_BACKEND=tokio-postgres` opts the Core into native
self-service session listing, renewal, own-session revocation, revoke-other and
logout. It also selects native cookie reads, using the same bounded max4/min0
pool. The older read-only flag remains supported. Unknown values fail startup;
all defaults remain `sea-orm`. No production activation, migration or credential
change is part of this increment.

The backend retains the established contract: setup accounts may manage their own
sessions without gaining tenant/operator access; foreign session IDs/tokens never
authorize deletion; revoke-other also clears that user's MFA challenges. Renewal
keeps the token and session ID, refreshes at most once per 15 minutes and sets the
same eight-hour expiry when due. The checked connection is released before the
canonical Principal is reread, including with a one-connection pool. Invalid
cookies are rejected before acquiring a database connection.

Mutation authorization acquires canonical user/session row locks before checking
local/provider authority in a fresh statement. Native transactions explicitly use
READ COMMITTED, so a role/server default of REPEATABLE READ cannot hide revocation
committed while the request was waiting. Canonical expiry is checked again in the
fresh post-lock statement; renewal also predicates its UPDATE on current validity.
The retained locked-actor path receives the same expiry/preview refresh security
fix, so a cookie expiring during a lock wait cannot be renewed or used to list
sessions in either backend. Valid setup-session behavior remains unchanged.
The consuming trusted-backend
`authenticate_session_native_in` boundary similarly retains SHARE locks for the
caller, then reads complete current Principal/acting/policy facts in a new
statement. Bare subject/session IDs are not authentication proof. Callers still
need verified request provenance, tenant fences, an operation budget and confirmed
transaction completion; SQL independently checks live tenant authority.

Operations have a 30-second client budget; cancellation/uncertain completion drops
unsafe connections, and no unconfirmed mutation is reported as successful. Logout
revokes locally before a best-effort provider sign-out within the remaining budget.
Expired/disabled provider authority does not prevent own-cookie logout. Existing
encrypted-cookie custody validation is shared with the retained path: user,
provider subject and provider session must match before forwarding a provider
cookie. Credentials/diagnostics are never logged. Invalid custody still fails
closed as in the retained path.

Qualification uses exclusively owned local PostgreSQL18 fixtures. It includes
ownership/setup parity, renewal throttling, challenge cleanup, canceled mutation,
saturated-pool invalid cookies, wrong/default isolation, provider revocation during
row-lock wait, fresh acting mode after SHARE-lock wait, retained canonical locks,
local logout/provider denial and actual operator MFA/passkey interoperability.
The real Core HTTP test observes the native canonical query under an owned table
lock, then checks list/revoke/logout responses and surviving foreign sessions.
No live provider or production data is used.

```sh
cargo test --locked -p webdock-identity --test native_session_management -- --include-ignored --test-threads=1
cargo test --locked -p webdock-identity --test native_sessions -- --include-ignored --test-threads=1
cargo test --locked -p webdock-control-plane --test native_runtime --test native_runtime_postgres -- --include-ignored --test-threads=1
cargo clippy --locked -p webdock-persistence -p webdock-identity -p webdock-control-plane --all-targets -- -D warnings
cargo fmt --all -- --check
```

Issuance, password/MFA/passkey storage, browser/OAuth flow persistence and product
services still have explicit retained paths pending their subsequent ports. This
is not the completed database rewrite or a performance/Neon compute-cost claim.
Disable management to return to retained session writes; the read-only flag can
remain enabled independently. A normal Core restart applies the selected mode.
