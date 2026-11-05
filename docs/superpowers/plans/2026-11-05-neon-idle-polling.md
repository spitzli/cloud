# Neon idle polling reduction plan

**Goal:** Reduce repeated empty database work while preserving current authority,
lease, restart and active-operation behaviour. The owner prioritised this before
continuing the existing TODO queue.

**Evidence:** A bounded, read-only production sample found continuous empty-queue
traffic. Detailed consumption and connection evidence remains in private operator
records. The plugin controller discovers jobs,
availability operations and installations every 500 ms; the enrolled agent repeats
all its idle domain polls and heartbeat every second. Core readiness verifies the
restricted role and schema every ten seconds. Existing compute limits and the plan-default suspend
timeout remain unchanged. No database extension was installed.

## Scope and constraints

- Back off only successful **empty** plugin-controller and agent cycles.
- Controller cadence: 500 ms on progress; empty cycles grow through 1, 2, 4 and
  at most 5 seconds. Job completion, state work and installation work count as
  progress. A still-running job retains its own timeout and authority checks.
- Agent cadence: 1 second on work, including retained pending receipts; empty
  cycles grow through 2, 4 and at most 5 seconds. Reconnection starts with a fresh
  idle delay; transport backoff remains independent.
- Startup still polls immediately. Shutdown remains interruptible; controller
  `--once` behaviour is unchanged. Idle work-discovery latency is bounded by five
  seconds plus request processing; this deliberately trades the previous empty
  polling rate for a small dispatch delay.
- Keep heartbeat behaviour during work, lease deadlines, revocation, database
  roles, RLS, readiness, rollout guards and connection policies unchanged.
- Do not reduce production compute limits or remove existing branches merely to
  manufacture cost savings. Continued health probes and other background queries
  can prevent scale-to-zero even after this change. Measure CU-hours separately
  from transaction and egress reductions.

## Implementation and verification

- [x] Add failing tests for bounded idle progression, reset on real work, and a
  20-second empty/active polling budget in the two affected Rust components.
- [x] Wire the controller's actual completed-job/state/install results into its
  delay and the agent's existing cycle-work boolean into its idle sleep.
- [x] Run focused tests, relevant existing controller/agent authority and recovery
  regressions, Rustfmt and Clippy. Obtain independent review before merge.
- [ ] Build and qualify exact OCI executables on existing EU infrastructure. Stage
  the controller change, then the enrolled agent; retain rollback and identity.
- [ ] Repeat the bounded database activity sample and actual operation/readiness
  checks. Compare the later provider consumption window without claiming that
  fewer queries alone prove lower CU-hours.
- [ ] Record remaining sources of idle traffic, remove temporary credential
  custody, and continue the existing TODO queue.

**Files:** `crates/control-plane/src/bin/webdock-plugin-controller.rs` and
`crates/agent/src/management.rs`, plus the dated qualification/operations records.
No schema or protocol migration is required.
