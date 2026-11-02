# Protected Archives

![Protected Archives icon](assets/icon.svg)

Create private, short-lived exports from resources you already have permission to use.

## What you can do

Committed source archives, observed project reports, dedicated PostgreSQL logical exports, stored completed build logs and submitted diagnostic snapshots.

## Setup and access

An operator must install the native runtime and grant Archives access in your project. Choose an authorized source, create the export, generate it, then download it before its access lifetime ends.

## Data and security

Exports use Core authority, private storage and authenticated transfers. Access expiry blocks downloads; deletion is a separate action. Build logs may contain sensitive or truncated output.

## Status and limits

Beta. Most output is limited to 32 MiB and at most 24 hours of access. Build logs have a 24,000-byte source / 64 KiB compressed limit. PostgreSQL export is not point-in-time recovery; source archives exclude Git history and runtime volumes.

Catalog publication does not activate the plugin, enable public signup or grant
access to any customer resource. A new presentation release must use a new version;
previously signed releases and their assets stay immutable.

## Gallery

Actual Archives panel; synthetic completed report and job. No export was generated or downloaded.

![Protected report status, expiry, checksum and download control](screenshots/archive-status.png)

## Technical reference

The retained implementation notes below include platform integration procedures.
Commands referring to Core, `tools/` or `apps/console` require the [platform source
checkout](https://github.com/spitzli/webdock); they are not standalone setup steps.

Native, opt-in exports using existing Core resources, durable immediate jobs and
opaque authenticated binary downloads. No second user, queue or permission ledger.

Supported producers:

- **Committed source:** normalized immutable files through the existing repository
  provider/lease protocol, with a true archives job/attempt authority. Includes no
  Git history or runtime volumes; submodule contents and LFS hydration are not
  performed or guaranteed.
- **Observed project report:** gzip CSV from scoped Core project/resource/build
  reads. Only IDs, reviewed enum/slug categories, counts and observation time enter
  cells. User-editable names, URLs, credentials and logs are excluded.
- **Managed PostgreSQL:** a real confirmed dedicated database selected by its Core
  ID. Original actor must retain `org:databases:manage`. Source binds database/backend
  UID, confirmed revisions, host/CA and tenant/project; private derived readonly
  material exists only in memory/RPC/env. Actual TLS/role preflight and app-only
  PG18 custom gzip dump; no shared-table tenant or whole runtime backup claim.
- **Stored completed build log:** actual Core `git_build.logs` from succeeded or failed
  completed builds, selected by Core build ID. Current `org:deployments:create`, tenant,
  hosting mode/native operator and source identity are rechecked. Up to 24,000 stored
  UTF-8 bytes, at most 64 KiB gzip. Potentially sensitive and possibly truncated;
  the existing pipeline masks known environment literal values only. No complete
  stdout, complete secret removal or historical redaction-version claim is made.
- **Submitted diagnostics:** a strict finite allowlist of finished decimal build IDs,
  bounded counters and enums. Unknown/free text, tokens, credentials, URLs and mail
  bodies are rejected. Submitted data does not attest observed service health.

Other output has a 32 MiB limit; build logs use the stricter limits above. Access lifetime 1–86400 seconds, two concurrent producers.
Jobs honor the remaining Core lease, capped at 35 seconds with a safety margin;
pg_dump is capped at 25 seconds. Large bytes use private source streaming, anonymous
staging, verified shared S3 transport and Core's binary gateway, never action JSON.

Actions (same manifest REST/MCP parity): `archives.sources`, `archives.create`,
`archives.generate`, `archives.status`, `archives.download`, `archives.delete`.
Create keeps its `request_id` and Core admission idempotency key on retry. Source-bound
create input adds `source_id` from `archives.sources`; a report selects the current
real project ID. Generate queues the actual Core job. Status reports producer state,
receipt/provenance and the current Core job state. Download requires **both** the
canonical completed job and live original transfer/source authority. Delete revokes
producer epoch before exact-key removal/absence acknowledgment, with explicit retry
if provider deletion is uncertain. Expiry blocks access and is distinct from deletion.

```bash
CARGO_TARGET_DIR=target cargo test --manifest-path Cargo.toml --locked --features local-test -j 2
python3 tools/tests/archives-storage.py
CARGO_TARGET_DIR=target cargo test --manifest-path Cargo.toml --locked --test repository -- --include-ignored
CARGO_TARGET_DIR=target cargo test --manifest-path Cargo.toml --locked --test reports -- --include-ignored
bun test --tsconfig-override apps/console/tsconfig.json plugins/archives/tests/ui.test.tsx
```

Production-only packaging closure (excludes Core/test dependencies):

```bash
docker build -f plugins/archives/Dockerfile -t <owned-review-tag> .
```

Pinned Rust 1.94.0, PostgreSQL 18.6 and protobuf compiler; nonroot UID 65532. The normal
build rejects fixture configuration and requires verified HTTPS, private policy/ACL,
no versioning and bounded object/multipart lifecycle. Native qualification uses only
`fixtures/Dockerfile.native` with the compile-time run-scoped HTTPS/CA profile; its
explicit MinIO ACL/multipart capability exceptions are absent from production default.
No image is signed, registered or activated merely by building it.

See `docs/operations/protected-archives.md` in the platform source checkout for operator configuration and exact limits,
and `docs/verification/2026-10-25-completed-buildlogs-r4.md` in the platform source checkout for current buildlog/native evidence
(r2/r3 retain the earlier product and review-fix qualifications).
Nonempty legacy `databaseSources` configuration is rejected, including in fixture-feature
executables. Every PostgreSQL export requires a live managed Core `source_id` and current
`org:databases:manage`. Historical unqualified rows remain visible for authorized exact
deletion/recovery, but cannot generate output, issue plans or authorize an existing ticket.
Diagnostics remain explicitly submitted, source-free structured snapshots.
