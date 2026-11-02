# First-party browser sessions

`webdock-browser-session` handles the retained Console/Studio `/api/sso/login`, `/api/sso/callback`, and `/api/sso/logout` contract using the native OAuth provider. The browser receives signed opaque host-only cookies. Access, refresh, and ID tokens remain in encrypted authentication-only database rows.

The service uses `openidconnect` 4.0.1 for PKCE/state/nonce generation and RS256 ID-token issuer/audience/expiry/nonce/at-hash verification, `cookie` for signed cookies, and `iron-webcrypto-rs` 0.0.1 solely to read retained Fe26.2 cookies. HMAC verification precedes decryption in that compatibility library; a deterministic JavaScript-generated vector tests the actual wire encoding. No encryption, signing, PKCE or WebAuthn primitive is implemented here.

## Configuration and admission

`BrowserConfig::new(app_origin, issuer, client_id, client_secret)` and optional `with_additional_origins` must use exact configured origins. Each callback is exactly `app_origin + /api/sso/callback`. The provider client must be the explicitly configured first-party Portal client, confidential, enabled, have an enabled binding with no organization, and allow the exact callback plus `openid profile email`. Organization, MCP and operator-only clients cannot become a Console browser session. Startup never creates a client or signing key.

`BrowserSessions::new(auth_bootstrap_pool, identity, oauth, encryption_keys, cookie_secret, config)` takes the actual authentication role. Native migration0011 points to `migrations/0001_browser_sessions.sql`; it is immutable after journal application. The old policy classifier preserves authentication-only access to both sidecars.

New flow cookies last five minutes and bind the exact origin, state, nonce, verifier and validated relative return path. Persisted flows are consumed once. Code exchange and token validation happen server-side. Successful and denied OAuth authorization responses preserve the retained `iss` parameter; the callback validates its exact configured issuer. Browser sessions expire no later than the provider grant, underlying session or eight hours. Every request revalidates client registration, token authority, local identity, central session and the initial role/scope ceiling. Session deletion cascades to browser references. Stored refresh credentials do not extend this browser lifetime automatically.

## Trusted backend API

- `begin(origin, return_to) -> BrowserRedirect { location, set_cookies }`
- `callback(origin, raw_cookie, code, state) -> BrowserRedirect`
- `authenticate(origin, raw_cookie) -> Option<Principal>` for existing backend callers.
- `resolve_authority(origin, raw_cookie) -> Option<BrowserAuthentication { principal, authority }>` for new domain routes.
- `revalidate(&BrowserAuthority) -> BrowserAuthentication` for a server-owned persisted operation/viewer reference.
- `logout(origin, raw_cookie) -> BrowserRedirect` revokes its application access/refresh pair, clears the local browser session and retained `payload-token`, and creates an opaque short-lived central confirmation request.
- `logout_details(canonical_cookie, request)` and `finish_logout(canonical_cookie, request, confirmed)` bind confirmation to the same canonical account/session. No ID token is placed in the confirmation URL.

`BrowserAuthority` contains `reference`, `client_id`, `app_origin`, `expires_at`, `role`, `scopes`, `read_only`. It contains no bearer credential. It may be serialized in server-owned operation state; **never accept it as browser-supplied authentication**. Revalidation reloads the protected row and live token/session, and intersects the saved expiry, role, scopes and read-only restriction. Domains must also match the returned principal to their saved subject/session and recheck their resource-specific grants.

On configured Console/Studio hosts, the facade requires its own browser/retained SSO proof and never substitutes an unrelated native auth cookie. Native auth cookies remain valid on the canonical Auth origin, and the old direct facade remains available when SSO is disabled. Canonical authentication mutations require that exact Auth origin; passkey ceremonies have the same strict boundary.

Core exports `auth::resolve_actor(state, headers) -> Option<AuthenticatedActor { principal, authority }>` with `ActorAuthority::LocalSession` or `FirstPartyBrowser { browser }` and `read_only()`. Hosting uses `execute_with_ceiling`; new database/viewer routes should retain the reference through launch and Inspect. The retained Portal `reader` role admits ordinary customer accounts; live native tenant owner/admin membership still controls tenant permissions. It is not an organization-scoped third-party token. An operator cannot emerge from a narrower stored browser role; a new authorization exchange is required after such a role change.

A first-party-only introspection method permits an existing operator browser session to remain usable during a read-only customer preview and explicitly exit it. It returns a reader ceiling and preview marker. Public OAuth introspection, userinfo, authorization and MCP do not receive this exception. Signed RLS still confines the preview to its selected customer; expired previews remain restrictive.

## Retained cookies

`with_legacy(LegacyCookies::new(password_map, accept_until))` is optional and read-only. The configured deadline may be past (disabled) or at most eight hours ahead. Both the authenticated Fe26.2 envelope expiry and payload expiry must remain valid and bounded, version `~2` is required, and duplicate cookie names are rejected. Old flow cookies are capped at five minutes and still require the original PKCE/state/nonce; code replay is rejected by the provider.

Old session cookies are introspected against the exact same first-party client. Their server references are bounded by the minimum of the original expiry, provider/session expiry and configured transition deadline. No new legacy seal is issued, no email linking occurs, and no transition resets the clock. Runtime configuration uses `WEBDOCK_SSO_LEGACY_ACCEPT_UNTIL` as Unix seconds and the retained `WEBDOCK_SSO_COOKIE_SECRET` under its original password ID1.

## Verification

Unit tests verify return boundaries and the deterministic retained JavaScript seal, tampering, version and expiry. The real restricted fixture verifies PKCE handoff, signed ID-token nonce checks, wrong-origin/state/cookie duplication, replay, live source-session revocation, an actual retained-cookie transition, encrypted logout confirmation, and real operator MFA/read-only preview/exit. It also checks persisted authority ceilings for backend consumers. Literal loopback/test-database guards run before connecting.

The real Playwright workflow is `apps/console/tests/account-browser.mjs`; its fixture and launcher are under `crates/control-plane`. It runs canonical password/TOTP setup, consent, PKCE return, recovery regeneration/single use/replay denial, session revocation, WebAuthn registration/rename/sign-in with Chromium's virtual authenticator, origin rejection, responsive account UI and central logout. This proves actual browser/protocol/database behavior, not a physical authenticator or actual email delivery.


`prune_expired(limit)` provides bounded cleanup for expired flow/session sidecars; login opportunistically prunes a small batch. Browser login construction is rate-limited by Core's maintained governor limiter. Cookie signing secrets and envelope keys must be retained across migration; key rotation and provider client/signing-key registration remain explicit offline operations.

Domain services can share `StoredAuthority::{LocalSession, FirstPartyBrowser { browser }}` and `revalidate_stored_authority(identity, auth_transaction, browser_sessions, subject, session_id, authority)`. This helper accepts only references obtained from server-owned records, locks and validates the canonical session in the supplied authentication transaction, and revalidates first-party client/token/role/scope ceilings. Keep that transaction alive through the domain commit; separately enforce `authority.read_only()` and resource-specific permissions. Database and Mail gateway services use this boundary without constructing a new Principal or exposing a central cookie.

`BrowserSessions::revalidate_in(&DatabaseTransaction, &BrowserAuthority)` uses the supplied authentication transaction for the browser reference, Portal client/token checks and canonical identity. The shared `revalidate_stored_authority` helper calls this variant, so a caller holding the last available pool connection never needs a nested acquisition. The ordinary `revalidate` wrapper opens and commits one transaction. A restricted regression fills a two-connection pool with two existing caller transactions, validates both proofs concurrently, and repeats expiry/client/role/scope denials on the caller transaction.
