# Native durable plugin jobs implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use checkbox syntax for tracking.

**Goal:** Execute immediate session-admitted plugin jobs durably through Rust Host/runtime.
**Architecture:** Retained PostgreSQL queue and actor-backed SQL authority; short signed
attempts over authenticated UDS/mTLS; fixed leases with bounded retries and native controller.
**Tech Stack:** Rust, PostgreSQL18, SeaORM, tonic, existing Ed25519 claims and RLS.
**Spec:** `docs/superpowers/specs/2026-10-24-native-plugin-jobs-design.md`

## Global Constraints

- Rust is the only active API; Bun only for tooling. No new queue dependency.
- Migration0044 only; never modify accepted0042/0043 or retained baseline.
- Immediate project/session jobs only; no schedules, system jobs, apps jobs or nested jobs.
- Real actor/session and original exact authority; 32768-byte payload/result.
- Attempts default3/max5; runtime45s/token60s/lease90s; no renewal.
- Stable job ID and operationID; fresh claim/invocation per attempt; at-least-once semantics.
- Preserve untracked handovers; never read `.dev-local/demo-owner.json` or print credentials.
- No production/provider/mail/firewall changes. Existing dev/k3s services stay stopped.
- Root owns final staging/sign/push. Workers do not commit or run broad quality.

## Review Focus

- Parent normal cleanup versus session/acting revocation: only the former permits continuation.
- Lost completion after effect: stable receipt identity and stale lease fencing.
- Disable with expired/exhausted work: terminalize rows and release continuation gates.
- Same action/job name and token replay: distinct RPCs deny kind confusion.
- Auth pool size1 and concurrent cancel/complete: no lock inversion or cross-scope access.

### Task 1: Signed job protocol and bounded runtime

**Files:** plugin-protocol invocation/manifest/contracts; plugin-runtime lib/host/transport/
pinned_tls and tests; `proto/webdock/plugin/v1/plugin.proto`; existing claims constructors.
**Interfaces:** Produce optional `JobIdentity {id:String,attempt:u8}` in `InvocationClaims.job`;
`Manifest::job(&str)->Option<&Value>`; `JobHandler::run(claims,token,input,configuration)`;
`Runtime::with_jobs(Arc<dyn JobHandler>)->Self`; `RunJobRequest/Response` with same payload
fields as InvokeRequest/Response. HostClient accepts jobs enqueue/get/cancel.

- [x] Write failing token-golden/kind-confusion, declaration, payload and cancellation tests.
- [x] Implement optional non-null signed marker, RunJob with45s absolute deadline and bounded
  authority checks; leave action30s deadline. Make transport budget compatible.
- [x] Relax native lifecycle only for immediate data/http jobs, rejecting intervalSeconds and
  unsupported job adapters. Preserve other lifecycle restrictions.
- [x] Run protocol/runtime tests and scoped clippy. Include all constructor compatibility fixes.

### Task 2: Persisted admission, leases and authority

**Files:** plugin-host new jobs module(s), authority/scope/session/self_service/rpc; migration0044;
plugin-host tests/jobs.rs and owned fixture helpers.
**Interfaces:** Export opaque `JobLease` (no Debug/Serialize), getters `id()`, `plugin()`,
`generation()`, `manifest()->&Manifest`, `invocation()->&AdmittedInvocation`; `HostService::claim_job(Option<&str>)
->Result<Option<JobLease>>`; `complete_job(&JobLease,Result<Value>)->Result<()>`;
`jobs_enqueue/get/cancel` routed from native Host Call. All public values match spec.

- [x] Write owned restricted PG regression: enqueue before implementation fails.
- [x] Add immutable provenance/result/attempt bound and narrow signed SQL helpers; legacy
  rows cancel. Actor-backed job invocation plus lease required in plugin_context/lock function.
- [x] Implement canonical scoped dedupe, get/cancel privacy, grant jobs, no nested enqueue.
- [x] Implement auth-first claim/reclaim/finish, real subject/authority/policy revalidation,
  stable effect receipts, drain and bounded retries. No provider await while holding transaction.
  Preserve transient verifier errors as unavailable/deadline; cancel only confirmed denial.
- [x] Prove parent cleanup, session/acting exit/reentry, grant/config/feature/resource loss,
  forged SQL, partition denial, two claimers/pool1, stale lease/receipt, cancel/complete races,
  max5 exhaustion and terminal drain with owned disposable PG. Run scoped clippy.

### Task 3: Native controller and complete RPC acceptance

**Files:** control-plane plugin_jobs module/controller binary; acceptance in plugin-host/tests/support/jobs_rpc.rs; quality runner
if its current target selection misses new tests; operations guide/parity verification.
**Interfaces:** Consume Task2 JobLease and Task1 RunJob through exact generation/pinned
RuntimeConnections. Controller job execution outside DB transactions; errors sanitized.

- [x] Write integration test using native bootstrap restricted roles, real Host/runtime UDS and
  synthetic plugin whose job writes a document; first run demonstrates missing controller.
- [x] Wire `PluginEndpoints::advance_job()` and controller loop. Keep state/install advancement
  responsive while a bounded job is executing; preserve ctrl-C cancellation and crash leases.
- [x] Prove enqueue -> worker -> real handler -> host write -> status, retry and worker restart,
  cancellation, drain, ordinary actions unchanged. No Redis/k3s/production services.
- [x] Document protocol/lifetime/limits/at-least-once boundary and operational command.
- [x] Run targeted integration and `bun run quality`; fresh independent full-change review,
  one coordinated fix wave if needed, verify cleanup and signed branch integration.
