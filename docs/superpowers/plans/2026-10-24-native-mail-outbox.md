# Managed Mail outbox implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Track steps with checkboxes.

**Goal:** Move managed Mail message custody and execution onto the reliable canonical
send ledger, including plugin actions and the native API, with no uncertain resend.
**Architecture:** One Core transaction-bound control ledger retains source/budget/job/
receipt authority. A Core-independent runtime library supplies private payload custody,
ingress, dispatch and cleanup to the Mail plugin and compatibility executables.
**Tech Stack:** Existing Rust/tonic/Axum/PostgreSQL, React Console, Bun, local SMTP catcher.
**Spec:** `docs/superpowers/specs/2026-10-24-native-mail-outbox-design.md`.

## Global constraints

- Base5fe5576; preserve unread handovers, live Spitzli, retained keys/storage and stopped dev k3s.
- No production/provider/real delivery/live migration, broad image cleanup or new queue backend.
- Native migrations0042–0044 stay immutable. Only Task2 owns new native0045.
- Canonical Core ledger remains atomic with native source checks and recipient allowance.
- No private queue, fake project/customer/invocation or autonomous generic-job authority.
- Only expired preparation claims retry. Rejected is terminal; handoff/unknown never resends.
- Preserve exact native API semantics, authority-before-body, encryption formats and legacy wrappers.
- Executor/payload-store assignment is distinct from provider routing; old workers exclude managed data.
- Root owns Cargo/shared-file coordination, actual acceptance authorization, final review/quality/
  cleanup/signed push and requested local-PC shutdown after successful completion.

## Review focus

- Fresh invocation UUID after a lost reply must still resolve the same logical send.
- A DATA-accepted message with no final SMTP reply must remain unknown across restart/replay.
- Incoming activation and outgoing handoff serialize for the same customer across processes.
- Auth pool1 and business pool1 never await each other in the wrong lock order.
- Legacy workers/cleanup never adopt managed PostgreSQL payloads or change old route meaning.
- Native account/mode/revision changes and project relay changes never silently reroute accepted work.
- Candidate/disabled/stale peers cannot start work; exact terminal bookkeeping is not new authority.

### Task1: Shared runtime and compatibility extraction

**Owner:** Runtime worker B; no Core policy/migration or managed dispatcher behavior yet.
**Files:** Create `crates/mail-runtime/{Cargo.toml,src/*,tests/*}`; add Mail protocol DTO
module; update `crates/mail/{src/{payload,send_control,dispatch,provider_transport,send_api,
smtp,secrets,outbox,sending}.rs,Cargo.toml}` only for extraction/compatibility exports;
message helper in `crates/mail-message`; coordinate Cargo locks and quality dependency graph.

**Interfaces:**

- Canonical `PayloadFormat`, `SendLease`, `PayloadLease`, `Receipt`, `Preparation` move to
  `webdock_mail_protocol`, retaining serde names, tags, field order and compatibility exports.
- `webdock_mail_runtime::SendControl::call(&self, Value) -> Result<Value, ControlError>`
  keeps existing command/reply semantics and safe status codes.
- Async `PrivatePayloadStore::{put(customer,reference,bytes),get(customer,reference),
  remove(customer,reference)}` returns the existing `SavedPayload {reference,bytes}` shape.
  Retain concrete filesystem compatibility APIs/format/key marker; adapt with bounded blocking IO.
- Extract native public `SendApi` orchestration; internal Webmail/SMTP-policy adapters stay Core.
- Extract preparation/dispatch/cleanup and phase-aware transport logic without changing receipt
  meaning. Retain public compatibility exports for old binaries/tests.
- Shared message helper compiles the bounded plugin send input plus canonical domain into the
  same canonical native request/envelope on both sides; freeze its exact Rust API before Task2.

- [x] Pin extraction contracts with failing parity/compilation tests; preserve old golden bytes.
- [x] Extract only Core-independent units, preserving rejection/unknown, staged custody and cleanup.
- [x] Run complete shared/default package suites plus existing focused native parity fixtures as
  authorized; scoped clippy/format and dependency-graph checks. Record exact source/API freeze.
- [x] Fresh scoped review before policy/plugin integration relies on this shared boundary.

### Task2: Authenticated managed control, source and lifecycle policy

**Owner:** Core worker A, after Task1 DTO/message API freeze.
**Files:** Mail protocol native-control proto/client/server adapter; Core Mail managed control
modules and existing sending/submission/policy/authority code; neutral plugin-host transaction
helpers and managed policy; control-plane adapter/listener registration; new native migration
`crates/persistence/migrations/0045_managed_mail_outbox.sql`; focused restricted tests.

**Interfaces:**

- `NativeMailControl.Call`: plugin ID/generation/strict command JSON → strict bounded result JSON.
  Implement existing `SendControl` on the client. Spec limits/deadline/status text are binding.
- Reuse existing commands plus `send.admitAction` and `send.actionStatus`. Derive operation/source/
  actor/project/customer/resource from actual persisted invocation; no redundant public authority.
- Action names `mail.send`, `mail.status`, `mail.cancel`, `mail.history` select their operation/intent. Shared
  message compilation recomputes hash/length/envelope, never trusts submitted replacements.
- Explicit managed provider variant supports external IDmail with real current transport/binding;
  preserve legacy builtin-Mail/external-provider behavior and native sender evidence.
- Actual peer/runtime/source checks stay within the same signed authoritative ledger transaction.
  Auth-before-business order and installation fences remain held through commit.
- Canonical jobs carry server-owned `managed-mail-v1` executor/store identity. Legacy operations
  exclude the lane; managed claims/receipts/cleanup validate exact row/lane/token/state.
- A narrow current-action transition barrier may be added if necessary to serialize incoming
  reservation or domain deletion against retained sends; freeze it before Task3 consumes it.

- [x] Write restricted tests exposing external-mail policy, source/lane/peer denial, stable replay,
  actor/grant/resource revocation, pool1 lock order, drain and unknown retention failures.
- [x] Add only forward SQL/neutral helpers needed to preserve atomic authority and state.
- [x] Preserve native key status/cancel, scheduling, shared allowance and definitive receipt settlement.
  Prove old workers cannot claim/delete managed payloads and old paths remain compatible.
- [x] Test schema-H old-reader denial and managed disabled/draining/candidate command distinctions.
- [x] Run complete Core Mail/host defaults and focused owned restricted fixtures; clippy/format,
  then freeze migration and public client/admission APIs for Task3/4. Fresh scoped review.

### Task3: Managed Mail execution and truthful send UI

**Owner:** Runtime worker B for product/runtime; root for Console integration.
**Files:** `plugins/mail/src/{main,startup,lib,store,transport,...}.rs`, focused delivery/payload
modules/tests, manifest/README/Cargo; shared `plugin-runtime` public service/channel support only
where required. Root owns `plugins/mail/ui/index.tsx`, Console invocation helper and browser tests.

**Interfaces:**

- Consume NativeMailControl as `SendControl`, sharing the authenticated callback connector.
- PostgreSQL payload adapter owns encrypted bytes/key marker only, never a second queue. Stable
  purpose-bound key derives from retained sealing material; preserve existing credential ciphertexts.
- Compose native HTTP routes and generic runtime gRPC on the same mTLS listener; actual gateway
  peer pinning is separate from runtime/Core peer authority. Thin public proxy authenticates before
  body polling, only forwards native public routes and has no body replay/redirect behavior.
- `mail.send` keeps input and returns `{id,state:"queued"}`; `mail.status`/`mail.cancel` use owned
  domain resource plus canonical job ID; `mail.history` returns scoped bounded summaries. UI keeps
  one key/snapshot through uncertain retries and only enables the release's declared actions.
- Listener-owned supervised dispatcher handles SIGTERM/Ctrl-C, no candidate work, fresh authority
  before attempt, bounded finalization and conservative crash recovery. No detached sender task.
- Freeze private native account map/mode/revision/digest and project relay pinning. Missing mappings
  fail closed. Shared customer fence covers send admission/handoff and incoming reservation.
- New private payload migration and release logical2/readable[1,2] block unsafe schema1 rollback;
  existing physical/credential migrations stay unchanged. Bump the checked-in Mail release identity.

- [x] Add failing supervised-worker, payload, classified receipt, route/incoming race and restart tests.
- [x] Replace inline delivery; preserve other Mail actions, retained incoming/legacy behavior and Unix/TLS.
- [x] Implement queued/status/cancel UI, stable retry identity and truthful unknown/review presentation;
  retain API/MCP action input compatibility and existing customer access controls.
- [x] Run all product/shared/default tests, focused process tests, Console typecheck/unit/browser,
  scoped lint/format. Freeze source and review before actual root-owned acceptance.

### Task4: Real local native-source and action-source acceptance

**Owner:** Acceptance worker C with root authorization after interfaces freeze.
**Files:** Focused tracked fixture/tests based on Core Mail platform, plugin-host TLS/Mail process
fixtures, local SMTP capture/failure helpers; dated verification/operations records. Root owns
quality runner and final repository integration.

- [x] Fresh independently owned platform/private TLS PostgreSQL, restricted real Core services,
  actual managed Mail executable and thin native gateway. No retained cluster/operator/source reuse.
- [x] Drive native key ingress and real plugin admission through actual authority; same-key/restart
  replay → same job/one catcher message; changed content conflicts; pending cancel prevents send.
- [x] Record complete DATA then drop final reply: unknown retains bytes/allowance and no resend after
  expiry/replay/restart. Cover lost begin/finish acknowledgements and stale competing claims.
- [x] Prove revocation, peer/payload/route/account tampering, incoming race, pool1 and drain fencing;
  confirm outgoing-only allocates no CE/workload/PVC. Preserve retained credential/key identity.
- [x] Build actual OCI if packaging/dependencies changed; consume installer material in real process
  tests. Label HTTP/provider stand-ins and prior Console/PVC proof separately; no new rollout claim.
- [x] Inspect sending UI at375/1280 with no secret/body leakage; exact owned cleanup and private-data
  scrub, preserving initially stopped k3s state. No real external mail.
- [x] Run full quality after fixture cleanup; fresh whole-change review and one coordinated fix pass
  with red/green evidence if needed.
- [ ] Sign/push current branch and verify remote/signature/handoff (post-commit operational receipt).
- [ ] Only root then performs the requested local-PC shutdown. Never put credentials in source,
  reports, child prompts or final chat output.
