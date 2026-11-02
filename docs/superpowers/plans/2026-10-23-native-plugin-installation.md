# Native plugin installation implementation plan

> Execute independent bounded tasks with delegated implementers; root integrates and requests one fresh whole-feature review. Follow superpowers:executing-plans for each owned task.

**Goal:** Install PostgreSQL through native Console on a fresh platform and use a real managed database without development seeding/manual installation SQL.
**Architecture:** Trusted catalog → durable initial-install admission → private storage/material preparation → guarded installation-owned enrolled-agent deployment → authenticated candidate discovery → atomic promotion. Keep availability commands and existing plugin resource UI.
**Tech stack:** Rust/Tonic/PostgreSQL/RLS, Kubernetes/Traefik, existing mTLS and React/Vite/Bun.
**Spec:** `docs/superpowers/specs/2026-10-23-native-plugin-installation-design.md`.

## Global constraints

- Existing T3 worktree/branch; preserve local handovers and other changes. No commits by children.
- No production/provider actions, real mail, secret/URL output, demo-owner file reads or operator-password changes.
- No dummy customer/project installation owner. No Kubernetes credentials in Core. No privileged storage credentials in plugins/public operation state.
- Only initial install/recovery; update/rollback/jobs/purge remain later. Default self-service assigned; unsupported offerings fail closed.
- Root owns final formatting/quality, integration review, sign/push and fixture cleanup coordination.

## Review focus

- Signed release/recipe/target identity changes cannot alter admitted work or grant capabilities.
- Revoked operator/agent, expired/stale claim and lost callback cannot promote or reexecute unsafe mutations.
- Installation and retained storage ownership survive crashes without replacement credentials or cross-installation resource adoption.
- Probe-only candidates never gain invocation/workload authority before exact authenticated promotion.
- Private configuration, TLS/storage secrets and connection URLs never leak through snapshots, logs, checkpoints or public configuration.

## Task1: release, admission and persisted installation authority

Owned: plugin-host catalog/installation modules and migrations0042+, metadata binding source, focused restricted tests. Root owns HTTP/config/runtime client integration.

- [x] Map strict public/worker/agent interfaces with root before edits; reserve migration slots.
- [x] RED→GREEN release JWS/byte/identity/bounds/duplicate tests with a shared signed golden.
- [x] RED→GREEN current operator-only initial install/retry, exact idempotency, approved target/feature ceiling and immutable provenance.
- [x] Persist installation-owned spec and encrypted material references; implement short claim/guard/checkpoint/complete/promotion methods and managed binding lookup.
- [x] Verify candidate has no ordinary authority; stale/changed/revoked operations cannot complete. Preserve existing state controller and snapshots.

## Task2: installation-owned enrolled-agent deployment

Owned: agent-protocol DTO/proto, agent-control service, agent daemon/outbox/executor and reviewed generic runtime chart/profile. Root owns production binary wiring/CLI.

- [x] Settle exact spec/lease/material/checkpoint/completion DTOs with root and Task1.
- [x] RED→GREEN authenticated agent RPC, independent install ownership, token/spec/generation fencing and exact receipt replay.
- [x] Apply/inspect/suspend retained runtime resources with guards around Kubernetes actions; stage private files without weakening file validation.
- [x] Verify real local deployment, mTLS runtime readiness, ownership denial, failures/restart and absence of secrets in durable callbacks.

## Task3: PostgreSQL secure runtime and metadata provisioning

Owned: plugin-kit transport/storage helpers, PostgreSQL runtime entry point and generic private storage provisioner with focused tests. Coordinate provisioning module location/API with root/Task1.

- [x] RED→GREEN optional pinned mTLS listener/callback configuration while retaining Unix setup.
- [x] Add verified PostgreSQL TLS to plugin metadata storage; loopback-only plaintext remains explicit fixture/development behavior.
- [x] Provision one normal role/database with retained deterministic ownership and stable generated credentials; refuse foreign/existing mismatches and invalid storage trust.
- [x] Verify actual standalone runtime, wrong CA/name/pin rejection, migrations and restart/state retention. Keep startup errors redacted.

## Root integration and Console

- [x] Implement trusted catalog/target/private worker configuration, private material/PKI preparation and managed endpoint resolution using the agreed interfaces.
- [x] Add supported native core-only fresh bootstrap and exact-target agent enrollment; wire agent control and separately supervised install controller.
- [x] Add catalog/review/install/operation/retry UI with exact request identity; ordinary/preview/acting users have no installer access.
- [x] Fresh disposable native installation + actual signed image/agent/Console → PostgreSQL resource → SQL write/read and retained runtime restart.
- [x] Resolve independent review findings, run final full quality and record acceptance.

Branch integration uses a signed commit/push on the existing branch. Only owned fixtures are cleaned; the final handoff records integration and cleanup results.

## Integration evidence

- Fresh native CLI bootstrap, first-party MFA/PKCE, signed actual OCI installation, dedicated PostgreSQL 17 SQL write/read and runtime-pod replacement passed on owned disposable services. Metadata rows, sealing key/password and SQL data survived replacement. Screenshots at 375/1280px inspected with credentials hidden.
- Actual Traefik CRD normalization exposed an unsupported TCP service field; the fixed profile removes it and retains strict object/UID checks. Regression: 9 passed, one opt-in fixture ignored.
- Whole-feature review found initial-install availability interference, interface-dependent storage identity, and a Console target-label limit mismatch. All three findings are fixed, and the scoped independent re-review found no new load-bearing regression.
- Full `bun run quality` passed on the final product tree. Fresh restricted catalog/install regressions passed 4/4, and the real dual-interface storage boundary passed 1/1. One intermediate Mail UI timing assertion failed in an earlier full run; standalone and complete confirmation passed without product/test changes. Native browser SQL/retention acceptance passed again after the final API/controller build.
- Sanitized acceptance: `docs/verification/2026-10-23-native-plugin-installation.json`. Operator setup and trust/recovery limits: `docs/operations/native-plugin-installation.md`. No production or real-mail action.
