# Retained frontend outputs

Remaining required build scope: retain completed frontend files alongside the OCI
image, normalized source and release evidence. This is not a build cache and does
not activate a CDN, Vercel deployment or an existing-store migration.

## Proposed concrete path

- Add an explicit `dockerfile-frontend` recipe. Ordinary Dockerfile builds remain
  unchanged. Its bounded repository configuration selects a relative directory
  from the final image filesystem; no host path or arbitrary exporter is accepted.
- Use the pinned BuildKit0.21.1 multiple OCI/local exporters in a new uniquely owned
  disposable VM image. Only this explicit recipe materializes the local rootfs.
  Package selected regular files into frontend.tar; reject traversal, links,
  special files, excessive entries and size. Keep image.tar unchanged.
- Bind frontend bytes/hash and source configuration into a versioned evidence
  envelope. Give it a distinct storage-key format and negotiate reader capability
  before publication leases, as for current .e1.tar. Never strand an old publisher.
- Source recipe admission and worker recipe capabilities must prevent old guest
  images from taking frontend jobs. Existing local one-file receipts stay valid.
- An additive source-recipe SQL constraint update may need0053, after Mail0052 is
  coordinated and frozen. Do not write it during current schema51 qualifications.
- Preserve the existing immutable Core artifact ledger and common S3 receipt.
  No second release identity, TTL or GC. All companion bytes share the finite
  artifact budget; deterministic preparation errors complete failed, not leased.
- Qualify real VM build, verified S3 retention, a different publisher, exact
  frontend files, stale/unsupported workers, corruption, authority revocation,
  restart and rollback. Use fresh owned image/fixture paths; never overwrite the
  retained .runtime-test/build-vm baseline or touch existing Kind/k3s.

The current source-file SPDX inventory is an explicitly scoped retained SBOM.
Resolved OS/dependency analysis is an additional capability, not a claim made by
that inventory. Preserve supplied release evidence with accurate provenance and
avoid inventing package/license conclusions.
