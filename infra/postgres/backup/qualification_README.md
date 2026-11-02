# Native PostgreSQL backup qualification

This is a disposable local fixture. It does not activate production, Wasabi, shared
PostgreSQL, an old-data migration, or a production signing identity. The source
provenance tools and their outputs are separate prerequisites; this harness never
modifies them.

Use Python 3 with PyYAML and local Docker. `prepare` requires the exact private
provenance output and public pin embedded in the tool, the existing
`webdock-dev-minio-1` on `webdock-dev_default`, and the existing pilot image. It
verifies the signed bundle before importing either image into a **new** node.
The node is limited to four CPUs, 6 GiB RAM, 3,072 PIDs and a 45-minute phase budget.
It publishes no host ports. Up to ten explicitly enumerated 1.5 GiB loop-backed
volumes are permitted (15 GiB aggregate); none is an S3 mount. The initial source,
all recovery targets and the ENOSPC probe have distinct bounded volumes.

The test writes private state containing short-lived fixture credentials under a
new mode-0700 output directory. Never commit or share it. Commands and diagnostics
stay private; root MinIO credentials are read only through the existing private
configuration. Cleanup removes exact owned resources, detaches owned loop devices,
scrubs credentials and preserves the existing MinIO container. A failed phase stops
with an exact private diagnostic path; inspect it and run cleanup. Do not initialize
an existing fixture volume or use prune.

```sh
python3 -m unittest discover -s infra/postgres/backup -p qualification_test.py -v
python3 infra/postgres/backup/qualification_core.py
python3 infra/postgres/backup/qualification.py prepare --output /tmp/NEW-OWNED-DIRECTORY
python3 infra/postgres/backup/qualification.py qualify --output /tmp/NEW-OWNED-DIRECTORY
python3 infra/postgres/backup/qualification.py pitr --output /tmp/NEW-OWNED-DIRECTORY
python3 infra/postgres/backup/qualification.py tls --output /tmp/NEW-OWNED-DIRECTORY
python3 infra/postgres/backup/qualification.py rpo --output /tmp/NEW-OWNED-DIRECTORY
python3 infra/postgres/backup/qualification.py faults --output /tmp/NEW-OWNED-DIRECTORY
python3 infra/postgres/backup/qualification.py schedule --output /tmp/NEW-OWNED-DIRECTORY
python3 infra/postgres/backup/qualification.py full_pitr --output /tmp/NEW-OWNED-DIRECTORY
python3 infra/postgres/backup/qualification.py rename --output /tmp/NEW-OWNED-DIRECTORY
python3 infra/postgres/backup/qualification.py cleanup --output /tmp/NEW-OWNED-DIRECTORY
```

The Core fixture has its own fresh PostgreSQL container, restricted roles, native
migrations and synthetic identities. Its checkpoints are **synthetic authority
contracts**, not a real restore. The CNPG fixture performs real base backup, WAL
archiving, PITR, SQL/hash/role/TLS checks and a separate writer/crash recovery.
Those results must not be described as a connected Core → gRPC → Native executor
end-to-end result.

`qualify` writes A, takes a database-clock target, then writes B; only that
deterministic case deliberately switches WAL. `rpo` writes acknowledged commits,
disables reconciliation, force-kills the source **before** fencing it, and restores
the available archive without a final WAL switch or graceful source shutdown.
The selected base and timeline remain explicit. `schedule` uses a ten-second cron
only to bound the fixture; the Core product contract currently accepts daily cron.
The schedule suspension check counts new Backup resources, allowing an already
started backup to finish.

`qualification_admission.documents(plugin_pin, sidecar_pin)` renders the additional
fail-closed sidecar policy. Before deployment, also regenerate the existing database
boundary with the sidecar pin in its approved image list. The native agent advertises
`databaseBackupVersion: 1` only after its configured pinned Barman deployment,
sidecar and admission policy are observed. This harness does not install or change
any production admission policy.

## Connected qualification API

`qualification_connected.py` adds the actual Core domain → enrolled Unix gRPC →
`NativeDatabaseExecutor` path. It creates its own cluster and Core PostgreSQL;
the existing MinIO stays running. Use a new private output directory:

```sh
python3 infra/postgres/backup/qualification_connected.py prepare --output /tmp/NEW-CONNECTED-OWNED-DIRECTORY
python3 infra/postgres/backup/qualification_connected.py backup --output /tmp/NEW-CONNECTED-OWNED-DIRECTORY
python3 infra/postgres/backup/qualification_connected.py archive_core --output /tmp/NEW-CONNECTED-OWNED-DIRECTORY
python3 infra/postgres/backup/qualification_connected.py recover_core --output /tmp/NEW-CONNECTED-OWNED-DIRECTORY
python3 infra/postgres/backup/qualification_connected.py restore --output /tmp/NEW-CONNECTED-OWNED-DIRECTORY
python3 infra/postgres/backup/qualification_connected.py cleanup --output /tmp/NEW-CONNECTED-OWNED-DIRECTORY
```

The database-only integration caller is
`cargo test --locked -p webdock-agent-control --test database_backup_connected`;
the harness runs its ignored test inside its own node, with explicit private
configuration and restricted Core roles. The Rust fixture implementation lives in
`crates/databases/tests/support/connected_backup.rs`. It uses first-party password
change/TOTP operator setup and an enrolled, UID-bound private Unix gRPC listener.
It never manufactures a browser principal or native Ready receipt.

`archive_core` destroys the original Core PostgreSQL container after saving a
protected database/roles backup and separately retained encryption/RLS/identity
bundle. `recover_core` creates a new empty PostgreSQL runtime and restores those
artifacts. The source CNPG database stays unavailable. The recovery test selects
historical credential revision 1 after the source has confirmed revision 2.
Recovery artifacts are fixture-only and are destroyed during exact cleanup.

The S3 TLS endpoint is an owned TCP bridge on the new node to existing MinIO.
It preserves signed HTTP bytes and verifies a private fixture CA through Barman's
`endpointCA`; no certificate verification is disabled. Host utilities and their
shared libraries are copied into the disposable node to exercise the native bounded
storage helper. The debug Rust test uses 16 MiB thread stacks. Production runtime
images and the provenance bundle are never modified.

The connected harness uses a separately bounded **6 CPU / 8 GiB** node so the source,
a fault-tested recovered target, and the independent timed target can all retain
their reservations under the native 20% node headroom check. Core PostgreSQL is a
separate 2 CPU / 1 GiB disposable container. Each target has its own 1.5 GiB bounded
filesystem; the pool ceiling is 9 GiB. PostgreSQL requests/limits 250m CPU / 512 MiB;
the native Barman sidecar requests/limits 100m CPU / 256 MiB. The 128 MiB native
sidecar profile OOM-killed during a real base backup and is no longer the new-policy
default. These are local fixture measurements, not qualification of every tariff.

A failed/restarted fixture can resume existing accepted orders and native Core
receipts without inventing readiness. If a lease expires while the fixture client
is stopped, `databases.reconcile` requires a current operator and returns unfinished
restore work to guarded configuration before SQL readiness. The original migration
journal and source volume are preserved.

A Core logical dump must retain extension-member ownership separately: pg_dump
recreates pgcrypto but does not reproduce its changed function owners. New bundles
include `extension-ownership.sql`. The diagnostic bundle created before this addition
was repaired using the pinned native bootstrap ownership invariant, after the
original Core was destroyed; no live source metadata was consulted.

## Independent review transitions (fresh owned services only)

The four transition fixes have a separate evidence record:
`docs/verification/2026-10-25-native-postgresql-backup-transitions.json`.
The old 78.221-second/125-test drill remains historical and is not approval of them.

`qualification_review.py` creates its **own** MinIO container, isolated Docker
network, restricted Core PostgreSQL and Kubernetes node. It never reads the shared
MinIO credentials or falls back to an existing provider/network. The nonroot MinIO
uses a private 2 GiB tmpfs owned by UID/GID 10001, 512 MiB RAM and one CPU. Core uses
2 CPUs/1 GiB; Kubernetes uses 6 CPUs/8 GiB. PostgreSQL is 500m→1000m/1 GiB, Barman
100m/256 MiB, each native filesystem 1.5 GiB. All resources carry exact private
custody IDs. Only the pinned fixture images are reused, never an existing service,
stopped Kind cluster or dev k3s instance.

```sh
python3 infra/postgres/backup/qualification_review_core.py
python3 infra/postgres/backup/qualification_review.py prepare --output /tmp/NEW-PRIVATE-REVIEW-DIRECTORY
python3 infra/postgres/backup/qualification_review.py review-initial --output /tmp/NEW-PRIVATE-REVIEW-DIRECTORY
python3 infra/postgres/backup/qualification_review.py registry-blocked --output /tmp/NEW-PRIVATE-REVIEW-DIRECTORY
python3 infra/postgres/backup/qualification_review.py review-repair --output /tmp/NEW-PRIVATE-REVIEW-DIRECTORY
python3 infra/postgres/backup/qualification_review.py review-rotate-restore --output /tmp/NEW-PRIVATE-REVIEW-DIRECTORY
python3 infra/postgres/backup/qualification_review.py review-export --output /tmp/NEW-PRIVATE-REVIEW-DIRECTORY
python3 infra/postgres/backup/qualification_review.py review-stop --output /tmp/NEW-PRIVATE-REVIEW-DIRECTORY
python3 infra/postgres/backup/qualification_review.py registry-stopped --output /tmp/NEW-PRIVATE-REVIEW-DIRECTORY
python3 infra/postgres/backup/qualification_review.py cleanup --output /tmp/NEW-PRIVATE-REVIEW-DIRECTORY
```

The Core/Registry suite models existing installation metadata in a new restricted
fixture and uses actual password/TOTP principals and actual Registry methods. Its
initial metadata is not a physical readiness assertion. The separate connected
suite obtains every physical Ready receipt from `NativeDatabaseExecutor` via real
enrolled Unix gRPC. It creates 103 manual history entries, waits for an actual daily
completed backup, proves bounded paginated observation, and injects a closed native
API route to prove known observation failure immediately clears configured status.
Repair uses normal policy commands; no SQL row repairs are used.

The suite proves 500m→1000m resize retains 1800m CPU, 2.25 GiB memory and 1.25 GiB
ephemeral peak quota/demand, advances the policy resource revision and reobserves
it. A second fresh historical restore exercises readonly-role lineage. The archives
owner's new support helper qualifies the actual managed source with verified libpq
TLS and app-only PG18 custom gzip dump, then proves stale-reference denial after
actual Native rotation and suspension. Credentials stay in memory/environment;
only a temporary public CA is written. Dump scratch is anonymous and removed.

The Registry executable is prepared before stop/pause so no compiler delay consumes
the 90-second stop-observation window. Actual project and customer/Default archival
are refused before observed stop, and succeed after explicit recurrence/WAL/
retention stop and Native pause. A partition, stale stop or mismatched resource
revision refuses archival; it does not manufacture a controller acknowledgment.

0051 is frozen at SHA-256
`a4f16036e77bd2617f61cecc7c7881bb1abfceb920a16309e092770094ed7767`.
0048 and 0050 remain unchanged. The source export support file belongs to the
archives agent; the connected driver only calls its reviewed API.
