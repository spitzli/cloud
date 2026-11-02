# Native signed plugin updates and rollback

The owner approved: install PostgreSQL v1, write SQL, update to v2, roll back to v1, and read the same data using the original credentials. Incompatible schema readers must be refused. This extends the native initial installer; it does not authorize production rollout, generic jobs, Mail/Hosting expansion, backups or purge.

## Ownership and generations

Keep installation-owned metadata database/role/password and encryption key. Retain immutable signed release provenance, runtime specs, private material, bindings, checkpoints and operations for every runtime generation. A rollback deploys retained release bytes as a **new** monotonically increasing runtime generation; it never revives an old generation's authority.

New deployments use runtime spec version2 with a namespace derived from platform, plugin and generation. Existing version1 specs and resources remain unchanged and can be suspended with their original identity. Each generation retains the fixed names within its own namespace and a new runtime TLS identity/hostname. This replaces the initial profile's stable-runtime-namespace choice; external storage identity remains stable. It avoids adopting or mutating an old generation's immutable Secret, selectors or fingerprint.

No data volume or customer workload is deleted or replaced. Superseded runtime namespaces remain stopped and retained; purge is separate work.

## Admission and recovery

Reuse the operator-only Console availability command to disable and drain accepted work before update/rollback admission. Lifecycle requests bind the reviewed target, expected revision, exact capabilities and idempotency key. Configuration and placement are retained server-side; reject relocation or incompatible configuration. Current operator authority, trusted loaded catalog, release provenance, enrollment and agent capability are revalidated during execution.

Maintain one serialized lifecycle operation per installation and fence competing state commands. Preserve the currently promoted version/image/current generation until authenticated atomic promotion of the candidate. The supported feature ceiling and ordinary bindings always resolve through the promoted generation, never any historical/candidate row.

Prepare candidate material without starting it. Persist a quiescence plan for every potentially running predecessor or failed candidate. Dispatch suspension using each exact retained spec, material and complete six-UID checkpoint; wait for observed replicas zero and no remaining Pods, including terminating/unready Pods. Only then allow candidate Apply. Missing/replaced custody or uncertain shutdown blocks execution and retains resources for reconciliation. It never licenses a second writer.

Failed lifecycle recovery retains its candidate and material. Exact retry does not allocate replacement generations or regenerate keys. A rollback from a failed candidate must also quiesce that candidate before starting another writer; if custody is insufficient, fail closed rather than guessing. Source and candidate callbacks use fresh per-step tokens and existing exact issued/accepted/obsolete receipt custody. Obsolete ACKs release outbox custody only and cannot advance lifecycle steps or renew authority.

## Schema contract

Manifest `schemaVersion` and `readableSchema` describe **logical retained state**, including ordinary writes and migrations. PostgreSQL's three private SQL migration numbers are a different numbering domain. Do not infer logical schema3 from physical migration3.

This milestone trusts the reviewed signed publisher to declare those logical bounds truthfully. Describe/Health authenticate the exact admitted release; they do not independently attest arbitrary SQL schema integrity. No down migrations, physical schema undo, checksum adoption or new Health field is implied.

Retain an installation-owned monotonic logical schema high-water H. For an existing managed installation, initialize H only from its verified signed initial-install provenance; unsupported legacy/file installations fail closed. An empty initial installation explicitly initializes H from its release contract.

For update/rollback, require the target reader interval to contain H and reserve `max(H,target.schemaVersion)` atomically with durable admission, before any candidate can start. Recheck the reserved bound at dispatch/guard/promotion. Never decrease it after failure, cancellation, retry or rollback. A reservation followed by a pre-start crash can conservatively block an older reader even if SQL never ran. Enabling a retained runtime must also respect this ceiling; source Suspend remains permitted so an incompatible predecessor can be stopped safely.

## Agent compatibility

Specv1 serialization/fingerprints remain identical. Specv2 uses the new generation namespace helper. Poll responses advertise the supported spec version; absent/zero means1. New agents advertise capability2 only after this connection negotiates2, resetting on reconnect. Old strict Core continues seeing capability1. Lifecycle dispatch, including v1 source suspension, requires current authenticated capability2 observations and rechecks it on guards.

Stronger suspension preserves six-object ownership checks, performs a UID/resourceVersion-conditional replicas0 patch even if already zero, waits for the resulting observed generation and empty Pod list, and checks authority before returning. No namespace adoption, broad deletes or PVC purge.

## Console and acceptance

Expose safe lifecycle metadata/history, current/candidate versions and decimal-string generations/revisions. Update and rollback review show exact signed release identity, reader compatibility, permissions and retained data behavior. Reuse durable operation pages, exact uncertain-response retries and latest-operation links. Ordinary users, read-only preview and acting owners cannot access global lifecycle controls.

Positive real acceptance uses two distinct signed OCI releases built from the same reviewed PostgreSQL source with different release configurations. Both declare logical schema1. Record this as **compatible code-only lifecycle**, not proof of physical schema migration. Verify new runtime generations/namespaces/images, old runtime quiescence, retained metadata/password/sealing key and real SQL after update and rollback.

Separate restricted policy tests reserve schema2, inject failure before startup, and deny a schema1-only rollback before dispatch. Also cover conflicting idempotency, stale review/revision, revoked operators/agents, lost receipts, capability downgrade, failed-candidate custody and concurrent admission. Final independent review, full quality and owned fixture cleanup precede signed integration on the existing branch.
