# Protected archives implementation plan

Goal: runnable opt-in native archive producer using existing Core resource admission,
immediate jobs and authenticated binary downloads. Scope: PostgreSQL logical dumps
of explicitly bound loopback databases and compressed structured diagnostics.
No existing active database/project/report export implementation was found (searched
Rust modules, plugins, packages and legacy adapters). Build logs are arbitrary text;
redaction cannot establish that they contain no secrets and they are excluded.

Architecture: standalone `plugins/archives`, private PostgreSQL receipt metadata,
shared verified `crates/object-storage` uploads/readback, bounded private staging,
provider presigns returned only through the existing frozen StoragePlan ABI.
No Core migration, second queue/auth model, gateway change or object-storage manifest
change. Database backup/restore code remains another agent's responsibility.

- [x] RED/GREEN: typed diagnostic fields reject unstructured text, unfinished state,
  unknown keys and oversized input; compression roundtrip.
- [x] RED/GREEN: native producer records immutable receipts, resumable job identity,
  tenant scope, ready-only downloads, producer epoch, expiry and deletion denial.
- [x] Native executable uses existing ActionHandler/JobHandler and HostClient enqueue;
  Core owns reservations, current feature authority and opaque TransferDescriptors.
- [x] Real owned MinIO + fresh PostgreSQL acceptance: dump/restore, diagnostics,
  integrity, failures, restart, cross-tenant and delete/expiry; exact cleanup.
- [x] Record commands, results and remaining limits in dated verification evidence.

Decisions: bounded archive output (32 MiB), two concurrent producers, generation jobs
are serialized per archive by a private PostgreSQL session lock. Source credentials
stay in private runtime configuration. Storage bucket must have no versioning and an
enabled bounded expiration + multipart-abort policy, verified at runtime startup;
this also bounds any upload orphan after process death. Receipt state is not a queue.
Access expiry is at most 24 hours; underlying byte retention at most seven days.
Automatic provider expiry is not an immediate deletion receipt. Explicit delete
revokes producer authority before touching S3 and acknowledges only after exact-key
absence is observed. No provider/public bulk-delete API is added to the shared crate.

Outcome: scoped producer and both real acceptance paths complete. The exact remaining
product/infrastructure limits are recorded in
`docs/verification/2026-10-25-protected-archives.md`. No broader section-completion or
production-compatibility claim is made.
