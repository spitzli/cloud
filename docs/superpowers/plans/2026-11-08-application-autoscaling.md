# Application Autoscaling Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans inline, as already requested by the owner. Preserve unrelated native-Git work and original-root hashes.

**Goal:** Automatically adjust native application instances within admitted min/max bounds while preserving manual mode and stop/recovery safety.

**Architecture:** Kubernetes HPA controls replicas; Core controls the policy and reserves its maximum through existing transactional admission. Versioned agent capability and strict signed contracts gate activation. No separate scheduler or polling loop against Neon.

**Tech Stack:** Existing Rust Core/agent, PostgreSQL JSON specs, approved Helm chart, Kubernetes autoscaling/v2, React Console and shared TypeScript contracts.

**Spec:** `docs/superpowers/specs/2026-11-08-application-autoscaling-design.md`

## Global constraints

- Optional policy; fixed apps remain unchanged. Min 1, max 20, CPU target 10–90%, UI default 70%, downscale stabilization 300 seconds.
- Reserve the maximum; never change auth/RLS, operation fencing or recovery safety to make a scaling test pass.
- No public activation before real metrics-based, owned-cluster qualification. No customer load tests, new credentials or paid servers.
- No schema migration for the JSON field; keep pending migration 0076 and original dirty files intact. Targeted edits to overlapping Hosting code require before/after preservation checks.

## Review focus

- In-flight HPA scale after stop/suspension must not resurrect workloads or prematurely release reservations.
- Switching fixed/automatic modes must leave exactly one replica authority.
- Missing metrics must not appear as zero utilization or successful scaling.
- Concurrent customer admissions must reserve max capacity atomically.
- Old agents and plugin-owned/local-volume workloads must refuse unsupported policies.

### Task 1: Shared policy and maximum-bound admission

Files: `crates/hosting/src/app-spec.schema.json`, `crates/hosting/src/contracts.rs`, `crates/hosting/src/apps.rs`, `packages/hosting-contracts/src/app-spec.ts`, `packages/hosting-contracts/src/index.ts`; focused new test files alongside existing contract and restricted PostgreSQL tests.

Interface: optional `autoscaling: {minReplicas, maxReplicas, targetCPUUtilizationPercent}` in app spec; absent means existing fixed replicas. `app_demand` returns the maximum-bound demand while active, and the existing stopped/deleted demand otherwise.

- [ ] Add failing Rust/TypeScript parity tests for bounds, strictness, volumes, fixed compatibility and direct manual scaling refusal in auto mode.
- [ ] Add restricted PostgreSQL tests for max reservations, concurrent admissions, cross-tenant denial, policy update and stale revision.
- [ ] Implement the optional policy, explicit fixed/auto transitions and maximum-bound admission without a schema migration.
- [ ] Run focused contract suites and the owned PostgreSQL fixture; require every new denial and positive case to pass.

### Task 2: Signed runtime and cluster compatibility

Files: `crates/supervisor/src/spec.rs`, `crates/hosting/src/runtime.rs`, `crates/hosting/src/agent.rs`, runtime capability definitions and their existing consumers located by symbol search; constructors/tests of `RuntimeSpec`.

Interface: optional signed policy; versioned automatic-scaling capability. Policy absence preserves existing contract; Core refuses dispatch to agents without the qualified version. Observed runtime counts remain separate from policy bounds.

- [ ] Add failing tests for old agent refusal, tampered policy/hash, preserved fixed-spec serialization and allowed policy transfer.
- [ ] Implement signed policy propagation and capability checks in admission, claim and reconciliation.
- [ ] Update completion proofs to verify policy, rollout and actual count without forcing a fixed replica equality in auto mode.
- [ ] Run protocol, supervisor, Hosting authority and replay tests. Do not loosen uncertainty handling.

### Task 3: Owned HPA lifecycle

Files: `deploy/helm/webdock-plugin/values.yaml`, `deploy/helm/webdock-plugin/templates/runtime.yaml`, new `deploy/helm/webdock-plugin/templates/autoscaling.yaml`, `deploy/helm/spitzli-agent/templates/agent.yaml`, `crates/agent/src/kubernetes.rs`, `crates/agent/src/kubernetes/lifecycle.rs`, `crates/agent/src/recovery/kubernetes.rs`; focused HPA module if needed.

Interface: active auto rendering omits static replicas; exact namespaced HPA controls the approved Deployment. Fixed/stopped specs write replicas. Agent stop and recovery share HPA-removal and zero-Pod confirmation.

- [ ] Write failing render/ownership tests for target, metrics, bounds, unexpected HPA, namespace escape and fixed/auto transitions.
- [ ] Add HPA generation, narrowly scoped lifecycle permissions, ownership/retirement validation and compatible capability reporting.
- [ ] Test delayed HPA scale after stop, lease loss during HPA deletion, rollback, resume, fixed transition and recovery. Uncertainty must not confirm completion.
- [ ] Run owned-cluster tests with an actual metrics server and CPU load, verify min/max and multi-instance traffic distribution, then metrics loss and recovery.

### Task 4: Customer controls and status

Files: `apps/console/src/HostingApplication.tsx`, `apps/console/src/HostingSettings.tsx`, `apps/console/src/HostingCreate.tsx`, `apps/console/src/hosting-contracts.ts`, `apps/console/tests/hosting-contracts.test.ts`, focused scaling state/browser tests.

Interface: fixed/automatic mode, min/max and CPU target inputs, current/ready instances and HPA conditions. Report reserved max separately from running usage. The existing action/receipt pattern remains authoritative.

- [ ] Write failing UI tests for form validation, old agents, denied permissions, delayed/uncertain operation, absent metrics and mode switches.
- [ ] Build the controls using existing components and contracts, including server errors and keyboard-accessible labels.
- [ ] Test the actual compiled Console at desktop/mobile sizes, then build/lint and shared contract checks.

### Task 5: Controlled release

Files: new scaling operations guide, qualification evidence and feature-gate configuration in existing configuration boundaries.

- [ ] Document policy/reservation semantics, metric prerequisites, unsupported workloads, rollback and stopped-app behavior.
- [ ] Independently review the complete change and fix material findings before a signed commit.
- [ ] Run Blacksmith checks, publish signed compatible releases, qualify a disposable application and retain a rollback image.
- [ ] Activate only on a qualified cluster, explicitly for an opted-in app. Record real scale-up/down and stop results before describing autoscaling as live.
