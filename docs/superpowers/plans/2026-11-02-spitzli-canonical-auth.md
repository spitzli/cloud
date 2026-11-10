# Canonical Spitzli account navigation implementation plan

> **For agentic workers:** Use superpowers:executing-plans to implement this plan task by task.

**Goal:** Serve the account UI at auth.spitzli.cloud and redirect legacy platform page navigation toward Spitzli, while preserving retained authentication material.

**Architecture:** Separate public account origin from retained OAuth issuer and WebAuthn RP through two explicit startup-only compatibility settings. The existing canonical account configuration changes to Spitzli; cryptographic issuer/RP stay pinned to their original values. Keep legacy protocol endpoints available; redirect human navigation, including authorization entry, toward the new account origin. Do not rewrite incoming Origin headers or loosen signature, CSRF, UV, role or tenant checks.

**Tech stack:** Existing Rust Core/Identity, webauthn-rs0.5.5, existing Caddy gateway and static Vercel Build Output v3. Builds run on the existing qualified EU BUILD host. No new provider, paid resource or credential rotation.

**Spec:** Owner correction on2026-11-02: auth must stay at auth.spitzli.cloud; Webdock platform navigation must lead to Spitzli domains. Existing accepted rebrand design remains the baseline for customer URL retention.

## Constraints and review focus

- Keep the current OAuth issuer, signing keys, token audience and registered clients valid; do not globally replace signed identifiers.
- Keep WebAuthn RP auth.webdock.dev and its credentials. Permit exactly the configured new account origin as an additional verified origin; expose a credential-free /.well-known/webauthn related-origin document at the old RP. Browser Related Origin Requests support is required for cross-site use; password+MFA remains available.
- New account origin controls request Origin validation, session host-only cookies, reset links and Managed Neon Auth frontend origin. Preserve MFA, ownership/admission and database data.
- Legacy API/discovery callbacks are protocol compatibility, not ordinary page redirects. Preserve methods, state/query encoding and PKCE behavior; never redirect credential-bearing POSTs arbitrarily.
- Inspect all platform old/new navigation roots; keep tenant application domains unchanged. No repeat PC shutdown.

## Task1: Identity compatibility configuration

- [x] Add tests for default versus explicitly retained RP/issuer origins and invalid production origins.
- [x] Add startup parsing for SPITZLI_PASSKEY_RP_ORIGIN and SPITZLI_OAUTH_ISSUER_ORIGIN, defaulting to canonical auth origin.
- [x] Add IdentityService::with_passkeys_for_account(account_origin,rp_origin); old with_passkeys behavior remains unchanged. Test exact allowed origins, retained rp.id and foreign-origin exclusion without a database.
- [x] Wire Core account/reset origin to new canonical Auth and OAuth/BrowserConfig issuer to the retained explicit issuer.
- [x] Execute relevant real Rust tests/Clippy on EU BUILD; no native compilation on the desktop.

## Task2: Reverse public navigation safely

- [x] Add routing tests for explicit legacy protocol compatibility aliases, new canonical account UI and old OAuth authorize navigation; default navigation-only alias denial stays unchanged.
- [x] Extend Build Output renderer with a bounded explicit compatibility-alias input and GET/HEAD-only page redirects. Keep existing API proxy, Origin preservation and metadata contracts.
- [x] Prepare new Auth/Console static outputs from the already-qualified static bundle with updated canonical routing/CSP and public related-origin metadata. Test candidates before promotion.

## Task3: Controlled live cutover

- [x] Snapshot Core identities/images/public origin values, exact gateway configuration, Vercel targets and public metadata; never print secrets.
- [ ] Qualify a matching backend candidate with current retained data/schema through a disposable fixture. Do not apply an unrelated production migration merely to make startup pass.
- [x] Activate new canonical Auth origin plus explicit old RP/issuer settings; preserve technical API Host contracts, unrelated secret bytes and private agent-control forwarding.
- [x] Update the existing gateway's account Host and TLS routes with backup/rollback; verify Managed Auth already trusts the new account origin.
- [x] Promote only reviewed static artifacts. Verify new Auth has no Webdock navigation redirect, old Auth pages lead to new Auth, config/cookies/anonymous login/Origin denials and public RP/OAuth/MCP metadata stay correct.
- [x] Reverse remaining platform navigation aliases (website, docs, registry, monitoring) with provider-specific scope checks; preserve tenant routes and protocol authority.

## Execution rulings

- Keep technical API/dispatcher Host bindings and legacy Studio MCP resource unchanged; existing public API alias and page-only redirects provide canonical navigation without breaking signed clients.
- Backport only the three-file Auth patch to the exact production source (all SQL and unrelated code unchanged). No unrelated main migration is required. HTTP image and canonical origin activation passed actual production readiness checks with rollback snapshots.
- Registry canonical navigation uses the existing RUN ingress and certificate plus an exact owned Core proxy. No registry DNS or TLS-key transfer is needed. The superseding DNS confirmation script now contains only the old Docs CNAME.
- Real existing-operator password, TOTP MFA and PKCE passed on the new account origin. A real browser/hardware passkey ceremony remains untested; related-origin metadata, exact RP and required user-verification options were checked live.
- After the owner unlocked 1Password, the runtime mirror was updated by changing only the three public Auth/RP/issuer settings; exact readback verified all credentials and unrelated fields were preserved.
- The owner interactively confirmed the old Docs DNS change; strict TLS and the forward redirect with preserved path/query passed. Registry DNS stayed unchanged.
- Exact production image acceptance used a staged live rollout with original origins first, then the canonical origin. Readiness, retained configuration and the existing operator were checked without resetting any production data. No separate disposable production-backport database fixture or hardware ceremony was run.
