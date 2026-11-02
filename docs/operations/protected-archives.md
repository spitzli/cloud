# Protected export operations

The archives product reuses Core resource reservations, current actor/session authority,
immediate jobs, audit/operation receipts and the binary storage gateway. The plugin
stores private output receipts/epochs; no autonomous schedule or standing browser
principal is created. Native migrations 0046–49 remain immutable. Forward 0050 adds
repository `plugin-job` source lineage while preserving classic Session/Build/Release;
0051 is a separate backup transition migration owned by that product.

## Package and install

Build the production Dockerfile from a reviewed checkout. Its adjacent default-deny
context includes only the five necessary crates/protos and the production runtime
manifest/lockfile; protected handovers, private env/CA keys and test dependencies are
excluded. Bases are pinned by digest; protobuf compiler is pinned to
`3.21.12-11+deb13u1`. Runtime is official PG18.6 with UID/GID 65532, a fixed archives
entrypoint and public root CA bundle from the pinned Rust base. It starts no PG server.

Publish only to the intended authenticated registry, record OCI child/config/layer
and executable digests, and produce the existing signed native release/catalog review.
Use the normal operator install and agent promotion. Public configuration is `{}`;
capabilities and the `archive` offering/`exports` feature come from the reviewed manifest.
No production activation/signing approval is implied by local fixture evidence.

Recipe: private PostgreSQL metadata (`storage: true`), `sealingKey: false`, private
configuration containing `backend` and `stagingDirectory: "/tmp/webdock-archives"`.
`databaseSources` may be omitted or empty only; nonempty legacy profiles fail startup,
even with fixture features compiled. Metadata URL/CA, runtime
mTLS identity, pinned Core channel and issuer key are supplied by the native installer.
The plugin creates only the private staging leaf (0700), rejects symlinks/untrusted
ownership, and uses anonymous 0600 output files. Run the managed workload with readonly
root filesystem, bounded `/tmp`, and the existing approved egress controls.

## Core configuration

Enable the fixed private source lane with `exportSources: true` in the owned native
plugin configuration. Use the same native database encryption secret as the existing
DatabaseService (canonical `WEBDOCK_HOSTING_ENCRYPTION_KEY`, otherwise configured Auth
secret), the existing database gateway origin, native identity and restricted pools.
This reader is a composition of the existing DatabaseService and RepositoryAdmission,
not a second source/auth database.

Configure the existing repository descriptor/signer/transport bindings for committed
source. `RepositoryContext::PluginJob` carries the actual Core invocation/job ID,
attempt, actor/session and original enrolled peer. The lease is clipped to that job's
expiry. Every provider callback and stream chunk rechecks current job/claim, runtime,
feature, source/provider grant/configuration and actual browser/OAuth authority. No
Session context is fabricated to bridge a job.

The private `ExportSources` v1 protocol is additive and independent of the frozen
PluginRuntime/storage/database backup ABIs. List/bind accepts only source IDs and enums;
source bytes use streaming RPC. Confidential database material can only be obtained by
an exact running `archives.produce` attempt, never by a public action output or job
payload. Download source checks authenticate the actual Core-sealed transfer and its
original current actor before testing the retained source identity.

Core gateway configuration remains `storage.stagingDirectory` (owned 0700) plus
`targetPolicy.storage[projectID]["archives"]` with exact HTTPS origin, pinned addresses/
CA and bucket path prefix. This is distinct from the producer's staging directory.
Neither target is selected by public action input.

## Managed PostgreSQL sources

A source must be a real active project/customer's confirmed dedicated installation:
ready database/backend, observed current revision, no pending backend, confirmed
credential revision, exact backend/CNPG identity, validated CA and endpoint. Admission
and download require the current actual actor's `org:databases:manage` permission.
Changing source ownership, backend/CA/revision, credentials or readiness invalidates
old bindings. Native pause disables new references/material and outstanding access.

Native create/reconcile/rotate/restore supplies `wd_export_<databaseID>`, separately from
app/migration roles: NOINHERIT, no memberships, no superuser/create role/create DB/
replication/RLS bypass; only CONNECT and SELECT/USAGE on the exact tenant `app` schema
and future objects. The readonly password is HMAC-SHA256, domain-separated by
`webdock.database-export.readonly.v1\0` plus database ID, keyed by the native canonical
256-bit getrandom application credential. Caller/import passwords are not accepted by
native managed create/rotate/upgrade contracts. Existing unsafe/unreconciled sources
fail clearly; no application/migration credential fallback exists. Source material is
transient in the private channel/process environment and never persisted into plans,
JSON responses, debug output or archives.

Before dump the producer verifies TLS and the actual role, including no inherited or
write/DDL authority even if transaction readonly is disabled. Only `app` is exported.
pg_dump uses no credential argv, a private temporary public CA, fixed flags and finite
connection/lock/statement/process limits. This is a logical snapshot export, not PITR,
RPO/RTO certification or a shared-table tenant export.

## Reports and diagnostics

Observed project CSV is produced from Core reads, not submitted claims. User-editable
project/resource names are omitted entirely; spreadsheet formulas beginning `=`, `+`,
`-`, `@`, tabs or control characters therefore never enter cells. Retained cell types
are decimal IDs, fixed/validated slug categories, nonnegative counts and clock values;
CSV quoting is not used as a substitute for formula safety. More than 100 resource
category groups fails rather than silently truncating the report.

Submitted diagnostics remain a distinct typed product: version 1, finished state,
up to 100 decimal build summaries and 32 checks, bounded numeric counters and small
enums. No arbitrary strings/logs/exception text, URLs, tokens, credentials or mail
bodies are accepted. Build logs use the separate potentially sensitive export below;
they never enter this diagnostic allowlist. Live search/tailing/monitoring remain outside S3.

Committed source archives contain the provider's existing normalized immutable commit
files, compressed as `project.tar.gz`, with the resolved commit retained in the receipt.
They do not include `.git` history, runtime volumes, database contents or keys. Submodule
contents and LFS hydration are not performed or guaranteed; verify provider payloads
and pointers before treating a source archive as a complete development checkout.

## Stored completed build logs

Select `kind: "buildlog"` and a genuine build ID from `archives.sources`. The archive
reads actual stored Core `git_build.logs`; public actions and Core job payloads contain
IDs/metadata only. Private source bytes use existing `Stream`/`Validate` RPC, then
anonymous staging, gzip, verified S3 storage and the authenticated opaque gateway.
Running/queued/cancelled, foreign, missing/unreceipted or disabled-source records are
ineligible. Both succeeded and failed builds with real completion hashes/finish times
are supported; a failed build can have a successfully generated log archive.

The exact stored UTF-8 string must be at most 24,000 bytes; gzip at most 64 KiB. Oversized
rows fail, rather than being silently shortened by this producer. The existing build
pipeline already limits captured logs and masks exact known build-environment values.
It does not prove complete output or remove transformed/unknown secrets. Logs are
therefore labeled potentially sensitive and possibly truncated. No historical redaction
version is asserted; the binding-format tag is not a redaction attestation. These logs
are separate from submitted, allowlisted diagnostics. Console provides a verified gzip
download and no raw text, HTML or terminal preview.

Current `org:deployments:create`, exact active customer/project, hosting installation/
mode and effective native operator rules apply at binding, read and retained access.
The fingerprint binds exact stored-byte SHA-256, completion hash, generation, terminal
status/finish time, original build/source lineage and current source/connection/
environment/hosting revisions. Original job/attempt/runtime/grant and browser/API/session
authority remain Core-owned. Each private stream metadata/data/EOF poll reacquires a
fresh authorized scope/transaction, validates the resolved source, then finishes the
original host authority; transactions are not retained across frames. Permission/source
revocation or changed stored bytes deny old descriptors and unfinished production.
Existing expiry, durable receipt, same-generation restart and exact deletion/retry rules
apply. Deleting an export leaves the original Core build-log record governed by the
Build domain's own retention; this feature does not purge that original record.

See `docs/verification/2026-10-25-completed-buildlogs-r4.md` for actual KVM/Core/native0.3/
browser proof and the separately recorded fixture-helper source closures.

## Jobs, downloads and retention

Access lifetime is 1–86400 seconds from create; output maximum is 32 MiB, with two
concurrent producers. The complete attempt respects remaining Core lease minus a
safety margin and max 35 seconds; pg_dump max 25 seconds. Project sources are bounded
before compression as well. Large/slow sources fail honestly; they need an explicitly
reviewed larger/durable execution design, not an ignored 45-second Core job budget.

A verified upload alone does not unlock downloads: Core must acknowledge the exact
resource's job as completed/ready. Status includes the actual Core job state/attempt,
private receipt-derived size/hash, expiry and sanitized failure code. Source permissions
are polled during production and checked again on plans and binary callbacks. Public
responses contain only Core opaque descriptors, never S3 locations/credentials.
Original actor/session, current membership/feature/grants/resource, producer state/
epoch and source identity must all still hold. Binary bytes are verified before release.

Retries keep exact Core/request IDs. Completed receipts survive restart and replay;
a different job cannot take over an uncertain producing record. Explicitly delete and
create a new resource after a cancelled/exhausted uncertain attempt. Delete commits a
new epoch/deleting first, then removes only the exact receipt key and observes absence
before acknowledging deleted. Failures remain deleting and deny access until explicit
retry. No public bulk-delete/GC API exists.

Production requires a dedicated private bucket with absent bucket policy, private
owner-only ACL, no previous/current versioning, and one enabled namespace-scoped
`webdock-archives-retention` rule: current expiration 1–7 days and incomplete multipart
abort after one day. Inspect permissions are needed alongside exact-prefix read/write/
delete/multipart permissions. Provider lifecycle is asynchronous; expiry is not an
immediate delete receipt. Unreceipted unique-key upload orphans rely on the verified
provider lifecycle. Receipts/tombstones remain metadata for audit.

The explicit compile-time native fixture profile only permits its exact 12-hex run
hostname `*.native-archives.test:19443`, owned `wd-archives-<run>` bucket, `archives/`
namespace, fixed hash-verified readable image CA and a <=4-hour deadline. TLS remains
verified. It permits only the observed local MinIO ACL and multipart-expiration gaps;
normal production configuration rejects that profile. Fixture cleanup stops producers,
aborts only exact owned incomplete uploads, verifies absence, then removes exact owned
bucket/user/policy/resources. It never changes the existing MinIO server or production
k3s/Kind/Contabo, and never installs object locks. Wasabi/commercial/signing inputs and
production provider qualification remain separate.

## Legacy export recovery and failed generation

Unqualified historical PostgreSQL receipts are fail-closed for generation, new download
plans and already issued binary tickets. Current managed Core source identity and
`org:databases:manage` are required for every new PostgreSQL export. Inspect status and
use the exact authorized `archives.delete` action to remove historical receipts/objects;
source reconciliation never upgrades old rows into qualified output. Expiry alone is
not an acknowledged deletion. Diagnostics intentionally remain source-free.

After source rotation or another failed Generate request, explicit Delete supersedes
the failed browser submission with its own immutable action/body/idempotency key. A
Delete retry retains that identity. A lost Generate response can mean the job was
accepted; the backend remains responsible for cancellation, mutation fencing and exact
deletion. Browser request cancellation does not promise that a backend job stopped.

Retained committed-source downloads recheck current hosting mode. Nonoperators require
`selfservice`; a current verified native platform operator may access `managed` sources.
A raw role label, preview, revoked operator mapping or expired browser/API authority
cannot provide that exemption. Old Session/Build/Release lease behavior is unchanged.

Current review-fix acceptance is recorded in
`docs/verification/2026-10-25-protected-archives-r3.md` and its sanitized JSON sibling.
The disposable native fixture has explicit finite hard eviction thresholds including
`memory.available<100Mi`; it verifies actual kubelet configuration/health and retains
its 4 GiB / 3 CPU bound. These settings apply only to its fresh owned test node.
