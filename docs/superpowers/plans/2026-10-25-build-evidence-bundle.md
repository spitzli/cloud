# Retained build source and evidence bundle

Scope: extend the existing remote OCI artifact milestone with the exact normalized
source snapshot, bounded build provenance, a clearly scoped source-file SPDX
inventory and optional detached Ed25519 evidence. This is one sequential part of
the full S3 handover; separately produced frontend outputs and a complete resolved
image dependency SBOM remain subsequent work.

Use the existing immutable outer artifact tar and Core digest/size/receipt ledger.
No new native database migration, user, queue or competing release identity.
Legacy one-file local artifacts retain their exact format and completion hashes.
Remote worker builds add a versioned, strictly allowlisted bundle after the guest
returns; guest-supplied evidence cannot choose the retained source or trusted pins.
The OCI image itself and its image digest remain unchanged for publication.

Files: crates/build/src/{artifact,evidence,execution,worker}.rs and Cargo.toml,
focused build tests/owned existing MinIO+KVM fixture, operations/evidence docs.
No changes to storage plugin manifest/protocol, native installation fixture,
backup0048 or existing migration history.

1. Add RED tests for source/provenance binding, unknown outer files, tampering,
   cross-build replay, signer mismatch and unchanged legacy archive behavior.
2. Encode exact source bytes plus provenance containing customer/project/build,
   generation, immutable source commit, image digest, VM/boot hashes, recipe and
   root directory. Never serialize build environment values, worker credentials,
   signing keys or local private filesystem paths.
3. Produce a source-file SPDX inventory, explicitly not a claim that all operating
   system and dependency packages have been resolved. Bind every companion's
   actual bytes/hash into provenance. Optional native-worker key signs a domain-
   separated provenance digest outside the VM; offline verification takes an
   expected public key and never treats an embedded key as organization trust.
4. Validate new bundles on local put/read and remote hydration, including exact
   customer/build/generation scope. Keep old one-file artifacts readable. Activate
   bundling for new remote builds only; keep legacy local output byte-for-byte.
5. Test pure validation/tampering and actual remote build/publication/restart using
   the owned existing fixture. Keep original OCI publication checks, authority
   polling, completion replay and rollback coverage. No TTL/GC/delete activation.

Source and evidence bytes share the existing 512MiB artifact limit and S3 receipt.
A completed unacknowledged object can still be an orphan; never free/delete it by
age. Reference-aware inventory and explicit orphan reconciliation remain open.
