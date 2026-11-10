# Selectable browser authentication

The browser provider is an installation choice, parsed by `@spitzli/auth-configuration` from `SPITZLI_AUTH_PROVIDER`. Absence preserves Better Auth. A configured object with issuer, authorized parties and audience defaults to Clerk when provider is omitted; explicit Clerk configuration selects exactly one browser entrypoint and requires an issuer and authorized-party list. Existing configurations require an audience; explicit `tokenMode: "session"` accepts native Clerk session tokens without an audience and rejects a simultaneous audience setting. New installation manifests should select Clerk explicitly. This change does not switch existing deployments or enroll their accounts.

## Trust boundaries

The Console runs the official Clerk Next.js middleware only in Clerk mode. Its server-only middleware uses `CLERK_SECRET_KEY`; this is a narrow, documented exception to the frontend credential prohibition. The key is never supplied to `ClerkProvider` or a client component. The public key is supplied by the server (`CLERK_PUBLISHABLE_KEY`, with the usual `NEXT_PUBLIC_CLERK_PUBLISHABLE_KEY` fallback), so an artifact built in legacy mode can be configured at runtime. The middleware establishes SDK request context, and server components/actions call `auth().getToken()`. Browser tokens go only to the fixed configured API `/v1/studio` origin, with redirects disabled and no legacy session cookie alongside them.

The API verifies the signed token with the official Clerk backend SDK and independently checks exact issuer, configured audience (unless native session mode is explicitly selected), authorized party, validity times and absence of impersonation. It also reads the live provider user and session. Unavailable verification fails closed. The API then requires an explicit `(issuer, external subject) → local user ID` mapping. Email addresses never establish identity. This mapping can be enrolled offline but cannot be reassigned or revived by the enrollment tool.

For native services that still authorize through local sessions, the API creates a local session projection, bound to the verified external session, with a lease no longer than 60 seconds or the external proof expiry. The signed native cookie remains inside the API. A request-local context allows native policies and existing PostgreSQL RLS checks to consume it. A database trigger caps native session refresh to the verified lease, preventing Better Auth's normal refresh from extending external authority. The bridge never sets a browser cookie or returns the native token in its safe DTO.

The local user, roles, bans, setup flags, memberships, tenant preview and resource permissions remain authoritative. Clerk email verification does not override an unverified local account. Operators require completed local security setup, live external MFA enrollment and signed evidence that a second factor was verified (`fva[1] >= 0`). Enrollment alone is insufficient. Mail's injected identity directory rechecks live external identity/session state for Clerk mappings; the separately authenticated legacy protocol scope retains its original local directory.

## Legacy isolation and first operator

The old opaque Studio browser cookie is not accepted by the Clerk-selected Studio endpoint. The native cookie-only Studio endpoint is unavailable in Clerk mode. OAuth, MCP, service credentials and native Auth setup remain separate contracts; the provider selector does not reinterpret their tokens as Clerk browser tokens. The legacy delegated path establishes its own trusted request scope only after its existing confidential-client introspection.

An initial local operator created by the offline bootstrap has no completed MFA and cannot gain operator access merely by being mapped to Clerk. Complete native account setup (including a real second factor) through the retained Auth setup service, then enroll that immutable local ID against the verified Clerk subject and complete Clerk MFA. This deliberately requires both setup facts. Removing the native setup prerequisite would require an audited distinction between native and external MFA facts throughout legacy authorization and RLS; this implementation does not fake `twoFactorEnabled` to avoid that work.

## Migration and operation

Migration `0040_clerk_identity_mapping.sql` adds two bootstrap-owned mapping tables with purpose-limited worker reads and the local-session expiry clamp. Apply it through the normal migration runner before enabling Clerk. The API validates browser configuration at startup. A configured Clerk Console requires its runtime keys; legacy builds require none. API/Console configurations and the Clerk session-token `aud` claim must match. A default Clerk token without `aud` is intentionally rejected; configure the session token claim in the selected Clerk application.

`apps/control-plane/scripts/clerk-map.ts manifest.json` validates and previews a single explicit mapping. Add `--apply` to enroll it through `AUTH_DATABASE_URL`. The tool creates no provider user, local user, credential, membership, role or MFA fact. Revoking an enrollment is an operator database change setting `revoked_at`; do not repoint it to another local user. Subsequent browser verification fails, and projected sessions have at most their short remaining lease. The selected Mail identity adapter also rechecks mapping and provider state.

Offline tests cover rejected external proof, absent mapping, stale/rebound sessions, incomplete local setup, missing second-factor evidence, bounded leases, safe browser DTOs, API token transport and client/server import separation. Adapter fixtures separately cover wrong issuer/audience, expiry and live provider revocation. No real Clerk application, account, key, session or production migration was created as part of this implementation. Live sign-in still requires the operator-supplied application keys, configured claims and explicit enrollment.

## Queued Mail and Hosting

Mail and Hosting workers configured for Clerk require the backend verification key and the same provider configuration. Startup validates this before opening the worker pool. Mapping reads bind to `workerDatabase`, never bootstrap credentials. Mail purposes (`mail-cron`, `mail-send-gateway`, `mail-webmail-gateway`) and the authenticated `git-worker` lane can read identity mappings for live user rechecks. All enrollment writes remain unavailable to workers.

Session mappings require a narrower fence: a live deploying Git release with the matching approval subject/session, the exact actor of a current confirmed Hosting setup lease/generation, or the exact subject/session placed into a `database-order` scope after user admission. Mail purposes, general agent/enrollment, scheduler and setup-dispatch contexts do not gain session-mapping access. Native session token columns remain unreadable. The database-order scope includes actor references because accepted setup work revalidates identity inside its worker transaction; it does not receive arbitrary mappings for other actors.

Mail queued authorization still uses the subject lookup and local grant checks without requiring the original browser session to remain active. Hosting retains its existing session and durable Git-policy rules. Missing verification credentials, mapping authority or current proof fail closed; there is no fallback to a local-only identity directory.

## OAuth callback refresh

Registered GitHub/Vercel callback URLs remain unchanged. If a selected-Clerk GET callback has no currently valid identity proof, the API seals its exact allowlisted callback path and query into a three-minute encrypted, host-only HttpOnly cookie and redirects to the query-free Console `/auth/integration-resume` route. OAuth codes and state do not appear in the Clerk handshake URL. The Console's official middleware refreshes the Clerk session; its route sends the fresh bearer and allowlisted integration cookies to the fixed API `/v1/integrations/resume` endpoint. The API checks origin, bearer, authenticated ciphertext, expiry, module admission and live identity before running the original callback handler. The handler still validates its existing signed flow state.

The resume dispatch disables further refresh and clears the handoff cookie on success or failure. Missing mappings and revoked sessions therefore stop after one application refresh attempt. The Console only forwards a successful redirect to its configured local origin and strips authentication cookies from callback responses. Responses use `no-store` and `no-referrer`. Tests cover exact callback allowlists, ciphertext tampering/expiry, deferred provider side effects, fresh-proof transport, external redirect rejection and loop prevention. Real provider/Clerk handshake execution remains unverified without operator-supplied live keys; no external account or provider call was made during these tests.

## Core-only protocol selection

`SPITZLI_AUTH_PROVIDER` selects browser identity. With Clerk selected,
`SPITZLI_OAUTH_PROVIDER` defaults to `disabled`: core Console requests use verified
Clerk principals and local membership/RLS validation without constructing Better
Auth or requiring `BETTER_AUTH_SECRET`. The older `/v1/studio` compatibility bridge,
OAuth client management, and OAuth introspection require the OAuth capability.

Existing Better Auth installations keep their bundled browser and OAuth runtime.
To retain the existing issuer, audience, opaque-token introspection, revocation,
and MCP clients during a Clerk rollout, explicitly set
`SPITZLI_OAUTH_PROVIDER=better-auth`. This compatibility mode still requires the
Better Auth secret and existing OAuth provisioning; it is not a second independent
Clerk OAuth implementation. MCP startup fails if OAuth is disabled. Clerk JWTs
are never treated as the existing OAuth tokens. Supporting direct Clerk MCP later
requires an explicit audience/scope and current membership/revocation contract.

The delegated `/v1/operator/registry` and OAuth-protected exposed plugin HTTP APIs
return 404 when OAuth is disabled. Native Console registry/plugin operations and
isolated plugin data transports retain their own authorization boundaries.

Selected Clerk Console traffic uses `/v1/console` with a server-only
`SPITZLI_CONSOLE_SERVICE_KEY`, the configured Clerk audience and authorized
parties, and live session/membership verification. Provision the existing
`webdock_auth.clerk_identity` and `clerk_organization` mappings before cutover.
Older adapter mapping tables are not automatically reinterpreted or migrated;
unmapped authenticated users receive a provisioning instruction and no access.
