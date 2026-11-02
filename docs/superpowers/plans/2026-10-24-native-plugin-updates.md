# Native plugin update and rollback implementation

Base: `da7d1f8`. User approved inline execution of the concrete v1→v2→v1 PostgreSQL milestone. Binding design: [native plugin updates](../specs/2026-10-24-native-plugin-updates-design.md).

## Backend and persistence

- [x] Reserve migration0043; preserve0042 bytes and existing managed execution records.
- [x] Retain per-generation execution/material/history, monotonic logical schema high-water and current/candidate separation.
- [x] Add strict operator update/rollback/retry/history APIs with revision/idempotency/catalog/placement/configuration/capability checks.
- [x] Reuse completed disable/drain; serialize lifecycle versus availability; guard current-reader enable against the watermark.
- [x] Implement phased source/failed-candidate suspension, candidate Apply, strict callback replay/fencing and atomic promotion.
- [x] Prove restricted role, pool1 ordering, incompatible-reader/unknown-start/high-water, retention and stale callback behavior.

## Agent and protocol

- [x] Add specv2 generation namespaces while preserving v1 fingerprints/resources.
- [x] Negotiate supported spec versions; retain old-Core/agent compatibility and update strict hosting capability contract.
- [x] Prove quiescence with complete custody, conditional replicas0 write, observed generation and empty Pod list.
- [x] Test stale patch fencing, terminating Pods, authority loss and downgrade/reconnect behavior.

## PostgreSQL fixtures and real acceptance

- [x] Retain the existing logical1/physical3 distinction; do not expand production migration APIs without a demonstrated need.
- [x] Prepare two real signed compatible OCI releases and fresh owned native CLI bootstrap/enrollment/storage/callback fixtures.
- [x] Through Console, install v1 and create a real customer resource; write SQL and privately retain credentials.
- [x] Update to v2, then roll back release v1 into generation3; prove old Pods stopped, new image/generation, metadata/password/data retained.
- [x] Run distinct schema-policy negatives without claiming a physical schema2 migration.

## Root integration

- [x] Freeze public/worker/agent contracts and ownership before parallel source changes.
- [x] Extend Core worker retained-seed/TLS preparation, dispatch and managed connection lookup.
- [x] Add lifecycle/history/review/disable/operation/retry Console UX and real HTTP/browser boundary coverage.
- [x] Resolve independent review, run full quality, document evidence, clean only owned fixtures, sign and push the existing branch.

No production/provider/real-mail actions, operator reset, private output, prohibited owner-file reads or handover mutations. Preserve the user's Tilt/dev services and MinIO. Root alone owns repository formatting and integration.

## Verification

Actual signed compatible PostgreSQL 0.1.0 → 0.2.0 → 0.1.0 ran through Console on fresh native CLI bootstrap, using runtime generations 1/2/3. Prior Pods were absent before successor activation; customer SQL, metadata and passwords survived. Screenshots at 375/1280px inspected. Full quality and fresh restricted lifecycle regressions passed. Independent review found old-H compatibility and operation polling recovery gaps; one coordinated fix wave and scoped re-review closed both. See `docs/verification/2026-10-24-native-plugin-updates.json` for evidence and explicit logical-schema/code-only boundaries.

Repository integration and owned fixture cleanup are recorded in the final handoff; this does not claim production rollout.
