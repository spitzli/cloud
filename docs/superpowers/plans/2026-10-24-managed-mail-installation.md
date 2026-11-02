# Native managed Mail installation implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Track steps with checkboxes.

**Goal:** Install Mail through the native Console and retain customer data across update/rollback.
**Architecture:** Existing signed installer and separate retained metadata storage; Mail uses shared
TLS transport and waits for current-runtime authority before retained customer reconciliation.
**Tech Stack:** Rust/tonic/PostgreSQL, existing Kubernetes agent, React Console, Stalwart CE.
**Spec:** `docs/superpowers/specs/2026-10-24-managed-mail-installation-design.md`

## Global Constraints

- Rust-only active API; Bun tooling; no new dependencies or product-specific Core installer branch.
- Existing Unix config remains supported; exactly one listener/callback; remote storage needs verified TLS.
- No customer reconciliation before ordinary runtime authority; candidate discovery stays separate.
- Logical Mail schema1/readable[1,1], physical migrations unchanged; no new native migration expected.
- First mailbox creation explicitly activates incoming; plugin/domain installation does not.
- Retain metadata/storage/master/engine/mailbox identities, credentials and PVCs across generations.
- Preserve legacy quarantine/send fail-closed. No public delivery/relay/Webmail/provider/production work.
- Never read handovers or `.dev-local/demo-owner.json`, print credentials, or change firewall/shared volumes.
- Root owns actual fixture authorization, final quality/review/cleanup/sign/push; workers do not commit.

## Review Focus

- Candidate with retained ready/failed/suspended rows: no JMAP/state/workload effects before promotion.
- Channel authentication and live runtime authorization are separate; TLS success alone does not resume.
- Wrong CA/name/pin or ambiguous config: fail closed without diagnostics containing private values.
- New runtime generation retains metadata and customer unit/PVC; no replacement-engine allocation.
- Installing/outgoing-only/mail-disabled states never provision Stalwart; interrupted intent survives restart.

### Task 1: Mail executable transport, storage and promotion-gated startup

**Files:** `plugins/mail/src/{main,store}.rs`, Mail startup helper if needed; shared
`crates/plugin-runtime/src/host.rs` and focused tests; Mail tests/README.
**Interfaces:** `HostClient::authorize_runtime(&self,plugin:&str,generation:u64)->Result<(),PluginError>`
uses existing AuthorizePeer; `Store::connect_tls(url:&str,ca:&[u8])->Result<Store,plugin_kit::Error>`;
plaintext `connect` enforces loopback. Strict config follows PostgreSQL `RuntimeTransport` fields.

- [x] Write failing config/constructor/Host helper and candidate-startup tests; observe RED.
- [x] Wire shared TLS transport and storage, preserve Unix, redact startup errors.
- [x] Listen before startup reconciliation; use cancellable live-authority wait before `resume`,
  bounded backoff/retries and shutdown ownership. Do not change candidate Core permissions.
- [x] Verify pre-promotion zero effects, post-promotion automatic resume, persisted state/secret
  retention, callback outage and shutdown. Run focused product/runtime tests and scoped clippy.

### Task 2: Real binary and generic installer compatibility

**Files:** owned Mail executable fixture/tests beside `crates/plugin-host/tests/provisioning.rs`
or standalone Mail integration tests; reuse existing TLS PostgreSQL/runtime fixture helpers.
**Interfaces:** consume Task1 strict config/API. Existing generic Recipe produces reserved TLS,
metadata/sealing fields; Mail-specific privateConfiguration supplies platformTransport/mailboxes.

- [x] Add a real Mail executable fixture with owned native TLS metadata and pinned callback/server.
  Verify actual invocations/callbacks and current-versus-candidate authority, not only Describe.
- [x] Prove wrong CA/name/pin/peer rejection, legacy Unix restart compatibility, fixed startup
  diagnostics, physical journal/data/ciphertext retention and no automatic engine allocation.
- [x] Test actual signed Mail catalog/review and generated private config compatibility using
  generic installer contracts. Add no product branch/permission/schema unless a real gap requires it.
- [x] Run owned fixtures, scoped clippy and source freeze before artifact/cluster acceptance.

### Task 3: Signed Console lifecycle and retained customer Mail acceptance

**Files:** tracked bounded fixture preparer/artifact helpers, Mail native Console acceptance script,
operations documentation, parity/verification record. Root owns live orchestration.
**Interfaces:** supported migration/roles/operator/enrollment/kernel CLIs; generic Console
catalog/install/policies/customer/project/update/rollback; actual Mail runtime and Workloads agent.

- [x] Prepare fresh owned fixture only after source freeze: separate platform/metadata PG,
  minimal callback mounts, genuine signed digest OCI Mail releases and reviewed private recipe.
  Record custody first. No reused deleted fixture identity or direct install/runtime seed rows.
- [x] Real Console install proves zero customer engines; domain/relay operations preserve that.
  Explicit mailbox creation activates one customer CE engine; observed readiness, safe local
  mailbox sentinel, credential/PVC and sealed metadata retention are checked privately.
- [x] Disable/drain → update gen2 → rollback gen3 with actual images and predecessor Pods gone;
  re-read retained data/credentials and exercise suspend/resume, reuse and foreign denial.
- [x] Document honest delivery limits and exact generic recipe. Run full quality after fixtures;
  fresh whole-change review, one coordinated fix wave if needed, final gates.
- [x] Clean only verified owned processes/namespaces/PVs/containers/image aliases/private material,
  restore retained k3s state and preserve sanitized proof.

Final Git integration follows source verification: root signs the commit and checks the exact
remote branch head. Post-commit receipts are recorded in the local run handoff and Git history.
