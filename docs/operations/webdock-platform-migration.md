# Webdock platform PostgreSQL migration

This runbook moves exactly `public`, `webdock`, `webdock_admin`, `webdock_auth`,
and `webdock_mail` into the dedicated managed Webdock platform database. `public`
is retained as the retired CMS archive; restoring it does not authorize reopening
its API or granting runtime access. Spitzli, Stall and other customer schemas and
browser runtimes stay on their existing databases. This procedure does not enable
a backup product, delete the source, or automate production writes.

## Prerequisites and private configuration

Use Bun, repository dependencies, and PostgreSQL 18 `pg_dump`/`pg_restore`.
The utility is `scripts/platform-database-migration/migrate.mjs`. All output paths
must be new private paths outside Git. Dump files contain customer data; logs and
manifests are also private. Generated files use mode 0600 and existing artifacts
are never overwritten. Partial failed artifacts are not successful exports:
only a matching complete manifest authorizes restore.

Supply `SOURCE_DATABASE_URL` and `TARGET_DATABASE_URL` through a private environment
loader, not command-line arguments or shell history. URLs must contain no query
parameters; supply CA paths separately. Do not paste their values into logs.
The source user needs consistent visibility of all selected tables and sequences;
`row_security=off` makes filtered access fail rather than silently export a subset.

A private `config.json` contains only explicit expected identities and CA paths:

```json
{
  "source": {
    "host": "<verified direct Neon hostname>",
    "port": 5432,
    "database": "neondb",
    "user": "neondb_owner"
  },
  "sourceCA": "/etc/ssl/certs/ca-certificates.crt",
  "target": {
    "host": "db-103217546630533120.pg.webdock.dev",
    "port": 32000,
    "database": "<verified managed logical database name>",
    "user": "<authorized managed migration login>",
    "comment": "webdock:database:103217546714419200"
  },
  "targetCA": "/private/managed-postgres-ca.crt"
}
```

The utility validates host, port, database, user, server major version, TLS chain
and hostname. Production target operations also require the exact managed database
comment. It clears inherited libpq `PG*` options so a service file or `PGHOSTADDR`
cannot silently redirect `pg_dump`/`pg_restore`. There is no unverified-TLS flag.
The explicit test-only `fixture: true` mode requires a loopback host and a database
name starting `wd_migration_test_`; never use it for a rollout.

## Rehearsal and selective source fence

### Prepare the managed target

Generate root bootstrap artifacts with `bun scripts/platform-database-migration/roles.mjs /private/role-config.json /private/new-roles-directory`.
The production role config contains only `{"caFile":"/private/managed-postgres-ca.crt"}`;
the host, direct port 32000, logical database and managed owner are pinned.
Review and execute `bootstrap.sql` through root's Kubernetes/psql channel on the
empty target. It refuses existing roles or nonempty platform schemas, creates
separate NOLOGIN owners and runtime identities, and assigns pgcrypto functions to
the private policy owner. Its temporary offline login has no superuser or
BYPASSRLS attribute. Keep the generated credentials and signing key private.

Install the database-agent overlay without replacing its existing App storage
router. Persist the exact logical database's service-login allowlist in the
root-owned `platformDatabases` map and apply its matching dedicated HBA rules.
This map also blocks ordinary control-plane self-maintenance at the agent.
Platform PgBouncer pools use one backend connection per service role within the
existing ten-connection database budget, avoiding starvation between roles.

After final restore and **before** additive migrations, run v2 structural/content
verification. Apply the reviewed additive Mail product migrations using the
offline login, then run:

```sh
bun apps/auth/scripts/install-platform-rls.ts /private/new-roles-directory/installer.env
```

This installs FORCE RLS, signed-context/OLD-row checks, scoped Mail and capacity
helpers, explicit column/function/sequence grants and permanent object owners.
It does not rotate an existing context key. The installer key named
`DATABASE_CONTEXT_SECRET` must equal Auth's runtime `DATABASE_RLS_SECRET`.
Auth additionally needs separate `AUTH_DATABASE_URL` and `DATABASE_WORKER_URL`;
its ordinary `DATABASE_URL` uses `webdock_auth_runtime`. Set
`WEBDOCK_CONTROL_DATABASE_ID=103217546714419200`, preserve all existing Auth/Mail/
SSO encryption keys, and supply the PEM `DATABASE_CA_CERT` to platform services.
Use direct port 32000 for offline work/browser access and pooled port 32001 for
the application service URLs. Never deploy the temporary offline login to Vercel.

After successful cutover, execute the generated `cleanup.sql` through the same
root channel. It terminates the offline sessions, disables login/CREATEROLE,
clears its password and revokes memberships without deleting owned objects.
Remove that temporary login from the persistent HBA allowlist too.

Complete the RLS, role, managed HBA/lifecycle, verified connection, native Tabularis
and application-flow rehearsals before production fencing. Preserve signing and
encryption keys, IDs and source credentials. Prepare the target identity marker
through the existing managed database provisioning process.

Stop platform deployments, migrations, cron/queue consumers and native browser
writes. Revoke the affected website browser grant and drain/stop its isolated
runtime. Do not stop unrelated customer runtimes. Application maintenance is also
needed for connections using shared `neondb_owner`; globally disabling that role
would affect unrelated customers. Freeze DDL throughout export and cutover.

Generate SQL using a read-only source connection:

```sh
bun scripts/platform-database-migration/migrate.mjs fence-sql /private/config.json /private/platform-fence.sql
bun scripts/platform-database-migration/migrate.mjs unfence-sql /private/config.json /private/platform-unfence.sql
```

Review the exact source identity and table list, then execute the fence SQL
manually against the verified source. The script itself does not execute it.
The SQL takes one migration advisory transaction lock, then ACCESS EXCLUSIVE
locks on every ordinary/partitioned table in the five schemas in deterministic
order. Existing writers must finish before all locks are acquired. A five-second
lock timeout rolls the transaction back instead of partially installing a fence;
issue ROLLBACK after any error and investigate the specific blocker.

Only after all table locks are held does the transaction create statement triggers
for INSERT, UPDATE, DELETE and TRUNCATE and enable them ALWAYS. Its commit releases
table locks and activates the complete fence atomically. New platform writes then
fail regardless of application role. No database-wide write setting, customer
schema trigger or broad role revocation is used. Existing reader queries continue.
The advisory lock serializes fence operations; table locks, not voluntary writer
participation in that advisory lock, provide the drain barrier.

This is an operational fence, not protection against a malicious schema owner:
an owner can remove triggers or issue DDL. Stop all such paths, prevent new table
creation and direct sequence writes, and independently confirm old deployments
are quiet. Sequences are not MVCC; export compares their state before and after
its snapshot work and refuses a changed result. A trigger does not fence arbitrary
standalone `nextval`/`setval` calls. Unrelated schema writes remain allowed; shared
cross-schema foreign keys could still cause an unrelated request to touch a fenced
platform table, so inspect those dependencies before the production freeze.

## Consistent export, restore and verification

```sh
bun scripts/platform-database-migration/migrate.mjs export /private/config.json /private/platform-final.dump
bun scripts/platform-database-migration/migrate.mjs restore /private/config.json /private/platform-final.dump
bun scripts/platform-database-migration/migrate.mjs verify /private/config.json /private/platform-final.dump
```

Export refuses any selected table without the expected always-enabled write fence.
A repeatable-read, read-only exporter transaction shares its exported snapshot
with `pg_dump`. Table verification runs on that same snapshot. Sorted canonical
JSON rows are fetched through a cursor in batches of 256 and hashed with SHA-256
and explicit row lengths; duplicate rows contribute separately. Memory is bounded
by the batch, not total table size; PostgreSQL may spill its sort to disk. Each
ordinary/partitioned relation is queried with ONLY, counting partition data once.
Manifest version 2 records table identities, row counts, content digests and sequence
`last_value`/`is_called`, but no plaintext rows. It also records logical structure:
ordered columns and types, defaults/nullability/generated and identity properties,
collations, partitions, constraints/indexes, sequence configuration, enums/domains,
routine definitions/security/search paths, noninternal triggers, views and rules.
Only the exact migration-fence function/triggers are excluded. Catalog deparsing
uses `pg_catalog` as search path; ownership, ACLs and RLS are verified separately.
The manifest can contain private routine definitions and remains mode 0600.
Older manifests without structural evidence are rejected. Foreign tables and
user-defined aggregates without supported structural comparison fail closed.

Restore verifies the archive SHA-256 and exact schema manifest first. It refuses
any existing selected table, sequence, foreign table, view or materialized view;
it never uses `--clean`, drops databases or overwrites populated data. An existing
empty schema such as `public` is reused. Restore runs in one transaction, stops on
errors and skips source ownership/ACLs; the separate reviewed RLS installer assigns
target ownership and privileges. Source routine bodies and fixed search paths are
preserved by `pg_dump`. Only the exact migration-fence trigger/function TOC entries
are omitted; the source remains fenced. No customer schemas are included.

Run exact verification immediately after restore, before application tests,
additive Mail migrations or the RLS installer. Those intentionally change structure
and must run only after source-to-target structural equality has passed. Any logical structure, table identity/content/count or sequence
mismatch fails the command. Then install and verify target roles/RLS/helpers and
run the rehearsed sign-in, Studio, Mail, worker, native Tabularis and tenant-denial
checks. Keep target writers unavailable until all required gates pass. Switch only
platform environment variables and the affected browser runtime, using verified
CA settings and dedicated identities. Verify actual deployed database identity
and unchanged customer services after switching.

## Rollback and evidence

Before target writes, rollback can retain the failed target, restore platform
connections/runtime configuration to source, and manually execute the reviewed
unfence SQL after confirming the source is still authoritative. It takes the same
table locks and removes only the named migration triggers/function. Keep unrelated
customer deployments unchanged.

After any target writes, the databases diverge. Stop target writers and reconcile
those writes through a separately reviewed reverse migration before removing the
source fence or routing traffic back. Simply reconnecting to old Neon data loses
changes. Retain the source, final archive/manifest, selected environment deltas,
verification evidence and fence/unfence SQL until the owner approves cleanup.

## Local verification

Unit checks require no database. The integration test creates and deletes only new
randomly named `wd_migration_test_*` databases on an explicitly selected local
PostgreSQL 18 container. It never accesses the restored rehearsal database.

```sh
bun test scripts/platform-database-migration/migrate.test.mjs
MIGRATION_TEST_CONTAINER=<local-pg18-container> MIGRATION_TEST_ENV=/private/local-postgres.env bun test scripts/platform-database-migration/migrate.test.mjs
```

The private test env file contains the container's POSTGRES_USER,
POSTGRES_PASSWORD and POSTGRES_DB. Tests prove writer draining fails safely on
lock timeout, all four platform write operations are fenced, Spitzli/Stall fixture
writes remain available, fenced export restores without target fences, duplicate
row content and sequences match, and altered data/sequences or populated targets
are rejected.

## Targeted native Tabularis cutover

The legacy browser cluster (`root@178.104.141.95`) is separate from the managed
PostgreSQL cluster. Keep `db-webdock-operator` in `webdock-databases`; preserve
website binding `102393528113434624`, project `99868136106360832`, saved connection
`8032eaca-fddf-4c95-8d12-1b29e2b2dc8e`, groups, state PVC, proxy secret and internal
TLS. Do not deploy the full gateway inventory or modify Spitzli/Lunares runtimes.

`scripts/platform-database-migration/tabularis.mjs` generates targeted JSON patches
only; it never contacts Kubernetes. Read-only live snapshots and initial review
patches are stored privately under the migration directory's `tabularis-cutover/`.
The generator requires the known resource names, namespace, connection ID and pinned
runtime image. Deployment/Secret/network patches test Kubernetes resourceVersion;
any intervening change must produce a fresh reviewed snapshot and patch set.

Capture only these resources into a mode-0600 private file (capture stdout directly;
do not print it because the Secret includes credentials):

```sh
umask 077
set -o noclobber
ssh -o BatchMode=yes root@178.104.141.95 'kubectl -n webdock-databases get deployment/db-webdock-operator secret/db-webdock-operator networkpolicy/db-webdock-operator -o json' > /private/live-before.json
```

Prepare noncredential patches using the public managed CA:

```sh
bun scripts/platform-database-migration/tabularis.mjs prepare /private/live-before.json /private/managed-ca.crt /private/patches-new
```

This writes a dedicated `db-webdock-operator-database-ca` ConfigMap, adds only its
read-only `/database-trust` mount to the Tabularis container, and replaces only that
runtime's egress with `79.143.177.157/32` TCP 32000. Existing DNS policy, runtime
ingress, TLS sidecar, gateway CA, volumes and all other deployments remain unchanged.
Rollback mount/network patches test the expected cutover values before removing
or replacing them.

At execution time only, create a private mode-0600 JSON credential file with exactly
`username`, `database`, `password`. The generator requires username
`webdock_browser_website`, database `db_103217546714419200`, and the actual newly
provisioned password; never synthesize a placeholder and apply it.

```sh
bun scripts/platform-database-migration/tabularis.mjs credentials /private/live-before.json /private/browser-credentials.json /private/patches-new
```

It changes only Secret `data["connections.json"]`, preserving its envelope,
groups, connection identity and unrelated parameters. The new saved parameters
use host `db-103217546630533120.pg.webdock.dev`, port 32000, `ssl_mode=verify-full`
and `ssl_ca=/database-trust/ca.crt`. The proxy secret, TLS key/certificate and
`config.json` are untouched. Generated Secret and rollback patches still contain
credentials encoded as base64; keep them private and never show kubectl diff output
for those files.

After source writer fencing and target verification, revoke the affected native
browser grant and scale only its deployment to zero. Refresh the three-resource
snapshot **after scaling** and regenerate into a fresh directory. Scaling changes
resourceVersion, so earlier review patches intentionally fail. Review and use
server-side dry runs, then create the dedicated ConfigMap, apply the Secret JSON
patch, NetworkPolicy JSON patch and Deployment JSON patch against exactly the
named resources. Kubernetes `patch --type=json --patch-file=<private-file>` accepts
the generated patch files. Scale only this deployment back to its recorded replica
count and wait for rollout. The existing init container copies the updated saved
connection from Secret to the retained state volume at startup; no PVC replacement
or direct state-volume edit is needed.

Before re-enabling the grant, exercise authenticated Tabularis RPC from its private
TLS endpoint using the existing protected proxy credentials and runtime CA, or the
normal native gateway after a controlled temporary operator grant. Negotiate
`/api/v1/session`, require pinned server commit
`fdd6c0a6f979ede0acb38415fa12b25c04aa9c4e`, use its CSRF/session values and execute `SELECT 1` through the saved connection ID.
The restricted upstream `test_connection` endpoint rejects inline host certificate
paths. The gateway therefore implements its browser-facing connection test as a
fixed query against the saved connection, preserving the CA and verify-full mode.
Then execute through that same runtime:

```sql
SELECT current_database() AS database, current_user AS role,
       current_setting('server_version') AS version,
       (SELECT ssl FROM pg_catalog.pg_stat_ssl
        WHERE pid=pg_backend_pid()) AS tls;
```

Require the exact target database/browser role, PostgreSQL 18.6, and `tls=true`.
A successful saved connection with verify-full and the mounted private CA exercises
the actual certificate path used by the runtime; a controller-side `psql` success
alone is insufficient. The pinned implementation reads `ssl_ca` from disk in
`src-tauri/src/pool_manager.rs` (`build_postgres_tls_connector`), validating chain
and hostname for verify-full; `require` does not verify certificates. The default
saved-connection CA path is passed unchanged through the gateway. Test wrong CA and
hostname failure in the isolated rehearsal runtime, not by weakening production
verification.

Verify schema browsing/SQL and approved disposable row editing, rejection of
Auth/Admin/other-tenant reads, forged connection-ID denial, credential redaction
and revoked-session rejection. Replace/re-enable the same grant to invalidate old
sessions. Separately confirm Spitzli and both Lunares connections still negotiate
and read their own metadata. Rollback restores the old Secret connection and egress,
removes only the added mount/volume, and restarts this one runtime; never remove the
source fence or point back after target writes without reconciling data first.

### Cleanup when the root signer is unavailable

Do not leave the temporary credential active. The offline role can clear its own
password with `ALTER ROLE webdock_offline_migrate PASSWORD NULL`. Commit that
change, terminate its other database sessions, then close the last session.
Verify fresh authentication fails on both direct PostgreSQL and PgBouncer and
that no offline-role backend remains. This is containment, not full cleanup:
restore privileged access to run `cleanup.sql` and remove the temporary HBA
entry. Record the pending LOGIN/CREATEROLE/membership cleanup explicitly.
