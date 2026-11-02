# Private S3 media for new CMS instances

This opt-in path is for new `apps/cms-instance` installations. Local media remains
the default. Existing `apps/cms` Blob stores, local files and production instances
are not migrated or deleted. S3 is object storage; it does not replace the CMS
PostgreSQL database or the private transfer directory.

## Provisioning

Provision an owned private Managed Object Storage resource with the intended
customer/project/environment binding. Supply only its restricted application key,
never a provider root/admin key. Use a dedicated bucket or a provider policy that
actually denies other scopes. The CMS prefix alone is not an isolation boundary.
The CMS needs object read/write and, for a separately authorized permanent purge,
delete permission. It does not need bucket administration, public ACLs or CORS.

Set the explicit `CMS_MEDIA_STORAGE=s3` variables listed in
[`apps/cms-instance/.env.example`](../../apps/cms-instance/.env.example), including
`CMS_ENVIRONMENT_ID` and `CMS_MEDIA_S3_RESOURCE_ID`. Endpoint, region and bucket
are mandatory; production endpoints require verified HTTPS. Data credentials are
explicit and cannot fall back to an ambient AWS identity. Only the literal
loopback acceptance fixture can opt into HTTP.

Run the existing reviewed migration job with `CMS_MIGRATING=true`, migration-only
PostgreSQL credentials and `CMS_MEDIA_INITIALIZE_STORAGE=true`. Include the same
non-secret S3 destination and identity variables, **without S3 data credentials**.
Initialization refuses a database containing media. The new forward migration
adds nullable hashes/internal adapter fields and a private immutable destination
binding; it does not alter historical migrations or copy bytes. Runtime receives
only DML PostgreSQL credentials and the restricted S3 application key.

Every runtime `getCMS` call verifies the binding before returning a Payload
instance. This check is independent of Payload's initialization cache: repeated
startup failures keep health at 503, and a transient verification failure can
recover when the database becomes available. No failed initialization is trusted.
The recorded binding must match the instance, project, environment,
resource, backend, endpoint, region, bucket and derived prefix. Rotating the key
for the same destination is allowed. Changing destinations, changing back to local,
or adopting an existing local store is rejected. Do not roll an S3 instance back
to an image that does not understand this binding. A real migration needs its own
inventory, source/target hashes and references, final write fence and rollback.

## Delivery and retention

The existing `/api/media/file/<UUID>.<extension>` URL remains the delivery path.
The bucket stays private; the CMS never returns a raw provider or presigned URL.
Media keeps its existing public raster policy: validated PNG/JPEG/WebP/AVIF,
maximum 10 MiB and 40 megapixels. Private documents and exports require their
separate authorized binary path.

The server derives the stored SHA-256 from uploaded bytes. Clients cannot choose
hashes, prefixes or internal key segments. Before returning a remote file, the
CMS checks visible metadata, exact byte length and hash; redirects, partial,
truncated, oversized or corrupted responses fail closed. The provider read has a
30-second hard deadline covering HEAD, GET, retries and body consumption, with
15-second throwing SDK request timeouts. Cancellation also applies to HEAD and
already-aborted callers; aborted bodies release their sockets. Original local
media without a historical hash stays readable; new local uploads receive hashes.
The checksum is hidden from both the Payload API's ordinary fields and the native
editor descriptor; clients cannot edit it.

The native CMS API keeps file replacement disabled. Trashing changes metadata and
retains bytes; restoring metadata reuses the same URL. Permanent deletion is a
separate trusted operation. A denied provider purge rejects the operation and
rolls back PostgreSQL deletion, retaining the exact object reference and versions
for retry. A successful provider deletion followed by a database commit failure
still needs explicit reconciliation: the two systems do not share a transaction.
Rollback never blindly deletes stored bytes. Versioned buckets may retain older versions after
ordinary deletion; provider version purge remains a separate storage lifecycle
action. No bucket-wide lifecycle, TTL, garbage collection or irreversible lock is
activated here. A failed database transaction after a completed provider upload
can leave an orphan: retain it for an explicit owned inventory/reconciliation,
never delete unknown objects automatically.

## Verification

```sh
bun test --isolate apps/cms-instance/tests/*.test.ts
bun run --filter @webdock/cms-instance build
python3 -B apps/cms-instance/scripts/acceptance.py
python3 -B apps/cms-instance/scripts/acceptance.py --s3
# Also test the built immutable server and bounded migration executable:
python3 -B apps/cms-instance/scripts/acceptance.py --image <owned-local-candidate>
python3 -B apps/cms-instance/scripts/acceptance.py --s3 --image <owned-local-candidate>
```

The S3 fixture reuses the existing Webdock MinIO service, creates random owned
buckets and a restricted user, and uses a separate TLS PostgreSQL 18 database
with separate migration/runtime roles. It recreates source processes rather than
pretending a local directory is durable S3. A delete-only provider denial proves
that permanent purge retains the document, versions, exact identity and bytes,
then a retry proves actual deletion. Both source and image checks repeat rejected
binding startup and health requests. Local acceptance first migrates a fresh
fixture to the old schema, seeds an actual local file, row and version, and only
then applies the forward migration; the null historical hash, URLs and bytes stay
intact. The image also reads that preexisting file over HTTP.

The dependency tests take about 30 seconds because they exercise the configured
hard deadline against stalled HTTP sockets without shortening the product timeout.
All owned resources are removed and their absence is checked;
private failure diagnostics are retained. MinIO acceptance is not a live Wasabi
qualification or a production rollout.

Dated results and the fixture-only image identity are recorded in
[CMS S3 verification](../verification/2026-10-25-cms-s3-media.json). The pinned
Payload dependency patches and their reasons are recorded in
[`patches/README.md`](../../patches/README.md).
