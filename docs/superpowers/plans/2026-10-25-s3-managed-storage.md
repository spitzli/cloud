# S3 and Managed Object Storage implementation plan

> **For agentic workers:** Use superpowers:executing-plans task by task. Preserve the existing worktree and unrelated changes.

**Goal:** Deliver the owner-approved storage scope in independently verified stages.

**Architecture:** A small Rust S3 data adapter handles private, bounded transfers and verified receipts. Existing products own authorization, durable operations, billing and recovery state; the adapter never grants authority. Build artifacts are the first consumer. Production activation is separate from local implementation.

**Tech stack:** Rust, Apache Arrow `object_store` S3 transport, existing PostgreSQL ledgers and plugin runtime, React/Vite Console, existing MinIO fixture infrastructure.

**Spec:** `docs/handovers/2026-10-25-s3-managed-storage.md`.

## Global constraints

- Build cache is explicitly excluded.
- No production/Contabo/Spitzli cutover, old-store migration, or broad cleanup.
- Keep immutable release provenance, current leases, current membership and runtime fences.
- No permanent S3 credentials in guest VMs, customer responses or diagnostics.
- Keep legacy local artifacts readable; a new backend never implicitly migrates/deletes them.
- Do not read or modify `CLAUDE_HANDOVER.md`, `CODEX_HANDOVER.md` or `.dev-local/demo-owner.json`.
- Owner steering: test with MinIO; Wasabi account is on a trial. Frankfurt (`eu-central-2`) is an implementation recommendation, not a confirmed existing bucket location.
- A bucket/prefix binding is not a provider policy or tenant-isolation proof.
- Neither measured storage nor Wasabi's daily quota assessment is byte-exact enforcement.

## Review focus

1. A receipt for another backend, customer, purpose or object must fail before publication.
2. Interrupted streams and failed multipart completion must not produce a successful receipt.
3. Retry after a lost completion response must preserve the existing exact-request outbox semantics.
4. Backend changes must not redirect a retained receipt or make legacy local releases globally claimable.
5. Referenced release bytes must survive restarts; no age-based release GC or irreversible lock policy.

## Inventory and implementation decisions

- HEAD at start: `819041524049117bc50e11dfd96a6a29407804b8`; already in the linked `t3code/rust-platform-rewrite` worktree.
- `ArtifactStore` validates outer tar, recursive OCI graph, exact bytes, hash and private local paths. Preserve this validation as the publication boundary.
- `BuildService::claim_release` currently requires the original build worker/generation. Cross-worker claims need an explicit durable backend binding and must retain enrollment, source, release and approval checks.
- Worker completion is already fsynced in `Outbox` before HTTP. Extend the exact receipt; do not replace this ledger with S3 metadata.
- Native migration high water at entry is 0045. New SQL is forward-only.
- `plugins/postgres` owns the new product; legacy `crates/databases` intentionally reports backup/restore unavailable. Do not enable those actions through UI assertions alone.
- Native plugin jobs provide immediate durable work, not scheduled autonomous authority. Backup scheduling must gain a real installed-system authority before activation.
- Use the maintained S3 transport instead of implementing SigV4/XML/multipart protocol by hand. Keep its raw client private.
- Review correction: shared storage is not a publication routing policy. Claims must also match the worker's configured project and exact registry repository before lease assignment.
- Backup inventory: `infra/postgres/versions.env` already pins a candidate Barman 0.15.1 image, with an explicit prior provenance failure. The upstream 0.15.1 release and current official CNPG/Barman documentation were rechecked; the existing signature qualification must still be completed or replaced by a reviewed source-build chain. Do not reinterpret the existing digest as verified provenance.
- Backup integration also needs explicit `ObjectStore`/`Backup`/scheduled authority admission and agent observation. Those CRDs are absent from `WORKLOAD_KINDS`; current allocation accounts for CNPG instances but not the additional Barman sidecar. Do not bypass these controls by accepting arbitrary manifests or calling kubectl from the plugin.

## Task 1: Private verified S3 transfers

**Files:** Create `crates/object-storage/{Cargo.toml,src/lib.rs,tests/transfers.rs}` and operations documentation. Extend the existing local MinIO test path without starting unrelated services.

**Interfaces:** `S3Config` explicitly binds an ID, endpoint, region, bucket and namespace; `Credentials` is redacted. `ObjectReceipt` binds backend, random immutable object key, SHA-256 and actual bytes. `Store::upload`/`download` stream through a caller-owned file under a byte ceiling. Product code validates scope; only exact receipts are readable. No ambient credential discovery, public ACL, arbitrary signed URL or automatic deletion.

- [x] Write tests for invalid endpoint/path/credential configurations, exact receipt round-trip, size/hash errors and backend mismatch; observe failure.
- [x] Implement explicit HTTPS configuration (HTTP only behind an explicit loopback test constructor), bounded multipart transfer, readback verification, and safe errors.
- [x] Exercise real MinIO multipart transfer, fresh client, wrong credentials, interrupted input, corruption and owned cleanup.
- [x] Verify formatting, crate tests and Clippy; record commands and actual limitations.

## Task 2: Durable OCI build artifacts

**Files:** `crates/build/src/{artifact,worker,http,service}.rs`, new remote artifact module/tests; additive `crates/persistence/migrations/0046_*`.

**Interfaces:** Keep the local artifact key and OCI validation. Add an optional typed remote receipt; old local records remain unchanged. Explicit stable backend bindings determine which remote releases a worker can claim. Completion receipt hashes include remote storage only when present, preserving old local retries.

- [x] Add failing tests for remote receipt scope, recovery by a fresh worker-local store, hash/size corruption, wrong backend and legacy preservation.
- [x] Upload trusted worker output after OCI validation and before completion outbox; recheck build authority around transfer.
- [x] Rehydrate verified remote bytes into private local staging before existing registry publication. Check release authority throughout download and publishing.
- [x] Add durable backend metadata, exact wire parity and opt-in matching backend claims; retain all source/approval/enrollment checks.
- [x] Prove worker A upload → durable completion → worker B download/publication, lost responses, revocation and rollback retention with owned local fixtures.

- [ ] Extend the existing OCI milestone to retained frontend outputs, required source snapshots and separately bound signature/SBOM/provenance bytes; qualify artifact inventory/orphan cleanup without deleting referenced releases.

## Task 3: Dedicated PostgreSQL recovery

**Files:** `plugins/postgres`, `crates/plugin-kit`, existing agent workload boundary, scoped backup manifests/acceptance under `infra/postgres`.

Inventory correction: the live managed native product currently uses `crates/databases`, `DatabaseJobSpec` and `NativeDatabaseExecutor`; `plugins/postgres` is a different generic-workload path. Extend the actual native product's authority and agent executor, then preserve those semantics in product-ownership work. Do not route long-running backups through the 45-second immediate plugin-job runner. A persisted Core backup-policy grant can authorize CNPG `ScheduledBackup`; recurrence, WAL archiving and retention deletion need distinct observed authority/revocation states. All native CNPG clusters are named `postgres`, so persist an installation/source-UID-bound unique archive path and explicit plugin `serverName` instead of inheriting a shared default. Reserve Barman restartable sidecars and fresh-target restore peaks before creating workloads.

- [ ] Select and pin current official CNPG Barman Cloud plugin, controller and sidecar provenance; inspect installed CNPG compatibility.
- [ ] Add explicit backup destination and separately scoped backup/read-only restore credentials in protected plugin metadata.
- [ ] Extend existing resource actions/jobs for backup configuration, on-demand base backup, WAL status and restore into a new resource. Keep Shared recovery unavailable.
- [ ] Implement persistent scheduled authority with explicit operator activation; revocation stops future work.
- [ ] Restore a base backup plus WAL to a selected point into a fresh target; verify roles, data and connectivity, observed recovery window and measured RPO/RTO.
- [ ] Add retention, failure notifications and truthful Console/REST/MCP actions only after that evidence.

## Task 4: Managed Object Storage product

**Files:** New independently installed `plugins/object-storage`, existing catalog/resource/offering/limit/job and Console extension contracts.

- [ ] Use separate customer/project/environment resources with private backend assignment, resource ownership and a real provider isolation policy.
- [ ] Implement restricted access, rotation/revocation, retained deactivation and separately authorized deletion using current membership checks.
- [ ] Provide direct S3 app credentials only after provider policy isolation is proved. Browser grants have bounded duration and clearly stated bearer authority; membership-sensitive download uses an authenticated data path.
- [ ] Inventory actual bytes/versions/multipart reservations; label direct-S3 limits soft unless provider enforcement is independently demonstrated.
- [ ] Add offers/usage, Console, REST/MCP parity and real two-tenant denial, revocation, interrupted-upload and concurrent-write acceptance.
- [ ] Probe Wasabi-specific IAM/account capabilities in explicitly owned bounded resources only; do not infer a resellable account model from the trial.

## Task 5: Files, exports and archives

**Files:** Existing CMS/app file integrations, existing plugin jobs, build log handling and product-specific private metadata.

- [ ] Bind new CMS/app uploads to explicit project/environment stores; preserve current URLs and old stores until a separately authorized checked migration.
- [ ] Generate DB/project/report exports through existing jobs; stream outside JSON, check current membership on downloads, expire exact objects explicitly.
- [ ] Archive completed build logs and sanitized diagnostics with purpose-specific retention. Test secret/URL/token/body redaction and foreign-tenant denial.
- [ ] Verify narrow/wide Console flows and matching REST/MCP behavior.

## Task 6: Mail/app recovery and registry

- [ ] Define a consistent recovery set for Core ledger, encrypted Mail metadata/payloads and CE state; retain independent encryption keys and durable identities.
- [ ] Restore into fresh private targets; uncertain historic sends stay closed for reconciliation. Test confirmed sends are not replayed.
- [ ] Back up application data with product-consistent snapshot/quiesce behavior and verify application function after fresh restore.
- [ ] Add an authenticated Distribution registry with isolated S3 credentials; preserve digest, publication and observed deployment checks. Prove actual push/pull after restart and reference-aware retention; no production migration.

## Task 7: Jenkins evaluation (research only)

- [x] Compare the native isolated buildworker and external CI handoff against Jenkins maintenance, security advisories, plugin inventory, ephemeral runners and tenant separation.
- [x] Record who owns commit/provenance/signature/approval/deployment authority and how large approved artifacts cross the boundary.
- [x] Record a reasoned decision with dated official sources; install nothing and add no build cache.

## Verification record

Initial baseline: `cargo test --locked -p webdock-build --no-default-features --test artifact --test contracts` passed (7 tests). Remote Wasabi compatibility, customer product isolation, backups and production readiness are not established by this baseline.

Milestone acceptance (2026-10-25): six offline adapter tests, two artifact recovery tests, the existing seven artifact/contract and two execution tests, live MinIO multipart/fault/isolation tests, and fresh native DB/KVM local plus cross-worker OCI publication/rollback passed. Independent review found publication routing, composed key preflight and fresh-checkout diagnostic setup defects; each was reproduced and fixed with regression coverage. Full quality gate passed: 52 checks, including existing Console browser fixtures. Dated evidence: `docs/verification/2026-10-25-s3-oci-artifacts.json`. This is the private transport/OCI milestone, not completion of all owner scope.


## Local qualification checkpoint — continuing implementation

The original full scope remains authorized. Completed local slices are not a
production/Wasabi activation or a claim that all application recovery is finished.

- Shared verified S3 adapter, durable OCI and normalized-source/evidence bundles,
  format-safe cross-worker publishing, authenticated S3 registry and durable count
  reservations have owned local acceptance and review responses.
- Managed Object Storage now has normal signed native installation, retained
  generations, restricted keys, current-authority binary gateway, actual browser
  Create retry/upload/download and genuine member read-only behavior. The known
  provider FullObject-hash omission is fixed; actual MinIO unknown-checksum
  semantics are labelled separately.
- Dedicated PostgreSQL has real connected backup/PITR and historical credential
  recovery. Review fixes cover invalid queue entries, observed stop before
  project/customer archival, resize quota/revisions and100+ history pagination.
- CMS opt-in S3 preserves old stores and URLs. Its four review defects are fixed
  and fresh local/S3 immutable-image acceptance passed. Protected exports now have
  actual managed readonly PG sources, committed source archives, observed reports,
  bounded diagnostic snapshots, source/job authority and signed native Console
  acceptance; its three review defects are fixed.
- Remaining required implementation is tracked in the frontend-output plan and
  native Mail/application recovery plan. Mail0052 is still under implementation,
  not frozen. The PostgreSQL Console must also consume the new backup-status
  contract without enabling production recovery. Completed build-log archives
  also remain open: existing typed
  diagnostics deliberately exclude arbitrary logs. Shared source/proto/native
  qualification is coordinated by phase.
- The read-only operator object inventory has 16 combined storage-tool regressions;
  it never deletes or supplies cleanup authority. Referenced/unknown/active objects
  remain protected. No automatic GC/TTL or old-store migration is enabled.

Evidence is under dated docs/verification records. The full root quality gate must
be run after shared integration stabilizes. Genuine production inputs remain EU
Wasabi account/destination/IAM qualification, approved delivery/signing/key custody,
and measured commercial retention/recovery commitments. The owner permits local
MinIO implementation while those account inputs remain open.
