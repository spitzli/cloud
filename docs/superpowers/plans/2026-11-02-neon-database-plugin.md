# Neon database plugin implementation plan

> **For agentic workers:** Use superpowers:subagent-driven-development or superpowers:executing-plans to implement the tasks with independent review and verification.

**Goal:** Publish and integrate a real database-first Neon plugin, keeping tenant metadata and secrets in Core.
**Architecture:** Independent native v2 runtime plus optional Core named HTTP/credential adapter and protected existing-database attachments. Retain the working customer Connections API.
**Tech Stack:** Existing Rust, PostgreSQL forced RLS, native plugin SDK/gRPC, Console declarative UI, BuildKit, Blacksmith, signed Tabularium catalog.
**Spec:** `docs/superpowers/specs/2026-11-02-neon-database-plugin-design.md`.

## Global constraints

- Retain Core-owned encrypted API keys, MFA/passkeys, original migrations and restricted runtime roles.
- No new metadata service, paid resource creation, provider deletion, SQL execution or placeholder capability claims.
- No new GH-hosted/persistent runner fallback; existing EU build infrastructure for manual native builds.
- Preserve unrelated staged/untracked work; select exact paths for any authorized commits.
- No production secret in source, build context or test output.

## Review focus

- A resource ID, project ID or callback URL supplied by a plugin never replaces authenticated Core scope.
- Key replacement/disconnect, grant/session revocation and tuple changes during IO fail closed.
- URI/password fields never appear in inventory, persisted receipts or logs.
- Lost attach/detach replies and retried requests do not create unbounded attachments or delete provider data.
- Current registry presentation is checked publicly; a Git commit is not publication.

### Task 1: Catalog repair

- [x] Inventory live and exact-source manifests, URLs and release signatures.
- [x] Refresh nine remaining entries at immutable commits using normal authenticated admin API.
- [x] Back up and delete the five owner-selected old entries only.
- [x] Verify all public assets/README, retained release identities and temporary credential revocation.

### Task 2: Core adapter and bindings

Files: `crates/plugin-host/src/{http,authority,session_authority}.rs`, new focused adapter module; `crates/provider-connections/src/` focused database/binding modules and additive migration if existing protected storage cannot hold the binding; `crates/control-plane/src/{app,plugins,neon_connections}.rs`; focused unit and restricted PostgreSQL tests.

- [ ] Define adapter request/response and authoritative action/resource context; publish the contract for the plugin task.
- [ ] Write failing validation, authority, URI-redaction and binding replay/race tests.
- [ ] Implement fixed-origin scoped broker, additive RLS attachment storage and before/after fences.
- [ ] Compose the adapter without replacing the existing Connections endpoint or secret encryption purposes.
- [ ] Run exact affected Rust tests/Clippy and restricted PostgreSQL fixture on EU BUILD; review all authority boundaries.

### Task 3: Independent Neon plugin

Files: new `../cloud-plugin-neon/` native v2 package, manifest, action/runtime code, provider fixtures, OCI release scripts, Blacksmith workflows, README/icon/screenshots. Compatibility snapshot/UI references in root only where required.

- [ ] Implement database browse/attach/view/branches/reveal/detach against the Core adapter contract; declare only actual capabilities.
- [ ] Match existing standalone SDK/release/CI conventions, exact pins and buildable lockfile.
- [ ] Verify manifest/action schemas and safe malformed/secret-bearing provider responses with fake transport.
- [ ] Qualify actual standalone build, image/runtime and Core child flow; capture real synthetic UI screens.
- [ ] Create/reuse the explicitly authorized Spitzli repository, publish signed source and immutable image/release, then admit to the correct registry with complete presentation.

### Task 4: Integrate and release

- [ ] Verify current Core/public Console and all tenant/revocation/retry tests with the real native plugin.
- [ ] Apply reviewed additive migration through existing tooling; targeted Core rollout with preserved keys/rollback.
- [ ] Enroll exact plugin runtime generation and explicit grants; verify customer UI/public catalog and protected URI reveal.
- [ ] Update developer/user docs with current support, setup, limitations and future capability boundary.
- [ ] Commit exact reviewed source paths to dev, PR main, verify real Blacksmith CI and merge under the existing owner release authorization.

## Preflight rulings

- Ruling: use the persisted admission-derived adapter context defined in the spec; bare InvocationScope is not sufficient — prevents a runtime from changing resource/action/authority — costs a focused host context extension.
- Ruling: attach commits Core binding/index/public receipt/reservation in one transaction before returning the callback, generic recording is idempotent; detach has current-authority receipt replay before missing-resource gate — prevents lost-reply dead ends — costs narrow Core lifecycle integration and restricted PostgreSQL tests.
- Ruling: existing `PluginUI.Create` and feature-gated `databases.discover` provide selectors before resource creation; no new page contract/SDK wire required — avoids forcing customer opaque IDs — costs one first-party UI snapshot tied to the independent repo.
- Ruling: exact write action + `plugin.http.request` gates URI reveal even though the Neon request is GET — prevents read-capability secret access — costs no new public permission kind.

## Existing permission boundary confirmed during qualification

Migration 0071 already restricts provider connection reads to database managers
or an authorized read-only customer preview. The first plugin release preserves
that policy. Ordinary membership alone does not grant Neon actions. No broader
provider-key RLS policy is introduced to make a fixture pass. Workspace owners,
administrators and authorized operators retain management; authorized read-only
preview retains its existing safe settings view and cannot reveal credentials.
