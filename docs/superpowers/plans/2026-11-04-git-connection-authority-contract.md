# Git connection authority contract implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the independently testable signed authority contract needed before native customer Git onboarding can acquire provider access.

**Architecture:** Extend the existing repository-protocol crate with a separate connection-purpose module. Keep source leases, generated RPC services and running providers unchanged. This increment supplies cryptographic and structural validation; durable replay protection and live session admission belong to the subsequent Core increment.

**Tech stack:** Existing Rust, Serde, Ed25519, SHA-256 and UUID dependencies.

**Spec:** [Native customer Git connection](../specs/2026-11-04-native-git-connection-design.md).

## Global constraints

- No fabricated project/source IDs or optional fields in existing source leases.
- Browser authority remains private to Core; a signed connection token is not proof of a live session or unused operation.
- No provider code/token/PKCE verifier, private configuration or `StoredAuthority` in these claims.
- No production activation, credentials, database migration, RPC endpoint or public build admission in this increment.
- Preserve unrelated staged documentation and continue on the owner's authorized `dev` branch.

## Review focus

- A connection token must fail source-lease, descriptor and ordinary plugin-invocation verification, and source tokens must fail connection verification.
- Changing any customer, actor, session, flow, operation, action, plugin, generation or digest must deny use.
- Expired, future-issued, overlong, malformed and overflowing claims must fail without panic.
- Unknown JSON fields, malformed identifiers and invalid digests must fail rather than silently widening the contract.
- Successful signature verification must never be documented as replay protection, current authorization or a working customer onboarding flow.

## Task 1: Purpose-separated signed connection authority

**Files:**
- Create `crates/repository-protocol/src/connection.rs`: connection action, scope, claims, signing and verification.
- Modify `crates/repository-protocol/src/lib.rs`: export the module only.
- Create `crates/repository-protocol/tests/connection.rs`: positive and adversarial contract coverage.
- Modify `TODO.md`: link this increment without closing native onboarding.

**Interfaces:**
- `ConnectionAction`: `Begin`, `Exchange`, `Options`, `Finish`, `Abort` (snake-case JSON).
- `ConnectionScope`: `flow_id`, `operation_id` (canonical RFC4122 UUIDv4), `customer_id`, `subject` (native positive decimal record IDs), `session_id` (nonempty visible ASCII, at most 256 bytes), `plugin_id` (existing slug rules), `generation` (1 through 9007199254740991), `action`, `descriptor_sha256`, `request_sha256`, `configuration_sha256` (existing lowercase SHA-256 format). IDs serialize as `flowID`, `operationID`, `customerID`, `sessionID`, `pluginID`.
- `ConnectionClaims { v, purpose, audience, scope, iat, exp }`, all structs deny unknown fields. Version1, purpose `repository-connection`, audience `webdock.repository.connection.runtime.v1`, distinct signature domain `webdock.repository.connection.v1\0`, lifetime1 through60 seconds, checked arithmetic, token limit16384 bytes.
- `ConnectionClaims::sign(&SigningKey) -> Result<String>` validates at issue time.
- `verify_connection(token: &str, key: &VerifyingKey, expected: &ConnectionScope, now: i64) -> Result<ConnectionClaims>` verifies signature, structural/time validity and **every** expected scope field. It does not accept caller-supplied scope as live authority; admission must construct expectations from its stored flow.

- [x] Write round-trip tests for all five actions, purpose separation in both directions, wrong issuer, tampering and every mismatched scope field.
- [x] Add malformed/unknown-field, time/overflow and boundary tests; run `cargo test -p spitzli-repository-protocol --test connection --locked` and observe missing-module failure before implementation.
- [x] Implement the module using existing signing, digest, record-ID and slug helpers. Do not refactor or weaken existing leases.
- [x] Run `cargo test -p spitzli-repository-protocol --locked`, package Clippy with warnings denied, targeted Rustfmt and `git diff --check`; all must pass.
- [x] Obtain independent read-only review, fix material findings and rerun affected checks.
- [ ] Commit only owned paths, push the authorized `dev` branch, create/link a normal PR and require Blacksmith checks before merge.

## Subsequent increment boundaries

This plan deliberately ends at the signed contract. The design brief still requires a separate customer/session Host admission policy; private `StoredAuthority` verification; additive native-flow and operation-receipt storage with restricted-role RLS tests; committed in-flight/uncertain exchange handling; fake pinned-runtime transport tests; provider OAuth ownership checks; UI/callback integration; atomic repository replacement and revocation. None is marked complete by contract tests.
