# Mail and SQLite application recovery implementation plan

**Goal:** Build a bounded, operator-only local recovery milestone, leaving production activation disabled.

**Architecture:** Core remains the only send and operation authority. Mail adds a private durable write/dispatch gate; a matched recovery set must include Core, all Mail metadata and encrypted payloads, offline incoming CE state when present, and separately encrypted recovery configuration. The concrete application profile is ntfy with offline SQLite and local attachments, no external mutable dependencies, a fresh target volume identity and no-egress startup.

**Tech stack:** Existing Mail Rust workspace, PostgreSQL logical snapshots, existing object-storage multipart/readback transport, Python SQLite and age operator encryption.

**Spec:** `docs/handovers/2026-10-25-s3-managed-storage.md` and the delegated task brief.

## Global constraints

- No Core migrations/wiring/root Cargo changes during frozen 0046–0051 acceptance. No commits, delegation, production access, existing cluster changes or external messages.
- Protected handovers and `.dev-local/demo-owner.json` are never read.
- Local fixtures get unique custody, private diagnostics, finite deadlines and exact cleanup. Existing MinIO is reused.
- Snapshot only after writes/dispatch and all volume writers drain; fail closed on crash, revocation, uncertainty or stale completion.
- Restored dispatch and applications start quarantined, without restored API/browser sessions. A historical snapshot cannot prove later delivery outcomes.

## Inventory and integration contracts

- Core: `webdock_mail.send_job`, `provider_lease`, native incoming/configuration operations and restrictions; current authority is implemented in native Mail services and plugin-host. Never replace these with a backup send ledger.
- Private Mail: `plugins/mail/src/store.rs`, physical migrations 1–4: domains, relays, sends, legacy engines, project_customers, customer_engines, mailboxes, private_payload_key, private_payloads. Preserve UUIDs, encrypted bytea/text and migration journal.
- Keys: runtime `secretKeyHex` seals resource UUID credentials through plugin-kit AES-GCM. `payload::payload_key` derives the existing namespaced MailCipher key, verified by private_payload_key. Encrypt recovery configuration separately with an operator age recipient; no master-key customer download.
- Existing customer advisory fence `hashtextextended(customer,704611)` ends before SMTP I/O. A new private gate must first close admission durably, then drain shared write/effect guards, including SMTP completion. This gate is a safety interlock, never a grant of policy authority.
- Core integration required next phase: audited durable standing policy, current customer/project/grant/resource checks independent of activating browser lifetime; generation/lease/revocation callback fences; Core send admission/claim/attempt block and drain; consistent Core snapshot; target session/API-key invalidation and dispatch quarantine; existing hosting_operation job links and capacity reservation. Parent was notified before shared changes.
- CE is outgoing-only absent. Incoming requires one existing CE per incoming-enabled customer, all native transitions fenced, controller reconciles disabled, ingress and process stopped with observed offline storage before snapshot. No live mailbox tar.
- Native application volume: `crates/agent/src/storage.rs` VolumeRecord appID/projectID/clusterID/bytes/uuid and ready state are authoritative. Target allocation must retain its own fresh filesystem UUID and owner record. Copy only selected data, never pool metadata, volume.ext4 or service-kind.json. Shared native storage wiring awaits parent coordination.

## Tasks

1. Test first and implement private Mail recovery gate in `plugins/mail/src/recovery.rs`, migration in `plugins/mail/src/recovery.sql`, journal append in store. Integrate guards in Handler, incoming background reconciliation and ManagedTransport; gate private table writes/cleanup. Test draining, stale generations, persistent closure and quarantine. No exposed backup API or fabricated policy approvals.
2. Prepare active native Rust contract in `crates/agent/src/storage/recovery.rs`, tested against real native types without linking shared modules. Implement disposable ntfy profile demonstration in `agents/kubernetes/ntfy_recovery.py` and tests; the older Python backend is not final product integration. Require exclusive stop/fence proof, retained source/fresh target identities, bounded reservation, exact manifest and file hashes, schema/config/image, independent age environment/key envelope, strict extraction and closed startup. Reject generic mutable dependencies and online DB copies.
3. Add a concrete owned fixture and existing S3 adapter CLI under Mail's standalone workspace, using separate upload/restore credentials, verified receipts and private staging. Demonstrate source-unavailable recovery for private Mail and the supported SQLite profile. Report private/SQLite evidence separately from blocked matched Core/CE acceptance.
4. Run focused suites, negative tests and exact fixture cleanup. Record observed RTO/RPO for only the workload actually run. Report shared integration and production custody/provider/signer inputs without implying shipped arbitrary-app or full Mail recovery.

## Review focus

- Freeze/guard races must not acknowledge a drained fence while a writer or transport is active.
- A restore cannot reactivate historical authority, even if source was active when snapshotted.
- Missing/wrong recovery key and incomplete/corrupt object sets fail before target activation.
- Revocation or changed intent makes old source-resume/callback tokens unusable.
- Fresh target volume ownership/UUID and capacity remain distinct from source identities.

## Execution ledger

- Inventory complete. RTK.md referenced by supplied instructions is absent from this checkout; no substitute was assumed.
- Parent notified of exact source, guard, key and operation contracts. Shared changes remain frozen. Execute locally without another approval question under the explicit task instruction.
- Private Mail safety interlock includes durable opaque external-effect custody: explicit completion clears it; cancellation leaves the drain closed for real reconciliation. No message or attempt state is duplicated.
- Concrete ntfy + private Mail S3 source-unavailable proof passed. Incoming CE, canonical Core quarantine/standing policy, native pool/executor hooks and real SMTP historical replay proof remain coordinated integration gates. Exact native hook contract is in `docs/operations/mail-application-recovery.md`.
