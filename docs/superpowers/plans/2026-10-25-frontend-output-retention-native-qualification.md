# Frontend native qualification after Mail0052 freeze

Completed status: independent E2 review completed with one confirmed P1 in
Hosting recipe SQL. Both environment/publication query gates now admit the
explicit frontend recipe, with live SQL RED/GREEN regressions. The frontend-only
fixture runner is qualified. Root explicitly released final Mail0052 and its
protected snapshot. The reviewed draft was installed as forward0053; fresh
schema/security smoke, frontend qualification and Docker/legacy/E1 preservation
passed. See [final evidence](../../verification/2026-10-25-frontend-output-retention.md)
for exact commands, final pins, native observations and cleanup. The outside
draft remains a historical review artifact.

## Coordinated changes

1. Completed: address the confirmed E2 Hosting-recipe P1 with focused live SQL
   regressions. The minimal query fixture did not bootstrap Core or native schema;
   full frontend authority and publication remain unqualified until the run below.
2. Completed: receive Root's explicit Mail0052 freeze/coexistence confirmation. Record the
   immutable baseline and migrations0001–0052 SHA256 values before any addition.
3. Completed: add only `crates/persistence/migrations/0053_frontend_output_retention.sql`,
   using the reviewed outside-migrations draft. The source check retains
   `dockerfile` and `vercel`, adding `dockerfile-frontend`. Replace only the recipe
   allowlist in `git_release_authority`; preserve its remaining body, owner,
   SECURITY DEFINER/search path, and existing execution ACL.
4. No Cargo declaration/lock changes are needed. SQLx automatically embeds0053;
   compile the normal `webdock-migrate`, Build/Core service and Hosting test code
   against the finalized journal. Do not use an old migration binary, patch SQL
   rows around readiness, or bootstrap before the coordination gate.
5. Prepared: `tools/tests/build-storage.py` has an explicit `--frontend-only` selector
   for `git_frontend_remote_outputs_cross_worker_publication_and_rollback` and a
   finite 900-second frontend acceptance deadline. Keep old/default selectors and
   their behavior. This build fixture tool remains in this task's domain; Root's
   inventory/tool-quality changes remain separate. Three selector/deadline tests
   passed without starting Docker.

## Resources and invocation

Retain the already prepared immutable VM only at
`.runtime-test/build-vm-frontend-20261025-r2`. Its private `custody.json` and
`SHA256SUMS` record image/kernel/initrd pins. Verify the manifest before the run.
Do not overwrite this image to incorporate a review fix: prepare a new uniquely
owned image path if guest behavior changes. Never overwrite or refresh
`.runtime-test/build-vm`, its boot files or other retained fixtures.

Use the existing `tools/tests/object-storage.py` fixture to create fresh restricted
MinIO buckets/user/policy. It uses the existing local MinIO; do not start another
S3 service or use any old buckets/credentials. Inside it, the build fixture creates
one fresh loopback Postgres container, uses random private role credentials, and
runs the normal compiled migration/bootstrap entrypoint against an empty target.
The existing flow creates its own temporary registry container.

Executed frontend command, after both gates above:

```sh
WEBDOCK_FRONTEND_VM_ROOT="$PWD/.runtime-test/build-vm-frontend-20261025-r2" \
  CARGO_TARGET_DIR="$PWD/.runtime-test/frontend-target-20261025-r1" \
  RUSTUP_TOOLCHAIN=1.94.0 \
  python3 tools/tests/object-storage.py \
  python3 tools/tests/build-storage.py --frontend-only
```

The runner supplies generated database URLs and signing/context secrets privately.
Do not print configuration, credentials, signed URLs, passwords or DSNs. Its
private diagnostics retain the migration/test output with redacted console tails.
Do not start Kind/k3s or touch production/provider resources.

## Required native observations

- The compiled migrator boots a genuinely fresh native schema through0053 and its
  journal matches the compiled release exactly. The old baseline and0001–0052
  checksums remain unchanged. The new source is admitted by the normal configure
  command, not direct row insertion to bypass its recipe constraint.
- Normal authenticated build-claim HTTP defaults to Dockerfile-only. An old
  worker cannot lease a queued frontend build. An explicit compatible worker
  receives the exact recipe, source commit, limits, authority and generation.
  Unknown recipes fail before any lease. Old pinned VM config remains incapable.
- The real disposable VM builds actual frontend files and original OCI bytes.
  The trusted worker constructs version2 source/configuration/output evidence;
  its signature, image and source identities are validated. Guest provenance is
  untrusted. The common512MiB artifact budget includes all companions.
- Verified MinIO persistence precedes canonical Core completion. Lost HTTP
  acknowledgment/restart replays the exact durable completion receipt. A changed
  receipt fails. Removing the producer/local store does not remove the retained
  artifact. A separately enrolled compatible publisher hydrates it from S3.
- Both legacy/default and explicit `[0,1]` publishers leave the E2 release queued;
  filtering happens before leases. Unknown reader formats fail. Compatible
  publication preserves registry/project/tenant scope and exact OCI graph bytes.
- The private library consumer returns exact `index.html`/`assets/main.js` only
  under the expected complete build scope. Corrupted frontend evidence and
  unsupported format keys fail; no arbitrary path/URL download adapter is added.
- Normal approval, account/session/browser revocation, current source/environment
  revisions, canonical receipt, active worker/release leases, registry binding and
  Hosting operation fences remain enforced. Exercise actual publication and the
  retained rollback operation; do not infer Core authority from the standalone
  library/registry tests.
- Deterministic oversized inventory, frontend output and source archives each
  complete FAILED through the existing outbox. A following queued original-commit
  build succeeds after each boundary; no oldest leased job starves the queue.
  Provider/remote uncertainty retains its existing reconciliation behavior.

## Cleanup and final checks

The Rust flow cleans only its exact fixture rows, principals and workers. The
registry Drop guard removes its exact uniquely named container. The build runner
checks Postgres container ID plus ownership label before removal and verifies its
absence; remove its private password file in `finally`. The outer S3 fixture
removes only its recorded buckets/user/policy and confirms no incomplete multipart
uploads remain. Preserve private diagnostic/custody records and report exact
cleanup outcomes, including any failure. Retain the explicitly owned VM for
reproducibility; no implicit old-store deletion or artifact migration.

After review fixes and successful native qualification, run Rust1.94 locked
all-feature and no-default-feature build-package suites, full all-target Clippy,
targeted formatting and diff checks. Clippy uses repository-pinned1.98.1; actual
Rust1.94 execution is verified separately. Pinned strict Clippy passed in all three
feature modes without Mail production edits. Recheck old
legacy/E1 load/restore suffix regressions. Publish dated evidence with commands,
source and migration hashes, timings, resources, observations and limits.

Root's read-only inventory correlates S3 `receipt.objectKey` in its namespace /
customer / build / generation / UUID layout. E2 changes a worker-local archive
suffix, not that S3 layout. No inventory adapter or local archive reader is
currently justified; add a focused fixture/proposal only if qualification shows a
real incompatible receipt difference. This record makes no static-site/CDN,
production-signing, Wasabi/provider or deployment-readiness claim.
