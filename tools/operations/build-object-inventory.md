# Read-only build object inventory

This operator tool correlates a fresh consistent Core metadata snapshot with an
explicit provider listing. It has no network client and cannot delete, abort,
upload or migrate anything. A candidate is a review item, never deletion authority.
It does not verify object bytes, completeness of a provider listing, or commercial
retention obligations. Do not use it to remove old stores or referenced releases.

Use an authorized privileged **read-only inspection** PostgreSQL connection.
The supplied SQL uses one repeatable-read/read-only transaction and disables RLS
filtering: PostgreSQL must reject insufficient privileges rather than silently
export an incomplete reference set. The query reads build/artifact metadata only,
with finite query/idle timeouts. Do not use a fabricated user/session RLS context.

In an owned mode0700 operator diagnostics directory, with umask077:

```sh
psql service=webdock-inspection --no-psqlrc -qAt \
  -f tools/operations/build-object-ledger.sql > ledger.json
mc ls --recursive --json readonly-alias/owned-bucket/builds/ > listing.jsonl
python3 -B tools/operations/build-object-inventory.py \
  --ledger ledger.json --listing listing.jsonl --namespace builds \
  --binding <retained-non-secret-backend-binding-sha256> --output report.json
```

Credentials stay in the operator's protected PostgreSQL/MC profile, not arguments
or reports. The Python tool rejects symlinked, shared-link, public, foreign-owned,
oversized input files or a stale Core snapshot and creates a new mode0600 report without overwriting.
Its stdout contains only counts. Unknown key shapes are hashed instead of copying
potential tokens/credential URLs into diagnostics.

Every Core artifact reference is protected, including rollback/unretained rows.
Foreign-binding copies, local-only migration copies, active/future generations,
unknown builds and unknown key shapes remain held. Only unreferenced terminal or
superseded generations are candidates. Any invalid reference, size mismatch,
listing error or duplicate/versioned listing suppresses all candidates.

Do not interpret a listing ETag as SHA-256. Do not act on an old snapshot. Before
any separately authorized cleanup, independently revalidate current canonical
references, all active producers/outboxes and applicable retention; stop if any
outcome is unknown. This task enables no automatic GC, TTL or cleanup command.

Offline regression:

```sh
python3 -B -m unittest discover -s tools/tests -p test_build_storage_inventory.py
```

[SQL qualification](../../docs/verification/2026-10-25-readonly-build-inventory-sql.json)
also exercised this query on a fresh, empty compiled schema 53: privileged read-only
inspection succeeded, while the native Auth and worker roles were denied. This
does not qualify populated reference inventories, provider-listing completeness,
or later recovery migrations.
