# Native session reads and signed tenant context

`WEBDOCK_SESSION_READ_BACKEND=tokio-postgres` opts the HTTP Core's canonical
cookie/session reads into the direct PostgreSQL driver. The default remains
`sea-orm`; unknown values fail startup. This flag is separate from the ID backend
flag. Password, MFA/passkey issuance, browser/OAuth flow storage and other identity
writes remain explicit retained paths pending their subsequent ports. This is not
the completed database rewrite and does not activate production automatically.

The reader uses the configured primary bootstrap URI and restricted
`webdock_auth_bootstrap` role. Its additional pool is bounded to four connections,
minimum zero, with 30-second local idle cleanup and no SQL background health
polling. Opt-in can therefore add up to four connections alongside the retained
pools. Existing verified TLS and URI/file-authority rules apply; unsupported URL
options fail instead of silently changing their meaning.

Each actual backend transaction verifies role and clean RLS scope. A parameterized
query reads the canonical session and invokes the established database authorities
`local_identity_authority` and `native_provider_session_live`. It retains expiry,
banned/setup/MFA state, provider binding/revision/session/credential revocation and
customer preview/acting fields. Missing policy functions, permission changes,
transport errors or unknown state fail closed; there is no fallback to retained
session reads after opting in. Native persistence failure is reported as service
unavailability through HTTP and dependent authority boundaries, without exposing
raw database diagnostics or treating the outage as a permanent permission loss.

The complete operation has a 30-second client budget, including acquisition,
queries and confirmation. Cancellation, failure or unconfirmed completion discards
the pooled connection. No cookie/token or configuration object is logged.

`RlsSigner::apply_native` consumes a checked native transaction and returns it only
after successful transaction-local context installation. The signer shares the
existing payload/HMAC generation with retained callers, verifies audience against
the transaction's actual role and bounds installation to five seconds. An error or
canceled future cannot return an executable transaction with uncertain authority.
The SQL policies independently revalidate the live canonical session, membership
and exact acting scope; a previously returned Principal is not a durable grant.
Commit/rollback clears the transaction-local context before healthy reuse.

Qualification uses only owned loopback PostgreSQL18 containers and synthetic
identities/provider tables. It checks cookie/Principal parity, setup and operator
MFA restrictions, provider revocation, preview/acting expiry, tenant isolation,
stale membership/session denial, role/audience refusal, context cleanup and query
cancellation/replacement. A real local operator flow completes password/MFA
setup, recovery-code step-up, WebAuthn registration/login, replay refusal and
session revocation with native reads and the durable allocator. The maintained
software authenticator cannot store resident keys: the test changes only client
selection; server UV/signature/origin checks remain required. The actual Core process serves an authenticated HTTP
request: a brief owned table lock proves the blocked query belongs to the native
backend before the request is released and its response checked. These tests make
no provider network calls and accept no supplied production DSN.

```sh
cargo test --locked -p webdock-identity --test native_sessions -- --include-ignored --nocapture --test-threads=1
cargo test --locked -p webdock-control-plane --test native_runtime --test native_runtime_postgres -- --include-ignored --test-threads=1
cargo test --locked -p webdock-identity -p webdock-control-plane -p webdock-databases -p webdock-provider-connections -p webdock-plugin-host --lib --tests
cargo clippy --locked -p webdock-identity -p webdock-control-plane -p webdock-databases -p webdock-provider-connections -p webdock-plugin-host --all-targets -- -D warnings
cargo fmt --all -- --check
```

The ignored local benchmark alternates 200 reads per driver after warmup. It
measures this debug-build session path, including its respective security checks;
it is not full API throughput, a pure driver comparison or a Neon compute-cost
measurement. Initial local samples do not establish a latency improvement. Keep
the default unchanged until broader realistic measurements justify activation.
Switching the explicit flag back to `sea-orm` requires a normal Core restart and
no data, schema or credential migration.

The later [native session management mode](native-session-management.md) also
selects native reads and reuses this same pool for session control. It does not
create a third identity pool; the read-only mode remains compatible.
