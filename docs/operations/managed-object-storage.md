# Managed Object Storage plugin foundation

Internal implementation and acceptance record, 2026-10-25. This is the native
Managed Object Storage foundation from Task 4 of the approved
[S3 plan](../superpowers/plans/2026-10-25-s3-managed-storage.md), not completion of
the [full storage handover](../handovers/2026-10-25-s3-managed-storage.md).
No Wasabi account, retained service or production deployment was changed.

## Product and authority contract

`plugins/object-storage` is an independent Rust workspace, native plugin runtime,
private metadata migration, provider controls and optional Console contribution.
Its one offering is `bucket`. Core owns installation, live membership, action
admission, self-service features and resource indexing. The plugin additionally
matches persisted customer and project before every operation. A transferred
project cannot silently adopt another customer's retained resource.

Each resource permanently binds a generated UUID, customer, project, environment,
backend fingerprint, generated bucket and generated provider username. Customers
submit a display name and environment label on creation. They cannot submit
bucket names, endpoints, account identities, IAM policies, public ACLs or retention
locks. Backend configuration is private. Its identity hashes explicit endpoint,
region, account label, backend ID and provider kind; changing that identity does
not redirect existing resources. Keep the same account identity across admin-key
rotation. The account label is an operator binding, not independently verified
Wasabi account discovery.

Core invocation claims currently provide no independent environment authority.
The immutable environment is a binding/label, not an additional authorization
promise. Permanent application credentials grant project application data access;
they are not membership-sensitive browser download grants. Private binary plans now target the native Core Storage gateway. Public callers receive
Core descriptors only; provider URLs stay private. Browser transfer controls still
await the parent Console integration. Browser CORS configuration, Object Lock and
compliance retention are unavailable. Large object bytes never enter plugin action JSON.

Every implemented action has one native REST binding and one MCP binding in
`manifest.json`. Its compiled contract is checked at runtime startup, allowing only
the release image digest to differ. `buckets.purge` uses `resource:use` and never
removes Core's index. Only verified `buckets.delete` uses `resource:delete`.
There is no resource-count guarantee from this plugin alone; concurrent Core
reservation/migration 0047 qualification belongs to the parent integration.

## Operations

| Actions | Behavior |
| --- | --- |
| `buckets.create`, `buckets.view` | Create a fresh private generated bucket/user; return safe resource output and timestamped provider observations. Never adopt collisions. |
| `credentials.create`, `credentials.rotate` | Generate at most two application keys for the exact restricted user. Rotation overlaps; it does not revoke the old key. |
| `credentials.reveal` | Explicit synchronous write-authorized reveal of a sealed, still-existing key. The engine never journals the secret. Console Reveal/Hide uses the parent cache-free `usePluginReveal`; outer REST/MCP no-store/log-redaction behavior still needs signed-install acceptance. |
| `objects.upload`, `objects.download` | Side-effect-free private plans for exact bounded objects; Core returns public transfer descriptors and rechecks private authorization throughout the transfer. See the integration contract below. |
| `credentials.revoke` | Explicitly delete an exact known key and observe its absence. |
| `versioning.set` | Enable or suspend versioning; verify the observed setting. Suspension does not delete existing versions. |
| `lifecycle.preview`, `lifecycle.set` | Preview a typed whole-bucket policy or explicitly apply current/noncurrent expiration and supported multipart abort days. Empty policy removes lifecycle. Preview supplies no deletion count or billing guarantee. |
| `buckets.deactivate` | Suspend destructive lifecycle first, revoke all application keys and verify both. Retain completed objects, versions and the bucket; incomplete multipart uploads remain subject to provider cleanup (see below). |
| `buckets.reactivate` | Return an inactive resource to active without restoring old keys or old lifecycle. |
| `usage.inventory` | One bounded metadata page per explicit request. Persist private cursors and observed category totals. |
| `buckets.purge` | Inactive bucket only, exact resource ID confirmation; one explicit bounded deletion/abort page. Return `purge_complete`; this is separate from bucket deletion. |
| `buckets.delete` | Inactive bucket only, exact confirmation; independently walk empty inventory categories, require no keys, delete bucket/user/policy and verify absence. |
| `buckets.reconcile` | Explicit conservative recovery to inactive when the owned resource and safe outcome are provable. Never blindly repeat creation. |

Mutation inputs require UUID `request_id` and the currently observed integer
`revision` (create has no prior revision). Reusing the same request ID with changed
scope/action/input is rejected. Identical completed requests replay metadata only.
Pending requests do not replay external effects. Keep both the plugin request ID
and Core submission idempotency key for a retry after a lost response. A replayed
create cannot resurrect a deleted tombstone.

State `active` means an enabled resource, not a hard uptime guarantee. `observed`
contains the latest successful timestamped provider observation, or null when an
observation failed. A missing/foreign provider resource moves to
`needs-reconciliation`. View does not repair pending operations. Inventory is
observed and soft, with current bytes/objects, noncurrent bytes/versions, delete
markers and incomplete multipart bytes/uploads, start/freshness timestamps and
an explicit completeness flag. It is not an atomic snapshot, quota enforcement
or a billing measurement. Concurrent writes/deletes/completions can invalidate a
scan. Expected invalid upload/version cursors reset the scan and return `scan_reset:true`,
without deactivation, key revocation or purge. Other scan errors also discard the
cursor without forcing resource reconciliation; start again with a new request ID.
A completed scan starts afresh on the next request. Partial pages remain explicitly partial.

Deactivation removes configured destructive lifecycle rules; it does not override
provider retention behavior. Wasabi automatically deletes incomplete multipart
uploads after approximately 31 days even without a lifecycle policy. Retained
completed objects and versions remain protected from plugin lifecycle expiration;
incomplete uploads have no indefinite retention guarantee. See
[Wasabi multipart cleanup](https://docs.wasabi.com/docs/how-does-wasabi-handle-multipart-uploads).

## Private metadata, fencing and recovery

The default and only metadata runtime uses the physically separate private
PostgreSQL database and retained role supplied by native installation. Startup
uses plugin-kit storage migrations (`storage`/`storage_tls`); each operation opens
its own connection through `webdock_plugin_kit::postgres::connect`. Remote
connections require `storageCa`, verify the driver's actual hostname and have no
plaintext fallback. Without a CA every configured host/address must be loopback.
There is no metadata directory, PVC, SQLite dependency or second retention path.
This plugin/migration is unreleased; the initial migration is PostgreSQL-native,
with no automatic import of earlier local test SQLite files.

A stable database-scoped session advisory lock serializes observations, revisions
and provider effects across processes and runtime generations. Contention fails
closed. Every fence owns a dedicated connection; no locked session is returned to
a pool. Cancellation/drop closes it and PostgreSQL releases its lock. There is no
expiring lease or application-configured idle-session timeout. A five-second SQL
statement timeout bounds metadata calls; it does not expire the session fence
while the provider is working. Database/network loss may close a session while an
external request is unresolved. Every provider call checks the live fence before
starting, and a monitor checks it every 50 ms while awaiting the call. Connection
loss cancels the provider future and prevents later calls in that sequence.
Committed intent includes a durable `executing` marker. Only the original healthy
session can clear it after the execution returns. A new process cannot reconcile,
reactivate, reveal or mutate that resource while the marker remains set, even
after the advisory lock is released. A crash, timeout, cancellation or fence loss
therefore requires private operator recovery; the marker never expires into an
automatic takeover. The operator must stop the original runtime and establish the
outcome of any in-flight external request before repairing metadata. Dropping an
HTTP future does not cancel a request already received by S3/IAM. This is not a
distributed transaction with the provider.

Under the session fence, a short transaction commits resource state, incremented
revision and operation fingerprint **before** any external mutation. The session
lock survives that commit and the later completion transaction. Completion
atomically saves the resource and safe result; reveal secrets never enter the
operation journal. `synchronous_commit=on` is required for effect intents. An open
transaction is never used to hide intent until after an external create. Schema
migrations use the existing plugin-kit advisory migration lock and transactional
version journal; startup rejects a newer schema. PostgreSQL role/default privileges,
backups, durable storage and private CA trust remain the native platform's duties.

AES-256-GCM reuses plugin-kit Sealer. Its AAD identity is derived from the complete
immutable binding and access-key ID, so moving encrypted secret bytes to another
customer/project/environment/backend/bucket/user/key fails. The encryption key,
installation database/role, private CA trust and immutable backend binding must survive deactivation, runtime generation changes, restart and
metadata recovery. Reuse native retained identity; do not generate a new database,
role or sealer key on every generation. Back up the private database, configuration
and encryption key together through the native private recovery process. Do not
rotate the sealer key by replacing it without migration.

Before external mutation, commit the next revision, request fingerprint and
`needs-reconciliation` intent. Cancellation, timeout, SDK error or failed final
observation leaves this state. SDK mutation retries are disabled, including IAM
CreateAccessKey; Wasabi supplies a secret only at creation. Never infer an absent
key means a timed-out create finished. Explicit key reconciliation can revoke an
observed unknown generated key; if no settled key is observable, recovery stays
blocked. A successfully returned key is sealed and saved with explicit
`returned_key` evidence before the final ownership observation. If that observation
fails on a healthy fence, conservative reconciliation can revoke the known key
and settle inactive without repeating creation. Empty/duplicate credential
responses do not receive this success evidence. Interrupted provisioning, provider setting changes, purge and final deletion
require private operator investigation. There is no general-purpose automated
adoption/retry escape hatch. Failed original requests remain non-replayable even
after explicit recovery under a fresh request ID.

A private operator must establish the exact provider outcome, binding/tag,
identity, policy, application keys, lifecycle and actual bytes before repairing
metadata. A lost create response is not evidence of an empty/nonexistent bucket.
A lost final delete response must not remove Core's resource index until both
bucket and managed identity absence are verified. Retain metadata and diagnostics
when this proof is unavailable. This foundation does not claim transactional
fencing inside S3/IAM or against an external account administrator.

Each engine invocation is capped at 45 seconds; native action runtime admission
currently has the stricter 30-second action cap. Each SDK request is capped at ten
seconds. Version inventory pages contain at most 100 records. Multipart discovery
requests 100 upload records; Wasabi responses above that bound are rejected.
The local MinIO version ignores the bucket-wide discovery page limit and markers,
so only that test backend allows a bounded buffer of up to 1,000 uploads; larger
responses are rejected without deactivation. Discovery buffers stay in private
persisted cursors and process one upload at a time, with part pages of at most
100 records. This local compatibility limit does not qualify Wasabi pagination. Purge
pages delete at most 100 versions/markers or abort at most ten uploads. No page
loops autonomously into a schedule. There are no declared jobs or schedules.

## Private runtime configuration

The executable accepts one absolute path to an owned private JSON file. It uses
plugin-kit RuntimeTransport and the normal RemoteAuthority for Unix sockets or
explicit mTLS. Startup failures print only `object_storage_plugin_start_failed`.
Provider error bodies, URLs with credentials and configuration are never logged.

Private fields, in camelCase:

- `manifest`, `issuerPublicKeyHex`, `generation`, `corePeers`, and either
  `listen`/`core` Unix paths or `listenTls`/`coreTls`, following the existing
  PostgreSQL/Mail native runtime contracts.
- `storageUrl`: retained installation PostgreSQL URL, including the private role
  credentials. Never expose it in readable plugin configuration or resource output.
- `storageCa`: optional absolute path to the owned PostgreSQL CA PEM; required for
  non-loopback storage. The CA file is read with the same ownership/integrity checks
  as the PostgreSQL runtime. `metadataDirectory` is rejected as an unknown field.
- `secretKeyHex`: preserved 32-byte hex encryption key, entered privately.
- `backend`: `id`, `kind`, `endpoint`, `region`, `account`, `accessKey`, `secretKey`.
  No ambient AWS profile, environment credential chain or instance metadata lookup.

Illustrative metadata fields within the private runtime configuration (placeholders
must be supplied by the native installer; this is not a deployable configuration):

```json
{
  "storageUrl": "postgresql://<retained-role>:<private-password>@<private-host>:5432/<installation-db>",
  "storageCa": "/run/webdock/runtime/storage-ca.crt",
  "secretKeyHex": "<retained-64-hex-sealing-key>"
}
```

`kind: wasabi` requires explicit HTTPS S3 Wasabi endpoint and region. IAM always
uses **https://iam.wasabisys.com**, with explicit private credentials; it never
falls through to AWS IAM. Users have a generated `/webdock/<resource UUID>/` IAM
path and one generated restricted bucket policy. Application policy permits only
exact-bucket listing/location/multipart metadata and exact-bucket object/version
read/write/delete/abort. It does not grant account administration, bucket policy,
ACL, lifecycle, versioning, Object Lock or other bucket access. Fresh buckets use
private defaults; observations reject public/group/foreign canonical ACL grants
and unexpected bucket policies.

`kind: minio-local` is explicitly limited to loopback endpoints and requires the
additional `minio: { executable, configDir }` private helper configuration. It uses
the pinned official mc release below, which implements madmin/signing. Root
credentials live in the matching private `configDir/config.json` alias `local`.
New parent-user passwords enter mc through stdin. Generated service-account
secrets are read through a bounded private pipe. Neither is passed in argv or
logged. Child processes receive only the explicit mc configuration and inherited
home path; arbitrary ambient mc/AWS settings are cleared. The native production
plugin has no Docker dependency.

Local limitations: this MinIO version returns a canonical ACL placeholder without
a grantee ID, and filters ListBuckets to allowed buckets. Acceptance checks that
no foreign bucket is returned. It drops multipart-abort lifecycle configuration,
so the local backend rejects that setting before any policy mutation. Current and
noncurrent expiration are observed after apply; explicit multipart abort/purge
and multipart byte inventory are implemented. Local admin behavior does not
qualify Wasabi IAM or Wasabi ACL/lifecycle compatibility.

## Verification and local acceptance

SDK versions are pinned to `aws-sdk-s3 = 1.137.0` and `aws-sdk-iam = 1.113.0`.
The latest inspected releases require Rust 1.94.1; these pinned service releases
require 1.91.1. The plugin lockfile was resolved with Rust 1.94 compatibility
fallback, and all resolved package MSRVs were checked at or below 1.94.0.
The integration and fixture suites now compile and pass on the actual Rust 1.94.0
toolchain, using an isolated target directory. Formatting used the installed 1.98.1
rustfmt without changing global toolchain defaults.
Do not regenerate this standalone lockfile without rechecking transitive MSRVs.

Official mc release: `RELEASE.2025-08-13T08-35-41Z`;
image `quay.io/minio/mc@sha256:a7fe349ef4bd8521fb8497f55c6042871b2ae640607cf99d9bede5e9bdf11727`;
Linux amd64 executable SHA-256
`01f866e9c5f9b87c2b09116fa5d7c06695b106242d829a8bb32990c00312e891`.
The fixture extracts only this already-present image without starting it, removes
the extraction container and verifies the executable hash. It does not install a
global tool or start/restart any existing service.

From the repository root, with Docker and the already available local test image
`webdock-dev-minio:local`:

```sh
python3 plugins/object-storage/fixtures/fresh.py
```

This runner creates a fresh loopback MinIO container with its own volume and
private credentials, extracts the pinned mc binary into an isolated private
configuration directory, and invokes the PostgreSQL runner. It never reads or
changes a retained MinIO configuration or service. Success removes only its exact
label-verified container, volume and private directory; failure retains the exact
owned fixture for investigation. The earlier `prepare.py` plus `postgres.py --minio`
workflow remains available only when explicitly authorized to use an existing fixture.

The PostgreSQL runner creates one uniquely named, loopback-only `postgres:18-alpine`
container with its own anonymous volume and private random credentials. Every test
creates a fresh random database only within that container. It starts, restarts and
removes only that fixture; it never starts/stops MinIO or connects to existing
PostgreSQL services. It enables the PostgreSQL lifecycle tests, runs MinIO acceptance
when requested, then runs two independent test processes around an owned PostgreSQL
server restart. On success it removes the exact container, volume and private
configuration. Omit `--minio` for PostgreSQL/controlled-provider checks alone.

Tests cover independent sessions/processes, committed pending intent visible to a
separate session while an external operation is suspended, cancellation releasing
the fence without replaying that effect, migration/version rejection, scoped replay,
retained credentials after engine/server restart, and the existing lifecycle and
identity transplant checks. The server restart test uses synthetic pending state
and a test-only repair; it does not supply a production recovery escape hatch.
Plain `cargo test` deliberately ignores DB-dependent entries unless run by this
fixture; a green ordinary run alone does not qualify PostgreSQL behavior.

The opt-in MinIO test uses only fresh random owned buckets/users. It checks restricted
cross-bucket and management denial, permitted application transfers, versions,
noncurrent bytes, incomplete multipart bytes, key overlap/revocation, unsupported
local lifecycle rejection, retained data on deactivate, reactivation without old
keys, nonempty-delete denial and final exact cleanup. Failed runs retain their
exact private metadata database/container, fixture URL file and sealer key. The
runner prints only the exact container/directory names, never credentials.

For an interrupted MinIO fixture, keep that owned PostgreSQL container available
and point `WEBDOCK_OBJECT_STORAGE_RECOVERY` at its exact `acceptance-<uuid>`
directory and `WEBDOCK_OBJECT_STORAGE_MINIO_CONFIG` at the same private backend
file. Run only the ignored `recover_exact_owned_fixture` test. Recovery reads the
owned private `storage.json` in that acceptance directory, verifies the saved
binding/tag and fixture policy, and removes only those recorded buckets/users.
This is test cleanup, not a product adoption or production recovery API. Once exact
provider cleanup succeeds, remove only the runner's recorded container using
`docker rm --force --volumes <exact-owned-container>` and its exact private fixture
directory. Never select resources with broad bucket/user/container listing or
pruning. Earlier SQLite acceptance directories require their earlier test tooling;
the PostgreSQL runtime does not open or migrate them.

Qualification caught and corrected: mc needs its inherited home path despite an
explicit config directory; successful empty service-account lists emit no JSON;
MinIO filters bucket listing; multipart-abort lifecycle is not retained; MinIO's
owner ACL grantee omits the canonical ID. The final completion assertion also
caught a multipart cursor that repeated the last upload; inventory now follows
provider continuation markers and stops when the upload page is not truncated.
MinIO may return multiple uploads despite a requested one-upload page; bounded
batches are now persisted and fully consumed instead of discarding extra records.
Acceptance now also covers 105 versions, a delete marker and null version, 101
uploads, and 101 parts on one upload. Concurrent abort/completion invalidates
persisted parts/batch cursors and resets the scan without revoking keys. Concurrent
object/version changes settle on a fresh scan. Scripted SDK responses separately
exercise standard upload continuation markers past 100 uploads and partial purge
errors. MinIO also ignores the requested purge upload count, so abort processing
explicitly takes at most ten returned uploads per request. Failed local attempts
retain only their exact owned fixtures until explicit cleanup.

Verified commands in this worktree (Rust 1.98.1):

```sh
cargo build --manifest-path plugins/object-storage/Cargo.toml --locked -j 2
cargo test --manifest-path plugins/object-storage/Cargo.toml --locked -j 2
cargo clippy --manifest-path plugins/object-storage/Cargo.toml --locked --all-targets -j 2 -- -D warnings
cargo clippy --manifest-path plugins/object-storage/Cargo.toml --locked --no-default-features --all-targets -j 2 -- -D warnings
cargo fmt --manifest-path plugins/object-storage/Cargo.toml -- --check
node_modules/.bin/tsc -p plugins/object-storage/ui/tsconfig.json --noEmit
python3 plugins/object-storage/fixtures/fresh.py
bun test --tsconfig-override apps/console/tsconfig.json ./plugins/object-storage/tests/ui.test.tsx
cargo test --manifest-path plugins/object-storage/Cargo.toml --locked --no-default-features -j 2
```

Round 3 first reproduced the terminated-session takeover, known-returned-key
reconciliation failure, invalidated scan and UI action-switch/create-form failures.
Additional tests first failed on SDK NoSuchUpload/invalid version markers,
unbounded local purge aborts, and prematurely active key-success evidence.
The fixes keep durable execution uncertainty closed, preserve sealed returned
credentials, reset nondestructive scans, and freeze the full UI attempt.

Final round-3 verification passed build, Clippy in default and no-default modes,
formatting and the Console typecheck. Default tests passed ten ordinary entries
and ignored 24 fixture/helper entries; no-default tests passed two ordinary
entries and ignored 22. The UI harness passed two tests using the real Console
submission hook with only the HTTP boundary controlled. Fresh acceptance passed
30 suite entries, three helper-process invocations and two server-restart phases.
It removed its exact PostgreSQL/MinIO containers, volumes and private configs;
the earlier reproduction fixtures were also removed by exact name and verified
ownership label. No retained MinIO service was read or changed. Ordinary
`cargo test` deliberately leaves fixture tests ignored; the fresh runner is required
for actual PostgreSQL/MinIO evidence.
This paragraph records the earlier round-3 baseline. The integration verification
below supersedes its pending Rust 1.94/image/reveal items. Browser rendering, signed
installation and Wasabi live qualification remain outstanding. No production or retained MinIO service was used.

## Parent integration still required

1. Build/sign/install/register an immutable release and add the Console module to
   the existing extension registry. The manifest image is a zero-digest placeholder;
   no release or registry file was changed by this task.
2. Supply private backend configuration alongside the native retained
   `storageUrl`/`storageCa`/`secretKeyHex` identity. The plugin now consumes the
   existing per-installation PostgreSQL contract and requires no metadata PVC.
   Exercise signed install, deactivate/restart/generation upgrade and metadata
   recovery end to end before claiming managed installation qualification.
3. Coordinate with and qualify Core's concurrent resource-count reservations. Do
   not advertise count caps from this plugin alone. Exercise live signed installation/admission,
   membership revocation, resource index replay and full Console narrow/wide flows.
4. Qualify the implemented Reveal/Hide UI through signed installation and actual browser
   rendering. The mounted component uses `usePluginReveal`, validates the synchronous
   `access_key`, `secret_key`, `endpoint`, `bucket` reply, and never uses a query or
   mutation cache for secrets. It is available only to writable `manage-buckets`
   users on active buckets. Hide aborts pending reveal; resource/project/key/revision,
   principal/scope changes, loss of permission and unmount clear mounted secrets and
   copy feedback. Existing native Create and frozen full-attempt retry remain intact.
5. Wire and qualify Core binary transfer using the exact contract below. The current
   Core conditional-header allowlist needs completion before either producer can be
   consumed. No typed Console transfer helper is exported in the current SDK, so this
   UI deliberately exports no fictional upload/download helper. Parent wiring should
   call the producer actions through the ordinary native action boundary and consume
   only the Core descriptor via its eventual typed binary helper.
6. Qualify Wasabi trial IAM, region, ACL/tagging/version/lifecycle behavior,
   two-key semantics, outage/recovery, partner/resale rights, WACM/account topology,
   economics, CORS and operational recovery on explicitly owned bounded fixtures.
   No live Wasabi compatibility, resale eligibility, public rollout, hard quota or
   complete S3 handover claim is made here.

Official references checked during implementation:
[AWS Rust SDK](https://github.com/awslabs/aws-sdk-rust),
[S3 pinned release](https://docs.rs/crate/aws-sdk-s3/1.137.0),
[IAM pinned release](https://docs.rs/crate/aws-sdk-iam/1.113.0),
[Wasabi service endpoints](https://docs.wasabi.com/docs/service-url-endpoints),
[Wasabi two-key/creation-only-secret rule](https://docs.wasabi.com/docs/creating-a-new-access-key),
[official mc release](https://github.com/minio/mc/releases/tag/RELEASE.2025-08-13T08-35-41Z).


## Storage producer and Core integration contract (2026-10-25)

Exported Rust APIs are `Engine::prepare_data(customer, project, StorageDirection, Value)
-> StoragePlan` and `Engine::authorize_data(&StorageAuthorization) -> Result<()>`.
The native `Handler::authorize_storage` implements the final authenticated runtime
callback; the standard runtime checks plugin generation, peer and action admission.
No new public endpoint exposes this private callback.

Actions are `objects.upload` (`storage:upload`, write intent,
`plugin.storage.write`) and `objects.download` (`storage:download`, read intent,
`plugin.storage.read`). Both use `resource:use`, feature `manage-buckets`, and required
host service `{name:"storage",version:1}`. REST bindings are POST `/objects/upload`
and POST `/objects/download` relative to the plugin API namespace. MCP names are
`objects_upload` and `objects_download`. The manifest output describes the **public**
`TransferDescriptor`: version, id, path, method, expiresAt, exactBytes,
expectedSha256, attachment {filename, contentType}. It never describes a private
provider URL or headers. Current protocol manifest property names accept lowercase
only; therefore camelCase descriptor fields are documented and governed by the
exported typed ABI, while the compatible schema explicitly validates version/id/
path/method/attachment/filename. After a protocol property-name update, tighten
that schema to all typed fields and `additionalProperties:false`.

Both inputs require `resource_id` (resource UUID), `key` (1–512 ASCII letters,
digits, `/._-`, no empty/dot/dot-dot segments), `exact_bytes` (0–536870912),
`expected_sha256` (64 lowercase hex digits), `filename` (1–160 safe ASCII filename
characters), `content_type` (the ABI's six safe attachment media types), and
`expires_at` (Unix seconds, in the future and at most 300 seconds away). No method,
endpoint, bucket, arbitrary headers, credentials or bytes may be submitted.
Download additionally requires `version_id` (1–512 bounded ASCII version characters;
`null` explicitly selects the provider null version) and `if_match` (quoted bounded
hex/multipart ETag). Upload rejects both fields: it is create-only, with signed
`If-None-Match:*`. Download signs exact key/version plus `If-Match`, and independently
HEAD-checks exact size, version, ETag and full SHA-256 checksum. An object without a
provider full SHA-256 checksum is unavailable through this producer; an ETag or
caller-supplied metadata is never treated as a checksum. Multipart composite
checksums do not qualify as a full-object SHA-256 here.

All signing uses the pinned maintained AWS SDK `presigned` operation. PUT signs
exact content length, SHA-256 checksum, safe content type and create-only condition;
GET signs exact version and ETag condition. Host and content length are supplied by
Core's pinned binary transport; the remaining SDK headers are preserved. Never
remove a conditional header to make the transport accept a plan.

**Current Core integration blocker:** `crates/plugin-http/src/storage.rs` allows only
`content-type`, `x-amz-checksum-sha256`, `x-amz-content-sha256`. It must additionally
allow and forward `if-none-match` for PUT and `if-match` for GET, with method-specific
validation and regression tests. The unchanged allowlist denies the producer's
correct signed headers. Until that is qualified, direct fresh-MinIO presign evidence
is not end-to-end Core gateway/browser evidence. No shared Core files were changed.

Private resource authority is SHA-256(binding, revision), serialized as
`resourceEpoch`. Existing mutations durably increment the revision before provider
effects, so deactivation, reactivation, key changes and every other mutation
invalidate earlier tickets. This conservative policy also invalidates tickets on
inventory/config mutations. Plan preparation checks persisted customer/project,
backend identity, generated bucket/user, active/provisioned/nonpending/nonexecuting
state and fresh provider ownership. It reads the snapshot again after presigning
and rejects a changed epoch/state. Preparation and callback never take the global
mutation fence or write an operation record. `Store::read` uses an independent
PostgreSQL connection and 500ms SQL timeout. The callback checks fresh private
ownership/state/epoch and expiry, not an old observation or successful signature.
The runtime caps it at one second. Missing/unavailable metadata denies access.

Retained deactivation leaves Core's resource index intact. Core must invoke this
callback before consumption and throughout staging/delivery, including before
final delivery; the issuer's plugin PG state is required in addition to Core actor,
membership, feature/grant, generation and session checks. Provider admin signatures
are never exposed publicly and are not independently revoked by deleting an
application key. Their safety depends on Core keeping them private and enforcing
these live callbacks. Committed pending deactivation immediately fails revalidation,
without waiting behind the provider operation's global fence. Reactivation cannot
resurrect a prior epoch. This does not cancel a remote S3 request already received.

Core private configuration needs `storage.stagingDirectory`, an existing owned
0700 directory, and `RuntimeBinding.storage[projectID]["managed-objects"]` with
`origin` (the configured provider origin), `addresses` (explicit pinned IPs), `ca`
(optional private CA PEM), and `pathPrefix`. For a single exact resource use
`/wd-<resource UUID without hyphens>/`; a project alias serving several buckets can
use `/` on a dedicated pinned backend origin, relying additionally on the plugin's
private exact ownership/key checks. Current alias is a constant `managed-objects`;
a narrower per-bucket alias would require coordinated binding provisioning.
Production uses HTTPS. HTTP is permitted only by Core's explicit loopback test
constructor, never by normal production binding. Keep private runtime
`storageUrl`/`storageCa`/`secretKeyHex` and backend configuration separate from these
Core transport pins. Kernel limits are 512MiB/file, four transfers, 2GiB anonymous
private staging and at most 300-second plans. No S3 build-cache feature is added.

## Standalone image and new verification

`Dockerfile` uses immutable official Rust 1.94.0 slim-trixie and Debian trixie-slim
multi-platform base digests, with CA certificates installed and verified present.
It compiles from the root context with the standalone plugin lockfile, copies only
the executable into the final image and runs as UID/GID 65532. The adjacent ignore
file defaults to denying every path, including descendants of traversable parents.
Only root Cargo workspace/lint metadata, the three native plugin library source
sets, their manifests, the native protobuf, plugin source/manifest/lock and migration
enter the context. Root Cargo.lock, unrelated Core code, other plugins, handovers,
fixtures, logs, environment files, private configs, TLS keys and targets are excluded.
The `source` target permits checking the actual filtered context before compilation.

```sh
CARGO_TARGET_DIR=/tmp/<unique-owned-target> cargo +1.94.0 test --manifest-path plugins/object-storage/Cargo.toml --locked -j 2
RUSTUP_TOOLCHAIN=1.94.0 CARGO_TARGET_DIR=/tmp/<unique-owned-target> python3 plugins/object-storage/fixtures/fresh.py
CARGO_TARGET_DIR=/tmp/<unique-owned-target> cargo +1.94.0 clippy --manifest-path plugins/object-storage/Cargo.toml --locked --all-targets -j 2 -- -D warnings
bun test --tsconfig-override apps/console/tsconfig.json ./plugins/object-storage/tests/ui.test.tsx
bunx tsc -p plugins/object-storage/ui/tsconfig.json --noEmit
docker build --iidfile /tmp/<owned-image-id> -f plugins/object-storage/Dockerfile .
python3 plugins/object-storage/fixtures/image.py <exact-local-sha256-image-ID>
```

Fresh PG/MinIO integration passed on actual Rust 1.94.0. Tiny (three-byte) presign
fixtures verify signed method/length/checksum/key substitution denial, exact upload,
create-only overwrite rejection, exact null-version download and wrong version/
ETag/size/hash denial. Existing large pagination, retention, sealed credential,
fence-loss/durable execution, scan reset and returned-key recovery tests still pass.
Additional PG tests verify no private plans in durable results, wrong-scope/inactive/
pending/expired plans, callback checks while the global fence is held, and actual
issued-ticket denial while deactivation is suspended at the provider, followed by
non-resurrection on reactivation. Reveal tests check explicit-only secret responses,
safe errors/status/operation results and no reveal journal writes. The four UI tests
cover existing Create/retry plus reveal/Hide, aborting late responses, key/resource/
feature changes, query/mutation-cache exclusion and cleared copy feedback. Signed
installation, actual browser flows, full Core binary transport with these conditional
headers and live Wasabi compatibility remain parent integration work.

Local image verification completed without a registry push:
`webdock-object-storage:integration-20261025-r1`, Docker image/index identity
`sha256:c48c275a29a0adf98f831df431a8fe9f55b2aa73177c09672adbcf8daecd457c`
(amd64 manifest `sha256:f23e91bf09d01d985f333dd07b6dbf649b36d524bee76c6b3cfa9055c72276fa`).
The bounded smoke verified the actual 35-file source context, UID/GID 65532,
read-only root (including failed `/tmp` writes), installed trust-bundle verification
of ISRG Root X1, and the executable's redacted missing-config failure. It did not
replace signed native startup with a real Core listener/private configuration.
That startup still needs the native private config/CA mounts and, for Unix
transport, its writable socket directory; it needs no local metadata volume.
