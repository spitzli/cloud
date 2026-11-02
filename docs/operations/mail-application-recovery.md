# Mail and application recovery: coordinated native integration contract

This change supplies a private Mail safety interlock, an unlinked Rust native
storage contract and a disposable ntfy recovery demonstration. It does **not**
activate a production backup service, schedules, customer downloads or arbitrary
application recovery. Core migrations 0046–0051 and existing shared interfaces
remain frozen. The parent must coordinate the following integration before a
native product claim or matched Core/Mail/CE restore acceptance.

## Implemented private Mail boundary

`plugins/mail/src/recovery.sql` is private physical migration 5, appended to the
Mail journal in `store.rs`. `mail_recovery_control` is a single installation-wide
safety gate; it is neither a backup operation ledger nor a competing send queue.
It contains an epoch, state, operation UUID, generation and intent revision.

`Store::fence_recovery(epoch, operation, generation)` first durably changes active
to fenced, then waits at most five seconds for an exclusive advisory drain lock.
Every private table mutation takes a shared transaction lock and checks the
durable gate with a locking read. The locking read also rejects a stale
repeatable-read transaction. A failed/cancelled drain leaves the gate closed.
External-effect guards also persist an opaque token/epoch in
`mail_recovery_effect`. Explicit completion removes it; cancellation, process exit
or connection loss leaves uncertainty. A released advisory lock alone cannot make
such an effect pass the drain. These tokens contain no message, attempt or send
state and cannot authorize a send. Unresolved tokens require the coordinated real
Core/agent reconciliation; no force-clear API is provided.
`DrainedRecovery` holds the exclusive lock until snapshot completion. A new
operation cannot borrow the old operation's generation. `resume_recovery` is a
source-only compare-and-set of epoch/operation/generation/revision;
`quarantine_recovery` creates a fresh epoch and has no automatic release API.

Handler invocations, background engine reconciliation/provisioning, native send
admission/begin and ManagedTransport hold effect guards. The transport guard
lasts through the final SMTP/HTTP result. The existing customer transaction fence
is **not** evidence that transport drained. Private payload insert/remove triggers
also stop cleanup while fenced. Existing payload UUIDs, ciphertexts, MailCipher
AAD, key derivation and plugin-kit sealing formats are unchanged. The key sentinel
can be verified read-only while fenced, without an INSERT-on-conflict attempt.

These methods are internal safety primitives. Their possession does not prove
Core policy authority. There is deliberately no public backup action, generic
scheduler grant or synthetic browser authority in this implementation. Core must
admit the real operation and revalidate its policy before source resume.

## Matched Mail set and required Core hooks

Inventory, with exact sources:

- Canonical Core `webdock_mail.send_job`, `provider_lease`, customer accounts,
  quotas/reservations, source context and send receipts; native incoming and
  native configuration operations/checkpoints/restrictions. Core owns all send
  state, attempts and authority.
- The Mail private PostgreSQL database: migration journal, `domains`, `relays`,
  `sends`, legacy `engines`, `project_customers`, `customer_engines`, `mailboxes`,
  `private_payload_key`, `private_payloads`, and the closed recovery control/effect rows.
  Installation metadata database/role names, ownership markers and actual login
  recovery material are part of operator recovery.
- Existing Stalwart CE state and configuration for incoming-enabled customers,
  including retained suspended engine data when present. Never provision CE for
  outgoing-only backup/restore. Snapshot all actual retained engine allocations
  after ingress, reconciliation and every writer process are stopped and observed
  offline, using the supported offline storage format. This local proof does not
  claim CE recovery.
- Runtime image/manifest digest, readable schema, generation/provenance and private
  configuration. Native runtime enrollment and provider/API/browser authority are
  not replayable restored credentials.
- Independent key custody: retained Mail `secretKeyHex`, existing payload key
  derivation/sentinel and required Core encrypted fields. These master keys belong
  only in separately protected operator recovery material, never customer exports.

Proposed coordinated hook files (not edited by this task):

- `crates/mail/src/{sending,managed_control,submissions}.rs` and the real native
  MailAuthority path: block admission, claim, handoff and begin_attempt at a
  persistent recovery epoch, drain in-flight attempts, preserve unknown/review as
  closed, and fence late callbacks against the original epoch/attempt. Stop all
  legacy/pre-gate executors before trusting the private drain.
- `crates/plugin-host/src/native_mail.rs`, existing incoming/configuration native
  operation/agent paths and `plugins/mail/src/{engine,mailboxes}.rs`: pin and close
  CE transitions before observed offline snapshots. Incomplete transitions require
  reconciliation; a copied volume is not proof of a completed CE operation.
- Existing `hosting_operation` admission/audit/lease infrastructure plus one
  coordinated forward migration for durable backup policy references and epochs.
  The exact next migration number must be assigned after frozen acceptance.
- Restore must create isolated new databases/volumes, preserve required business
  identities and encrypted formats, start the Mail private gate quarantined and
  disable restored Core dispatch, sessions/API keys and callback enrollment before
  any listener or worker gets network access.

A consistent source fence does not establish later history after the recovery
point. Accepted messages remain definitive and are never replayed; unknown,
handoff, review and queued historical messages remain closed on the fresh target.
An explicit later operator reconciliation/cutover decision is required even for
messages that appeared queued at snapshot time. The required local SMTP proof of
confirmed-once/unknown-blocked behavior belongs to the coordinated real Core
integration; the private storage proof does not substitute for it.

## Active Rust application recovery hooks

`crates/agent/src/storage/recovery.rs` is prepared but intentionally not linked by
`storage.rs`. Its tests import it directly against the actual native
`VolumeRecord`, `StoragePacket` and `StorageGuard`. It rejects identical source
and target apps/UUIDs, mismatched project/cluster/owner packets, non-ready records,
insufficient fresh capacity and missing peak reservation. The required reservation
is at least retained source bytes plus twice target bytes (target plus private
restore staging), with each transfer bounded by the existing 64GiB adapter limit.
It permits only ntfy `cache.db`, `auth.db` and flat attachments; never pool state,
`volume.ext4`, `service-kind.json`, environment or source ownership metadata.

Exact proposed shared hooks after coordinated unlock:

1. `crates/agent/src/storage.rs`: link `pub mod recovery`; add privileged fixed
   data-only recovery requests alongside existing ensure/state/retained commands.
   Reuse `StoragePool::volume_state`, image UUID/mount validation and pool lock.
   `StoragePool::ensure` must allocate a **new** target and preserve its new
   `VolumeRecord.uuid`. Copy only application data under target `mount/data` with
   UID/GID 65532 and restrictive modes. Never overwrite either pool's journal,
   filesystem UUID or source record.
2. `crates/agent/src/management.rs` and existing supervisor Executor/Journal:
   authenticated operation lease/revocation guards before each step and callback,
   private recovery checkpoint, source generation/spec/intent fencing. Resume the
   source only when that same owned stop fence and desired revision still apply.
   Do not map a backup operation to an unaudited scale/retire/redeploy command.
3. `crates/agent/src/legacy/lifecycle.rs` (the active Kubernetes adapter's retained
   storage implementation) and `kubernetes/lifecycle.rs`: use actual deployment
   UID/resourceVersion/spec and observed absence of **all** PVC consumers; stop
   gateway/secondary consumers too. Keep existing replica ownership/SSA rules.
   `crates/agent/src/docker.rs` local backend gets the equivalent exact container,
   volume and all-writer checks; existing unrelated containers stay untouched.
4. Native hosting admission/service, existing command schema and agent protocol:
   add the explicit ntfy consistency profile and operation references only in the
   coordinated phase. Reserve retained source, target and restore staging before
   writes. The fresh target starts with deny-all egress and isolated ingress,
   never production environment or job dispatch enabled by a restored config.

The Python profile in `agents/kubernetes/ntfy_recovery.py` is explicitly a
**disposable data-profile demonstration**. It is not in the native agent image
and is not final native product integration. Its fixtures use local operator
volume UUID receipts, not fabricated Core resource ownership evidence.

## Authority, S3 and key custody contract

Use dedicated PG's standing-policy concepts without editing its domain: the real
activating member/operator authorizes an audited, revocable policy bound to the
current customer/project/resource and immutable destination/credential revision.
Scheduled runs use that standing policy after the activating browser expires;
they still fail on resource transfer/deletion, membership/entitlement revocation,
policy change, revoked installation or changed resource generation. No dummy
principal/session. Product operations, recovery-set references, leases and retention
stay in existing canonical ledgers. S3 holds bytes and receipts only.

The backup principal needs Put/multipart/abort plus Get for verified readback, with
no blanket Delete. The restore principal has Get only for referenced recovery
sets, never upload/delete. Referenced sets have no blanket TTL/GC. A lost upload
acknowledgement stays incomplete until verified/admitted; no receipt alone can
complete a product operation.

The fixture encrypts bytes and separately retained key/config recovery material
using maintained **age**, with a local operator key outside the source data
directory. It is explicitly not production key custody. Production activation
remains disabled pending approved independent key escrow/recovery operators,
signer/enrollment, provider/destination/IAM and retention inputs. No irreversible
lock or production cutover is included.

## Disposable proof commands

```bash
cargo test --manifest-path plugins/mail/Cargo.toml --locked
cargo clippy --manifest-path plugins/mail/Cargo.toml --locked --all-targets -- -D warnings
cargo test --locked -p webdock-agent --test application_recovery
python3 -m unittest discover -s agents/kubernetes -p test_ntfy_recovery.py
python3 tools/tests/mail-application-recovery.py /absolute/path/to/coordinated/recovery-object
```

`tools/tests/recovery-object.rs` is prepared fixture source for the existing
`crates/object-storage` verified multipart adapter. Its standalone build wiring
was removed when the tighter dependency freeze arrived; no new root dependency
or workspace member was added. The executable used by the local proof was built
before that instruction. Rebuilding it requires the parent to coordinate fixture
build wiring; the fixture never edits Cargo manifests itself.

The ntfy fixture pins an already cached image, stops all its own writers, performs
SQLite's backup API while offline, hashes the actual schema/files, uploads an
encrypted set and removes source execution/local staging. It restores into a new
directory and isolated network-none container, preserves the logical HTTPS origin
in its own network namespace and tests the stored attachment URL against the
**fresh target**. Authentication, wrong CA/password, anonymous topic denial and
attachment bytes/URL permission behavior are checked. Native filesystem allocation,
Core standing authority, incoming CE and full Mail delivery recovery remain the
separate integration gates described above.

The tested ntfy version intentionally serves a known attachment URL without a
separate login (HTTP 200), both before and after recovery. Topic polling remains
protected: anonymous requests get 403 and a wrong password gets 401. This proof
preserves that application's bearer-URL behavior; it does not introduce private
attachment authorization. The supported origin strategy preserves the logical
origin in a separate network namespace; arbitrary URL remapping is not claimed.
The Docker demonstration bounds copied bytes and reserves staging, but does not
claim a native ext4 quota-backed target volume. That is the pending Rust storage
pool/executor integration above.
