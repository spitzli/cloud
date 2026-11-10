# Native password and MFA login

`SPITZLI_PASSWORD_LOGIN_BACKEND=tokio-postgres` selects direct persistence for the
owned password → optional MFA → canonical session flow. Core enables its required
native allocator, session reads and session management using the existing shared
max 4/min 0 bootstrap pool. Other capabilities, including native recovery, remain
independent and compose with it. Default `sea-orm` behavior is retained; invalid
values fail startup. No production activation is part of this source increment.

## What changes

- Durable normalized first-factor rate admission precedes login. Missing local
  accounts still perform the existing expensive password verification. No DB
  connection is held while waiting for local password workers or hashing.
- Canonical identity is locked before checking the current credential and local
  migration/provider authority. Setup, ban, verification and operator MFA gates
  retain their existing meaning. Ambiguous identities or credentials fail closed;
  a credential removed during verification retains the invalid-login result.
- MFA challenges retain their signed cookie, credential digest, five-minute TTL,
  five-attempt limit and durable factor failure counters. Recovery codes and TOTP
  steps are consumed atomically; expiry/time facts are refreshed after waits.
- Session issuance requires a confirmed commit, then releases the connection
  before rereading canonical authority, including max 1 pools. No bare user or
  session ID, partial provider login or MFA cookie becomes a business session.

Mapped accounts use the existing fixed HTTPS Neon transport, with its existing
certificate validation, no redirects/proxy, response bounds and timeout. The
native persistence path checks current mapping subject/origin/revision, actual
provider session facts and encrypted cookie custody. First-factor enrollment
retains exact local credential/approval locks and uses sign-up only; it never
silently adopts an existing provider account. Missing or unavailable Managed Auth
does not fall back to a local session. Provider evidence remains current through
step-up and final session issuance, including revocation while waiting.

The shared facts-to-provenance helper keeps the retained encrypted envelope and
credential fingerprint compatible. Existing MFA material, passkeys, cookie names,
canonical IDs and protocol audiences are retained. No schema, privileges, keys,
credentials or new libraries are introduced. Identity explicitly enables the
installed tokio-postgres JSON decoding feature rather than relying on features
enabled by other workspace packages.

## Qualification

Use a clean source archive containing committed migrations through version 75;
do not accidentally include an unrelated local draft migration. The owned test
harness needs local Docker and provisions restricted loopback PostgreSQL 18.
It does not need production credentials, 1Password or live provider access.

```sh
CARGO_BUILD_JOBS=2 cargo test --locked -p spitzli-identity --test native_password_login -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p spitzli-identity --lib native_login::tests -- --include-ignored --test-threads=1
CARGO_BUILD_JOBS=2 cargo test --locked -p spitzli-control-plane --test native_runtime --test native_runtime_postgres -- --include-ignored --test-threads=1
cargo test --locked -p spitzli-identity -p spitzli-browser-session -p spitzli-oauth -p spitzli-control-plane --lib --tests
cargo clippy --locked -p spitzli-identity -p spitzli-control-plane --all-targets -- -D warnings
cargo fmt --all -- --check
cargo run --locked --manifest-path tools/quality/Cargo.toml -- contracts "$PWD"
```

The public native login fixture disconnects the retained database. The private
provider-result fixture exercises the production persistence boundary using
synthetic verified provider results and real PostgreSQL provider facts. It does
not replace TLS or add a test-only production endpoint. It checks custody,
binding changes, provider revocation during step-up and password-worker/pool
cancellation. These are storage/authority tests, not a live Neon sign-in test.

The actual Core HTTP flow composes native recovery, password, MFA and session
handling. It verifies one-use reset and MFA proof, no canonical cookie before
MFA, an ordinary customer session, own-project access and foreign-project 404
through the existing Access/RLS boundary. Console-host admission remains enforced.

Passkey ceremonies, enrollment/security-management operations, administrative
provisioning and remaining identity/domain stores still retain their current
persistence until their own ports. Live Neon HTTP/enrollment, real mail delivery,
production rollout and full-platform/customer E2E require separate qualification.
No general latency, throughput or Neon compute reduction is claimed here.
