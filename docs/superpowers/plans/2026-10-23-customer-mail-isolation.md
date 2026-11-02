# Customer mail isolation implementation plan

> **For agentic workers:** Execute inline with TDD and verification. Root owns Core changes and final review; no nested agents, commits or pushes.

**Goal:** One retained Stalwart CE engine per explicitly incoming-enabled customer across all their projects.

**Architecture:** Trust required signed `InvocationClaims.customer_id`, derived by Core from the project owner. Keep resources project-scoped; persist mailbox customer ownership and key engine reservations, workload units and hosts by customer. Core accepts customer-owned workloads with no project lifetime dependency.

**Tech stack:** Existing Rust plugin, PostgreSQL, plugin workloads, React/Vite UI; no new dependencies.

**Spec:** `docs/architecture/mail-deployment-decision.md`.

## Constraints and review focus

- Only `plugins/mail/**` plus this plan; preserve concurrent edits and handover/TODO files.
- No production/provider/mail actions; disposable local fixtures only, no secrets or DB URLs in output.
- Legacy project engines stay quarantined, retaining data; never infer a customer or provision a replacement over legacy mailbox data.
- A lost management response must never remove a workload or volume.
- Cross-project customer reuse must preserve project resource authorization and deny cross-customer mailbox access.
- Suspension retains engine identity, credentials and PVC; activation/restart reconcile desired versus observed state.
- Outgoing domains, relay and sending never allocate engines; failed/suspended incoming must not silently use the outgoing transport.

## Tasks

- [x] RED: Customer hosts use `c<customer>` and unavailable management never destroys workloads. Run focused existing mailbox tests.
- [x] RED: Real disposable PostgreSQL fixture tests customer reuse, mailbox ownership, outgoing-only, suspension/restart desired state and legacy quarantine.
- [x] Implement customer registry migration without deleting legacy records, mailbox customer binding and fail-closed lookup; signed context binding.
- [x] Implement stable customer workload scope, retained bootstrap/reconciliation and incoming activation/suspension actions.
- [x] Update manifest, README and UI for customer engine lifecycle and migration-required state.
- [x] GREEN: Run mail crate focused tests including disposable PostgreSQL fixture and existing sending fixtures. Root runs full quality gate; include the owned real k3s fixture.

## Decisions and evidence

- Root confirmed exact claims/workload interface; Persistence0040 reserved for Core. No arbitrary project owner is used for customer engines.
- Local `webdock-dev` initially refused connection; root started its owned local fixture. Real two-customer CE lifecycle passed (190.45s) with retained PVC, cross-project reuse, cross-customer denial and interrupted account creation on restart. No deployed assets changed.
- RED→GREEN: customer hostname and no destructive management recovery; idempotent domain/account recovery; direct-send fallback denial; apply-revision acknowledgment before suspension.
- Suspension keeps desired=false/state=suspending until host observes the applied revision and zero replicas; root separately owns the agent zero-replica/controller-generation readiness fix.
- Final mail-crate fixture suite and scoped Clippy/UI type checks are the completion gate; root retains whole-repo review and quality ownership.

## Adjudicated review fix pass

- Finding 2: permanent admin authentication selects the configuration client; every reconcile finishes listener/certificate/default settings and recovery removal. Regression covers five interrupted boundaries, repeated completion, retained account/credentials/certificate and misleading HTTP 503/403 before confirmed HTTP 401.
- Finding 3: incoming intent admission does not take the long-running I/O mutex. Both activation and deactivation persist desired state and schedule the existing detached reconciler, which reads current intent under the execution lock. Disabled intent stays suspending on observation failure; state writes keep the existing desired-state fence.
- Barrier regressions prove prompt deactivation, no premature suspension before observation, disabled intent surviving a fresh runtime, retry after observation failure, and same/other-customer activation admitted while suspension waits. Root owns UI suspending polling and the browser fixture follow-up.
- One scoped fix pass; no extra review dispatch, action deadline increase, dependencies, destructive cleanup or provider actions. Full real fixture suite and scoped Clippy/UI checks rerun before refreezing.

Final integration: all15 Mail tests passed (7 customer PostgreSQL fixtures,3 real-k3s mailbox
checks,1 repeated-interruption JMAP boundary and4 sending checks). Scoped Clippy/UI checks and
root's complete `bun run quality` passed. Root also verified the actual agent's zero-replica
readiness and async completion polling on both domain and mailbox pages. No real mail was sent;
public reception, incoming relay, explicit legacy migration and plugin Webmail/outbox parity
remain open. Incoming-reserved customers cannot silently use the direct outgoing transport.
