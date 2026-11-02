# Private S3 transport and durable OCI artifacts

Status: opt-in local implementation, 2026-10-25. This is an internal operator guide,
not a Managed Object Storage launch or a backup/recovery promise. Production and
existing stores are unchanged. The owner requested MinIO testing first; real Wasabi
compatibility and account isolation have not been qualified.

## Boundaries

`webdock-object-storage` is a private byte adapter using Apache Arrow's
`object_store` 0.14.2 S3 implementation. Product modules retain authorization,
durable jobs, billing and retention. The raw S3 client is private. Configuration
uses an explicit endpoint, region, bucket, namespace and restricted credentials;
there is no ambient AWS credential discovery, public ACL, browser grant or bucket
creation API. HTTPS validates certificates. Redirects and ambient HTTP proxies are
disabled. Plain HTTP is available only through the explicit `local-test` loopback
constructor, never the production worker configuration.

Transfers use sequential 8 MiB multipart pieces with a 64 GiB ceiling (8192 parts),
bounded download streams, actual byte counts and SHA-256. A completed upload is
read back in full before a receipt is returned. Multipart ETags are not content
hashes. Each attempt receives a fresh UUID key under its purpose and scope;
this writer never overwrites a retained key. This is application-level identity
and integrity, not provider WORM/Object Lock. No irreversible lock is enabled.

A receipt contains the stable backend ID, a hash of its non-secret binding
(endpoint/region/bucket/namespace/ID), exact object key, SHA-256 and bytes. Changing
a credential does not change the binding; changing a destination does. Never reuse
an ID to imply that old data was moved. A receipt is a location, not customer
authorization. Download callers must use private staging and expose bytes only
after verification succeeds.

## Buildworker configuration

Apply forward migration 0046 using the existing explicit offline migration flow.
Do not rewrite older migrations. Existing local artifacts retain null remote
metadata and their old completion hashes. The new identity trigger prevents
changing a recorded artifact's ownership, hash, size, image digest or destination.
Retention state remains separate.

The existing private worker JSON accepts these additional fields:

```json
{
  "artifactStores": [
    {
      "storage": {
        "id": "builds-eu-v1",
        "endpoint": "https://s3.eu-central-2.wasabisys.com",
        "region": "eu-central-2",
        "bucket": "operator-created-private-build-bucket",
        "namespace": "builds"
      },
      "credentialFile": "/etc/webdock/private/builds-eu-v1.json"
    }
  ],
  "artifactUploadStore": "builds-eu-v1"
}
```

This example does not select an existing Wasabi bucket. Frankfurt is a recommended
EU test location, not a confirmed account setting. The credential file is an
absolute root-owned, single-link regular file, mode 0600, containing
`accessKeyId` and `secretAccessKey`. Provision it through the private secret path;
never commit it or place it in guest configuration, customer environment or logs.
Use separate purpose-specific provider credentials, never a customer-visible
account root key.

At most sixteen destinations can remain configured for reads. The explicitly
selected ID is used for new uploads; omission keeps new artifacts local. Retain
old bindings and provider permissions while releases reference them. The existing
local artifact root remains private staging with its existing capacity checks.
Adding S3 does not migrate, delete or make old local artifacts cross-worker.

After a real isolated build, the trusted worker validates OCI bytes locally,
uploads and verifies them, then fsyncs the exact extended completion into its
existing outbox. Core atomically commits build/artifact/release state and checks
current enrollment, lease and source authority. S3 upload alone does not complete
a build. No permanent S3 credentials enter the isolated VM.

Release claims advertise `artifactStoreBindings` and `publicationProjects` (project
ID to exact registry repository) on the existing worker HTTP endpoint. Core filters
both the persisted storage binding and the configured publication target before
assigning a lease, allowing a different currently enrolled eligible worker to
publish remote artifacts. A worker serving another project or registry target
cannot strand the release by claiming it. Legacy local claims retain
the original worker/generation restriction. Existing approval, source, environment
and publishing checks still run. A publisher downloads to fresh private staging,
verifies exact bytes and the recursive OCI graph, then uses the existing guarded
registry publisher. Transfers in the standalone worker recheck live authority
every two seconds and after completion.

## Failure and retention behavior

- A lost multipart completion response is reconciled by reading the exact unique
  key and checking all bytes. A missing/unverifiable result fails closed.
- Read errors and transfer-limit violations attempt a bounded multipart abort.
  No successful receipt is returned on a failed stream or integrity check.
- Lost Core acknowledgments replay the fsynced exact completion; a changed remote
  receipt conflicts. Backend identity cannot be edited in the ledger.
- A process crash or cancellation can leave incomplete uploads or completed
  unreferenced objects. A product-authorized inventory/reconciler is still needed
  for operational orphan cleanup. No age-based deletion or lifecycle policy is
  applied to release objects. Do not enable a blanket expiration policy.
- Existing publication uncertainty remains durable and requires reconciliation;
  this change does not blindly retry ambiguous registry writes or complete an
  interrupted deployment as successful.

Frontend outputs, resolved image/dependency SBOMs, separately addressable evidence
objects and reference-aware orphan reconciliation remain subsequent build stages.
The source-evidence bundle below retains exact source bytes, scoped provenance and
source-file inventory within the existing Core-referenced archive. Customer storage,
backup and archive qualification remain separate stages in the [implementation plan](../superpowers/plans/2026-10-25-s3-managed-storage.md).
Build caches remain excluded.

## Local verification

Use the existing Webdock MinIO service, with its existing private credentials.
Do not reset its volume or start unrelated development services. The acceptance
scripts create random owned buckets, a restricted test user and a policy, then
remove those exact resources and check multipart cleanup. The full build script
uses a separate temporary PostgreSQL container; it never uses the running platform
database. Its private diagnostics live under this plan's ignored workspace.

```sh
cargo test --locked -p webdock-object-storage --features local-test
cargo test --locked -p webdock-build --no-default-features --features worker --test remote_artifacts --test artifact --test contracts
python3 tools/tests/object-storage.py
python3 tools/tests/object-storage.py python3 tools/tests/build-storage.py
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
```

The offline adapter, artifact and fresh-checkout fixture regressions also run in
the existing quality gate. Infrastructure opt-ins remain separate.

The full build acceptance also needs the existing reviewed local KVM fixture and
the pinned registry image used by `crates/build/tests/support/registry.rs`. It tests
real build/publication and durable authority locally; the Hosting agent observation
is the existing fixture proof, not a real Kubernetes deployment. A passing MinIO
run establishes neither Wasabi IAM behavior nor production readiness.

Provider references checked 2026-10-25:
[Wasabi endpoints](https://docs.wasabi.com/docs/service-urls-for-wasabis-storage-regions),
[terms](https://wasabi.com/product-terms),
[pricing rules](https://wasabi.com/pricing/faq),
[quota assessment](https://docs.wasabi.com/docs/enabling-storage-quota-usage),
[S3 transport](https://docs.rs/object_store/0.14.2/object_store/).
Wasabi PAYG minimum billing/retention and trial terms must be reviewed for the
actual account before remote tests or commercial offers. Its documented daily
quota assessment cannot support a byte-exact upload-limit promise.


## Build source evidence bundle

New remote worker builds retain `source.tar`, `provenance.json` and
`source.spdx.json` beside the unchanged `image.tar` inside the immutable artifact.
The outer digest/size and existing S3 receipt bind all bytes; no second release ledger
or database migration is introduced. Local legacy builds keep their one-file format
unless the operator explicitly selects signing. The shared 512 MiB artifact ceiling
includes source and evidence bytes; this is not a dependency/build cache.

The trusted worker captures the exact normalized source archive received from Core.
The guest cannot substitute source or evidence. Provenance binds customer, project,
build/generation, immutable commit, OCI digest, source/image/inventory sizes and hashes,
recipe/root directory and the selected VM/boot hashes. Build environment values,
control/provider credentials and local filesystem paths are excluded. Source contents
remain private project material; this is not a diagnostic sanitization facility.

`source.spdx.json` is a source-file checksum inventory, checked against the vendored
[official SPDX2.3.1 schema](https://github.com/spdx/spdx-spec/blob/v2.3.1/schemas/spdx-schema.json).
It explicitly makes no claim to resolve operating-system/dependency packages or their
licenses. A complete image dependency SBOM and standalone frontend export still need
their own producer paths.

To sign evidence, configure `artifactSigningKeyFile` in the root-owned worker config.
The referenced root-owned, single-link mode0600 file contains a 32-byte Ed25519 seed
as 64 hex characters. Keep that retained key outside VM input and separate from S3
application keys. Its presence enables signed bundles even for explicitly selected
local builds. Without it, remote builds still retain hashes/provenance but do not
claim to carry a detached signature. All worker/control/S3/registry credential files
now reject group/world readability; non-secret readonly configuration is distinct.

The optional `signature.json` signs a domain-separated digest of the exact provenance
bytes. `webdock_build::evidence::validate` accepts an expected public key; use an
operator-approved pin when making a trust decision. An embedded key alone is not
organization trust or production approval. Existing Core artifact hashes still bind
that key/signature to the accepted build record. Rotation does not rewrite historical
archives or release references.

Local put/read, remote hydration and publication reject unknown bundle files,
wrong scope, tampered companions/signatures and changed project identity. Publication
retains the original OCI graph/digest validation. The normal Worker::build_once,
real isolated guest, Core completion/outbox and a separate publisher are included in
the owned MinIO acceptance. No TTL, GC, migration or orphan deletion is enabled.


### Mixed worker versions

New evidence archives use `<sha256>.e1.tar`; old one-image archives retain
`<sha256>.tar`. The existing Core receipt still owns the complete immutable key.
Private worker release claims advertise supported formats; an older wire caller
that omits the field can receive only format0. Core filters before leasing, so a
publisher with an older reader does not strand another worker's release. A new
worker that will produce evidence first requires Core capability1, before claiming
a build. Upgrade Core before enabling new evidence workers. Old local receipt and
completion hashes are not rewritten.

Known source/evidence budget failures complete the build as failed. They cannot
hold the oldest lease through repeated27-minute retries. Provider upload uncertainty
retains its existing reconciliation behavior; it is not converted into success or
an unsafe automatic deletion.
