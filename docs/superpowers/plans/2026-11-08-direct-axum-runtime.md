# Direct Axum Runtime Implementation Plan

> Execute inline with superpowers:executing-plans. The owner explicitly requested execution; no additional planning menus.

**Goal:** Remove Loco from native Core startup while preserving request and database behavior.

**Architecture:** Native validated configuration and compact startup errors initialize the existing pools/services and Axum Router. Tokio owns signals and bounded graceful shutdown. Later increments replace service persistence with tokio-postgres.

**Tech Stack:** Existing Axum 0.8, Tokio, tracing, current restricted-role persistence.

**Spec:** `docs/superpowers/specs/2026-11-08-native-performance-runtime-design.md`

## Global constraints

- Keep env names, TLS policy, role checks, bootstrap pool 1–10 and existing 5-second connection, 30-second acquisition and 30-second idle limits.
- No migration, schema mutation, seeding, new database polling or credential changes.
- Preserve Native-Git dirty files and TODO instructions exactly. Build only isolated release sources.
- No claim of completed tokio-postgres conversion or higher throughput in this increment.

## Review focus

- A held SSE/HTTP request must not hold shutdown indefinitely.
- SIGTERM must be registered before listener activation and must drain ordinary requests.
- No connection string or secret may enter startup error/debug output.
- Native startup must use the same role and exact router without framework middleware changes.
- Offline migrations and account bootstrap must remain explicit and unaffected.

### Task 1: Native lifecycle and configuration

Files: new `crates/control-plane/src/runtime.rs`, native config in `config.rs`, startup tests in `tests/native_runtime.rs` and existing `tests/http.rs`.

- [ ] Add failing tests for native config target/bind/pool policy, secret-safe errors, loopback HTTP delivery and graceful/bounded shutdown.
- [ ] Implement compact runtime errors/config and a native Axum serving helper with injectable shutdown for owned tests.
- [ ] Run focused tests and compare the old config defaults and source authority tests.

### Task 2: Remove Loco boot

Files: `app.rs`, `main.rs`, `db.rs`, `lib.rs`, root/Core Cargo manifests and lock.

- [ ] Extract existing service initialization into native `build_runtime`, retaining service order, restricted pools and canonical config.
- [ ] Replace main with direct Axum/Tokio startup and JSON tracing. Keep graceful shutdown bounded.
- [ ] Remove framework-only hooks/types/dependencies; retain native offline migration tools and SQL-backed services.
- [ ] Run HTTP, probes, config, all-target lint and owned PostgreSQL actual-binary readiness/termination tests.

### Task 3: Qualify and publish

- [ ] Capture dependency removal and startup/runtime evidence, with no performance promise absent load results.
- [ ] Obtain one fresh implementation review; fix material findings and rerun relevant checks.
- [ ] Commit only this increment with a verified signature, create a Blacksmith PR, then promote only a qualified immutable artifact with rollback retained.
