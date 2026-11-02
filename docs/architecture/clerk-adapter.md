# Configured Clerk authentication adapter

Source checkpoint: 2026-10-15. Adapter and configuration contracts are implemented. No Clerk application was created, no production identities were moved and no live provider cutover is implied.

`@webdock/auth-configuration` validates explicit authentication configuration. An absent configuration selects the existing Better Auth compatibility path. Newly configured Clerk installations supply `provider: "clerk"`, an exact HTTPS issuer, a nonempty authorized-party origin allowlist and an explicit audience. Unknown fields, malformed JSON, empty audiences and insecure origins fail validation. Secrets are passed separately at server composition. Configure the Clerk session-token `aud` claim to match this audience; a default token without `aud` is rejected rather than silently treated as this application's token.

`@webdock/auth-clerk/sdk` uses pinned `@clerk/backend` 3.24.0 to verify session-token signatures and read current sessions/users. It initializes no provider at import time. JWT verification checks authorized parties and audience; the adapter additionally pins issuer and checks time bounds, external user/session IDs and impersonation. Live provider session status, expiry and subject are re-read on every authentication and session authorization. Revocation, provider outage, missing mappings and mismatched local subjects fail closed. Queued subject authorization rechecks the current provider user and local account policy without depending on the originating browser session.

References: [Clerk verifyToken](https://clerk.com/docs/reference/backend/verify-token), [session claims](https://clerk.com/docs/guides/sessions/session-tokens), [live session lookup](https://clerk.com/docs/reference/backend/sessions/get-session) and [backend session fields](https://clerk.com/docs/reference/backend/types/backend-session), consulted 2026-10-15.

## Local projection contract

The server supplies `ClerkMappings` and an existing local `IdentityDirectory`. Mappings are issuer-scoped, explicit, durable and bidirectional: external user to local subject, external session to local session reference, and reverse lookup inside authorization transactions. A mapping is never inferred from email or client-provided local IDs. Both local subject and local session must match; unknown identities fail closed. The separate enrollment workflow must enforce uniqueness, ownership, expiry and audit. The adapter neither creates mappings nor issues local browser credentials. `verifyExternal()` exposes signature- and live-session-verified external facts before a session mapping exists, allowing the explicitly authorized enrollment bridge to establish a local session for an already mapped local subject. `verify()` additionally requires the subject and session mappings.

The local directory remains authoritative for Webdock role, ban, setup requirement, tenant membership, session lifetime and preview restrictions. Provider banned/locked state adds a denial; verified-email status must pass both local and current provider checks. The local MFA security flag and current provider enrollment must both be true for `twoFactorEnabled`; it is **not** proof of session factor completion. `verify()` exposes only signed `fva` values as `factorVerificationAge`, or null when absent. `-1` stays unverified. Step-up policy must consume these verified facts and never substitute an enrolled factor or a hardcoded MFA flag.

No raw provider token appears in `SessionReference`, the identity directory or verified evidence. Bearer input is explicit. Legacy cookies and product API keys are not reinterpreted as Clerk tokens. The production SDK uses the configured instance backend secret; no caller-controlled backend host or JWKS URL is accepted.

## Composition and rollout gates

The API composition must choose the configured adapter and supply safe database mapping ports. Browser/BFF integration must obtain a Clerk session token and forward it through an explicit authentication transport. The current API-key and OAuth routes must keep their own token contracts. Studio protocol URLs, OAuth issuer and MCP audiences are compatibility contracts and must not be rewritten during provider selection.

Before enabling Clerk for an installation, verify enrollment and unmapping, two local accounts with the same email, revoked sessions, blocked users, missing mappings, signed scope/RLS resolution, MFA step-up, legacy OAuth continuity and browser sign-out. This adapter alone is not a finished frontend cutover. Public login and session-management surfaces require their own wiring and tests; production switching remains a separate authorized rollout.

## Verification

`bun test packages/auth-clerk/src/*.test.ts` covers configuration, signature failures, issuer/audience/party mismatch, expired and future claims, malformed MFA facts, revoked/expired/mismatched sessions, missing mappings, local subject mismatch, provider outage and preservation of local restrictions. Injected tests call no provider and use no real secrets. A network-blocked test signs a local RSA JWT and exercises the real SDK verifier against valid and altered signatures, audience and party. Nine tests pass; `node node_modules/typescript/bin/tsc -p packages/auth-clerk/tsconfig.json` also passes against the pinned package.

Auth is now composed from the trusted `auth-better-auth` and `auth-clerk` plugin entries in `packages/auth-*`. Use an explicit `provider` in new configuration. Older Clerk JSON without that key remains supported for installation compatibility; missing whole configuration retains Better Auth. Provider SDK objects remain outside core.
