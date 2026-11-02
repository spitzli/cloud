## Review54r1 implementation in progress (not r2 ready)

Main resumed for11 owned findings; private finding10 completed independently (16 actual TLS-PG/CE cases;27 standalone tests, no migration/Cargo changes). Private source hashes/evidence: plugins/mail/tests/fixtures/review54-10-verification.json. New signed Mail image remains required before full capture.

Implemented source changes with focused RED/GREEN: durable provider submission journal/confirmed discharge (drop/cancel/restart stays uncertain; accepted remote PATCH can apply after client cancellation and still denies drain); full target+nonce+physical pool binding; pending checkpoint terminal deferral; explicit restore-only credential loading; distinct strict restore staging origin; live restore per-effect checks/cancellation. Observed SourceSQL3placeholder/four-values denial test did NOT reproduce a protocol failure; now4th generation binding explicit, admitted positive/stale-generation proof still required.

Core54 now WIP adds exact committed callback receipts/monotonic replay, fresh backup epoch, real target-row locking/CAS1, full backup/restore retained peaks and target-volume IDs/role budgets, native capacity accounting. Target roles use actual original customer only for incoming CE; global PG roles have no invented customer/project. Helper new recovery_pool.rs uses root-installed independent public trust, standard OpenSSH reservation namespace/principal, actual pool dev/inode/config identity, root-private quota custody and quota-backed staging/PG/CE filesystems; partial allocations stay charged. All this NEW integration still requires real SQL/physical/race proof.

IPC mode explicit --recovery-trust: new Root0:GID65532 parent0710/socket0660, actual kernel peer65 limited to signed recovery actions and bounded readonly info. Legacy Root-only paths/modes/limits unchanged. Actual UID65 positives/other UID/group-only/spoofed server/frame/legacy denial tests pending. Actual cached PG18 image is UID/GID70 (initial999 assumption rejected); PGDATA=/var/lib/postgresql/18/docker, declared mount=/var/lib/postgresql. Identity smoke exact cleanup in recovery-image-identities evidence. PG init/restore/WAL/temp quota tests pending.

Builtin Hosting setup now legitimate: POST /api/console/plugins/builtins/hosting/activate with real MFA/PKCE Console cookie+Origin and idempotencyKey; hosting feature enabled/service Some. Do NOT seed module/identity/session rows. Root owns oauth/service.rs central current native-token locking (latest SHA34f31794fda22ecbc39df12f8d175691ca6920f733aa73f275ffca37bd26d541; safe component GREEN27.04s). Do not edit that file. Earlier provider-filtered proxy approach is not test success; Root replaced it with ordinary real DB transaction proof.

Current gaps BEFORE r2 ready/freeze: finish wiring/validating reservations through all native paths; root app-source/ref guards and actual fresh native owner/UUID; runtime CPU/RAM/tmp bounds; signed helper real UID65/role/PGDATA/WAL/temp tests; genuine admitted Core policy/fence/CAS0 legacy before-submit versus accepted-remote-unknown proof; current role/revocation/quota races/phase replay/lost callback; missing/wrong key/hash/owner/source unavailable; full current checks and exact updated inventory/all12 responses. Full matched Core/private/CE/SMTP + native ntfy recovery remains NOT run, no RTO/RPO/full acceptance claimed. No current final54 freeze or protected Core54/Agent-v2 pin.46–53/52 historical snapshot/Root lock unchanged.

# Native Mail and application recovery implementation plan

Owner authorization covers the complete handover. No delegation, commits, production/Contabo/Spitzli changes, existing cluster/service changes, old-store migration or external messages. Never read the protected handovers or demo owner file.

## Coordination ledger

- FINAL0052 freeze: `c472a56bf4e5f90bc023778985f3191b56626e6b149c70d5c2439059816b0c20`. Never rewrite it; later schema corrections require a coordinated new migration. Fresh schema bootstrap/denial/cleanup green, shared compiler and Clippy green, fullfmt green. Protected14-ELF/migrations/source snapshot `/home/newt/.cache/webdock-native-recovery-52-b8c696efe326ea8b`; pin.json SHA256 `30cac5cab2df52a4bb5b698652e36d580a5c8a325c515443bcd2d8f6e9b8580e`; source archive SHA256 `0c004d3ffb710a5fdebcb213500fe93fca60243f6bc274646fd80bf824e13438`. Root explicitly notified to release0053/other qualifications against their own pins. This52 snapshot is historical and immutable. Full source-capture acceptance requires a separately named Core54+Agent-v2 pin; Core52 is not full capture proof. Public pin record `docs/verification/2026-10-25-native-recovery-52-pin.json`.
-60focused agent tests passed (34lib,3application contract,3config,13management,2outbox,2capsule,3storage);3privileged integration cases ignored. These are not full recovery qualification. Actual Native/Core/CE/SMTP/ntfy fresh-target proof remains required.

- Root format regression passed165.02s. Browser0.5 and Archives0.2 review-fix qualifications completed and cleaned. Root explicitly released shared compiler/schema changes.
- Reserved migration: `0052_native_recovery.sql`. Preserve0046–51.0053 is reserved for Root later and must not appear during frozen qualification.
- First fresh owned0052 bootstrap passed with SHA256 `fdc2587b03574dee0a79dce8d8d17569992d8f548b7c257acdc515eacad05edd`, journal51 entries/max52; unfenced cold-target quarantine denied; exact PostgreSQL/volume/private-dir cleanup completed. This was a smoke, **not the final schema/compiler freeze**. More SQL may be corrected before a new fresh bootstrap and explicit final freeze.
- Initial `cargo check --locked -p webdock-control-plane -p webdock-agent-control -p webdock-agent` passed; DTO test passed. No end-to-end native recovery qualification is claimed yet.
- Earlier private-Mail/ntfy Python evidence is historical and remains storage-only. New full proof goes in a separate evidence record.

## Draft54 review handoff

Root released0054 implementation after Archives independently pinned<=53. Draft54 currently hashes to `71fec2ff01c25683b89b5893205bf76dc6548b1d5e4e4542efdf9029c1f1c988`; it is not frozen. Source-review inventory, every exact source hash, commands/results, causal failure evidence, custody and outstanding proof are in `docs/verification/2026-10-25-native-recovery-54-review.json`. No implementation edits during the independent review; Root resumes this implementation with findings.

Fresh54 schema/security smoke: journal53/max54,16 actual runtime-role unowned calls/custody writes denied, unfenced cold quarantine denied, exact owned PostgreSQL/volume/private-directory cleanup.65 focused Agent tests pass,3 privileged cases ignored. Core/Agent locked check, expanded Clippy including tests, full workspace formatting and offline locked metadata pass. A new default-stack overflow was reproduced, corrected by boxing the recovery dispatch future, and retested without a larger stack.

Core54 owns immutable source intent copies/revisions/hashes and participant custody. Native authenticated RecoveryControl carries source admission/drain through the existing checkpoint envelope, with current policy/operation/installation/epoch/agent/lease authority. The Agent observes exact Deployment UID/resourceVersion/template/owner, conditionally stops without forced replica ownership, excludes all PVC consumers, uses a bounded read-only offline helper, preserves `/data` versus `/var/lib/stalwart`, and compensates only under same-fence checks. Extra enrolled executors fail closed pending explicit owned legacy worker handling.

The controlled old-intent provider test records guarded0 versus legacy1 PATCH. It does **not** replace the required actual admitted-Core/CAS0/pre-gate legacy-worker UNDRAINED regression. Real matched Mail/CE/SMTP/native quota-backed ntfy/source-unavailable acceptance and independent review remain required. No full-workload RTO/RPO claim, no production activation, and no final54 migration/compiler/binary freeze.

## Implemented/wired so far

- `crates/persistence/migrations/0052_native_recovery.sql`: real operator standing policies, existing hosting_operation-linked jobs/leases/sets, installation-wide Mail recovery epoch, DB mutation fence over all canonical Mail tables, frozen-source handoffs become unknown, guarded same-epoch source compensation, cold-target quarantine invalidates sessions/API/OAuth/agent/plugin enrollment and denies all old system/actor RLS authority. No second send ledger.
- `crates/hosting/src/recovery.rs`: revalidated actual browser/operator admission, live operator/resource checks for recurring work independent of session lifetime, native operation claims/checkpoints/completions, status/audit and operator-only admitted-set export. `recovery_signing.rs` signs only actually admitted sets with an independent operator-installed OpenSSH key; not a browser field.
- `crates/agent-protocol/src/recovery.rs` and `recovery/custody.rs`; additive `proto/webdock/agent/v1/recovery.proto` and build.rs registration. Existing agent.proto unchanged.
- `crates/agent-control/src/recovery.rs`, public listener registration, Core hosting recovery command route and durable target HTTP quarantine middleware.
- `crates/mail/src/recovery.rs` guards native worker/customer/incoming paths; SQL fence also protects pre-gate mutations/callbacks.
- `crates/agent/src/storage/recovery_io.rs` linked into actual quota helper, with snapshot/install/record/staging RPC. SQLite backup CLI only offline; preserved source identity and fresh target ext4 VolumeRecord, UID65532 data-only installation.
- Prepared but not fully linked/executed: `crates/agent/src/recovery/capsule.rs` standard OpenSSH signed admitted set + actual operator capsule and durable local replay journal; `gateway.rs` network-none read-only TLS JMAP/ntfy gateway.

## Required next work

1. Finish real agent runtime/config/management/outbox wiring and concrete Docker/Kubernetes source stop/drain/native target execution. Extend heartbeat strict capability schema for nativeRecovery. Keep PG/export/plugin RPC semantics unchanged.
2. Correct/source-pin policy live checks and complete Core execution plans with trusted source/target profile mapping. Admit only operator-installed key/destination profiles; no arbitrary recipients or keys. Add immutable cold signed-set issuance after actual Core commit, and operator capsule CLI with target/nonce/capacity/deadline.
3. Native execution: consistent Core full logical dump + private Mail PG dump/encrypted payloads + stopped actual CE state, fixed source/config/image/key receipts. Stop all controllers and CE writers including old processes; source resume only same live fence/intent. Independently encrypted age key/config custody outside source data failure domain. S3 uses existing verified multipart adapter; separate writer/Get-only restore credentials, no TTL deletion of referenced sets.
4. Cold restore must work without reading the lost Core/source stores: an actually previously admitted signed set plus fresh operator-controlled independently trusted capsule authorizes **quarantined data restoration only**. Offline cannot know later source policy revocation or delivery history. No fake principal/session or dispatch/cutover authority. Replay protection is durable and local to the bound target, not globally single-use across disconnected targets.
5. Actual CE recovery mode `STALWART_RECOVERY_MODE=1` disables background services/MTA; no recovery admin backdoor needed when original mailbox credentials work. Add network-none and a read-only JMAP TLS inspection gateway. Pinned0.16.25 upstream source/docs checked. Confirm actual mailbox data/auth/TLS; outgoing-only must instantiate no CE.
6. Native ntfy restore must use actual quota-backed helper/native executor, real Core resource ownership and new target UUID. Only SQLite cache/auth and local attachments; explicit environment/config allowlist, no external mutable dependencies. Preserve logical isolated origin and verify stored attachment URL against fresh target. Gate network-none/read-only TLS inspection.
7. Build new uniquely owned bounded fixture using real bootstrap operator + password change/TOTP, native enrollment and real API-created customer/project/resource data (never old SQL-seeded Mail fixture identities). Only local SMTP sink. Existing MinIO and cached images reused; no existing Kind/k3s/services touched. Disposable nested Engine/storage fixture is available as `webdock-rust-storage-test:owned` (imageID `sha256:60e7c44155bbcb84ec27c1f3d20997c12f6246e1d079a9a0225601b72c4c7bbe`), Ubuntu26.04 with Docker/e2fsprogs/util-linux; exact local descendants/loop devices need custody and cleanup.
8. Before acceptance, finish compiler checks/negative unit tests and fresh0052 bootstrap, then announce exact0052 hash/compiler freeze to Root. Never rewrite0052 after that. Later schema corrections require another coordinated migration.
9. Full proof: real confirmed SMTP stays accepted and never repeats; real lost response becomes unknown; queued/unknown/historical target work never dispatches. Expire/logout the real activating session and show scheduled execution continues; revoke policy and reject stale callbacks. Source Core/private PG/CE/ntfy genuinely unavailable; cold restore from receipts/custody only. Wrong owner/key/hash, partial/crash, capacity denial, no egress, target TLS/auth/content/permissions. Measure full workload RTO/RPO only and exact cleanup.

## Current useful commands

```bash
cargo check --locked -p webdock-control-plane -p webdock-agent-control -p webdock-agent
cargo test --locked -p webdock-agent-protocol --test recovery
python3 tools/tests/native-recovery-schema.py
```

No full native acceptance or production activation has been completed. Production custody/signer/provider decisions remain real external inputs, not a reason to skip local implementation.
