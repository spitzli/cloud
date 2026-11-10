# Direct PostgreSQL runtime — allocator increment

This increment introduces a direct tokio-postgres/Deadpool connection runtime and
ports central HTTP Core ID allocation through it. It is not the completed
SeaORM/SQLx replacement: business services, other allocator callers and offline
migration tools still retain explicit legacy paths.

## Opt-in and compatibility

`SPITZLI_ID_BACKEND=tokio-postgres` selects the native allocator in the HTTP Core.
The default remains `sea-orm`. Unknown values fail startup. Existing credentials,
database, ID layout and migration history remain unchanged; no new Neon project
is required. There is no automatic production activation.

The allocator uses a separate pool capped at two connections, warm/minimum one,
matching the existing isolation from business transactions. Pool wait is bounded
to 30 seconds; connection/authority checks to five seconds; idle cleanup drops
unused connections after 30 seconds down to the minimum. Cleanup performs no SQL
health polling and uses weak ownership so it cannot retain a discarded pool.

## Authority and transaction ownership

Only existing restricted platform login roles are accepted. URI authority,
authenticated backend user, current/session role, privilege flags and protected
owner memberships are checked. A transaction cannot start with a leftover signed
RLS context/signature. These checks run on the transaction's actual backend,
including when a provider uses transaction-mode pooling.

Native transactions explicitly begin with READ COMMITTED. Fresh authority/facts
queries after canonical row-lock acquisition therefore see revocation committed
during a lock wait, even if a role/server default selected repeatable snapshots.
This is the native runtime's isolation contract; it does not override retained
SeaORM transaction settings.

The public connection exposes checked native transactions, native rows and ToSql
parameters; it does not translate ORM statements. Normal callers explicitly
commit or roll back. Cancellation, an implicit drop or an unconfirmed/failed
commit discards the physical connection rather than returning uncertain state to
the pool. Runtime transactions have a 30-second server statement budget and a
five-second lock budget. Commit/rollback acknowledgement is bounded by a 30-second
client deadline from transaction start; the allocator's complete operation,
including acquisition and SQL, has the same overall client budget. A silent
network blackhole therefore cannot hold allocator slots indefinitely. Native
service ports must likewise bound their full query operations, rather than rely
on a server timeout to bound network I/O. Closing a client connection is not a claim that the
database has already stopped an in-flight statement; server budgets remain in
force.

Global ID allocation fuses the same authority predicate with its required
generator-row lock. Its private checked entry returns metadata only after the
predicate succeeds. Database time is read **after** acquiring that row lock.
The immutable generator layout/high-water trigger and confirmed-commit-before-ID
return remain intact. There is no automatic retry after an uncertain commit;
retrying later advances past any committed but unconfirmed allocation.

## TLS and URL options

Production/non-loopback targets retain the existing verified-TLS requirement.
Rustls checks the actual driver hostname. A supplied CA replaces the default root
set; otherwise WebPKI roots are used. Local plaintext is allowed
only under the existing development connection policy. A TLS refusal or wrong
certificate name never falls back to plaintext. SSL mode casing and aliases do
not change this requirement.

Supported native URL parameters are SSL mode/root/client certificate/key aliases
and `application_name`. Other SQLx-specific options are rejected explicitly;
they are not silently reinterpreted. Review existing URI options before opt-in.
TLS files must be bounded regular files owned by the service user or root, with
no group/world writes. Private client keys additionally disallow group/world
access. Symlinks and non-regular files (including FIFOs) are refused without blocking;
provision compatible file/subPath references rather
than weakening those checks or rotating credentials implicitly.

## Qualification and performance boundaries

Owned PostgreSQL 18 tests cover bounded pooling, healthy rollback reuse, dirty
session refusal, role privilege changes, cancellation/replacement, idle cleanup,
concurrent allocator instances, restart, backwards time, business rollback and
deferred commit failure. A transparent owned proxy silently withholds a durable COMMIT response without
closing the socket
to prove that no ID is released or reused after a lost acknowledgement. An owned
TLS proxy proves valid trust, hostname refusal and TLS-refusal behavior. Fixtures
deliberately change privileges/passwords only on their own disposable database.

An actual Core process opts in, handles a normal public waitlist write through
the configured Studio virtual host and confirms native backend activity and
unchanged schema/account records. Host and origin admission rules are retained.

The non-gating allocator experiment alternates both drivers against the same
durable local PostgreSQL database. It measures this one debug-build path, not full
API throughput or Neon CU-hours. Initial samples showed no reliable native
latency advantage; added authority checks and per-ID durable commits matter.
Do not change production defaults based on a driver name or one local run.
Native static queries use explicit PostgreSQL parameter types to avoid unnecessary
prepare/execute/close round trips, retaining all authority checks.

```sh
cargo test --locked -p spitzli-persistence --lib --tests
cargo test --locked -p spitzli-persistence --test native_pool --test native_ids --test native_tls -- --include-ignored
cargo test --locked -p spitzli-control-plane --test native_runtime_postgres actual_http_write -- --ignored
cargo clippy --locked -p spitzli-persistence -p spitzli-control-plane --all-targets -- -D warnings
```

The ignored cases create exclusively owned loopback PostgreSQL containers; they
accept no supplied production DSN or live credentials. The benchmark is diagnostic
and has no speed threshold assertion.

Continue signed tenant/session and product-service ports next. Remove legacy
constructors/dependencies only after each replacement and its rollback/permission
contracts are qualified. S3/storage library decisions remain benchmark-driven.

References: [Deadpool PostgreSQL](https://docs.rs/deadpool-postgres/0.14.2/deadpool_postgres/)
and [typed tokio-postgres queries](https://docs.rs/tokio-postgres/0.7.18/tokio_postgres/struct.Client.html#method.query_typed).
