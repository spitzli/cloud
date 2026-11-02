# Native frontend output retention qualification

Completed locally on 2026-10-25 with actual Rust1.94.0 execution and the frozen
forward0053 schema. The independent E2 review's Hosting recipe P1 was fixed with
live PostgreSQL query RED/GREEN tests; its other reviewed paths had no confirmed
defects. Repository-pinned Clippy1.98.1 passed without weakening lint settings or
editing frozen Mail production code.

- Frontend native Core/KVM/S3 acceptance: **269.90s**. Normal frontend configure,
  authenticated build/release claims, old-worker/E1-only-publisher exclusion,
  real VM HTML/JS, signed E2 persistence, producer-local-store loss, compatible
  hydration/publication, exact retained reuse, approval/revocation fences and
  rollback passed. Source, output and inventory overflows each failed terminally
  through the outbox, followed by successful queued builds.
- Ordinary Dockerfile/local legacy Core publication and rollback: **65.59s**.
  Old completion hashes and one-image archive behavior remained compatible.
- E1 signed-source cross-worker S3 publication/rollback and preparation-failure
  queue progression: **168.03s**.
- Fresh compiled schema/security smoke: **52 native journal entries through53**,
  exact compiled checksums, preserved authority body except recipe allowlist,
  owner/ACL/search-path checks and unsigned-worker denial passed.
- Rust1.94.0 build package: **35 all-feature**, **25 pure**, **29 worker-only**
  tests passed. Explicit VM/provider fixtures remain ignored in ordinary suites
  and were run separately as recorded above. Strict Clippy1.98.1 all-target checks
  passed for all-feature, worker-only and pure modes. Formatting, Python runner
  tests and scoped diff checks passed.

Final migration0053 SHA256:
`3ef35e7f191b15c7b9aa3d007a59247e587fcc2ecb081c72a84441c9f13a458c`.
Mail0052 remains
`c472a56bf4e5f90bc023778985f3191b56626e6b149c70d5c2439059816b0c20`;
the root lock remains
`c6f2848cdeeeb1f63ebf38bf9c8faba12cbb9247b336c1b999427e643901a576`.
Every pre53 migration and the baseline were preserved. The final53/compiler freeze
was announced after schema smoke and before full VM qualification; Root protected
its independent52 snapshot and released Archives'53 bootstrap.

All exact commands, migration/source/binary hashes, private diagnostic locations,
fixture custody records and observations are in
[the machine-readable evidence](2026-10-25-frontend-output-retention.json).
The schema-only, frontend-only, remote-only and legacy-only modes use the existing
owned build fixture runner; S3 modes run inside `tools/tests/object-storage.py`.

Each fresh Postgres container was removed after matching ID/ownership label and
checking absence; private password files were removed. The temporary registries
and each fixture's fresh MinIO buckets/user/policy were removed; no incomplete
multipart uploads remained. The existing MinIO service and Kind/k3s were not
replaced or modified. Baseline VM image/kernel/initrd hashes still match.

The uniquely owned VM remains at
`.runtime-test/build-vm-frontend-20261025-r2`. Its image, kernel and initrd pins are:

- Image: `489498f03c185625f83c19e52606afa43405f3db317203f3bb29c4de290b9262`
- Kernel: `26f146f664ad057be3b39b211422b03b1f94cb2c3d66522fd4841811a224bfd5`
- Initrd: `6db76fbf58bd0b4a8f300c6d4b1fbda08a64db26dc09f8e280924b640781e19c`

Its private `retained-inventory.json` lists retained files, exact sizes and hashes.
Owned compiler outputs and private diagnostics are explicitly retained for
reproducibility. Only three Hosting test initializers needed the additive
`recovery_runtime: None` compatibility field. Archives' `export.rs` and
`export_logs.rs`, Mail/agent production code, protobuf, Console and root dependency
declarations/lock were not edited in this lane. No commits or production actions.

This qualifies durable outputs and the native Core publication/rollback flow.
The Hosting fixture supplies runtime readiness proof; this is not a real
Kubernetes/static-site/CDN deployment. Wasabi/provider compatibility, production
region and signing custody remain separate. No customer download route, cache,
old-store migration/deletion, TTL or GC was added.
