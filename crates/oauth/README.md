# OAuth/OIDC and MCP identity service

This private crate runs OAuth authorization-code, access-token and refresh state machines through `oxide-auth`. Request-local primitive adapters receive transaction-locked PostgreSQL grants; they are not in-memory token storage. SeaORM owns persistence and all database transactions. `josekit` supplies maintained RS256/JWK signing and verification; the independent `oauth2` client helper is used only by PKCE tests.

Runtime construction:

```rust,ignore
OAuthService::new(auth_database, identity_service, dedicated_id_allocator, encryption_keys, config)
```

The authentication database must use `webdock_auth_bootstrap`. `IdentityService::authenticate_session_in` is a trusted-backend helper, not an HTTP authentication mechanism: public subject/session IDs alone are never proof. Authorization uses a verified cookie; exchange/refresh/introspection use verified server grants or bearer credentials. Current session, setup, ban, external-mapping, preview, role, tenant membership, explicit site grant and retained CMS entitlement checks are applied when needed.

`OAuthConfig::new(issuer, mcp_resource, studio_client_id)` preserves the canonical issuer (`authOrigin/api/auth`) and resource identifier. Configure the first-party Console's client ID as `studio_client_id` so active customer membership produces the existing portal reader claim. Do not change the WebAuthn origin to match a sibling Console host. The Console must use an authorization-code/PKCE bridge through the auth origin.

## HTTP integration

The parent router owns canonical hosts, TLS, CORS, cookie origin, CSRF checks, request/body limits, client/IP rate limits and UI. Parse only one client authentication method per request; reject duplicate credentials/query fields. Map HTTP `resource` occurrences into `AuthorizationRequest.resources`. Never place token responses in browser localStorage or logs.

- `authorize(cookie, AuthorizationRequest)` returns `LoginRequired`, `ConsentRequired { request_id, client_id, client_name, scopes, resources }`, or `Redirect(url)`. The redirect preserves OAuth state. Pending consent is bound to the exact user/session and expires in five minutes.
- `consent(cookie, request_id, approved)` consumes the pending request. Call only from the auth origin with CSRF protection; a bare request ID is not authorization. Persisted existing consent can suppress subsequent prompts.
- `exchange_code(CodeExchange { client, code, redirect_uri, code_verifier })` returns the standard serializable `TokenResponse` with `Bearer`, expiry, scopes, optional refresh token and optional ID token.
- `refresh(RefreshExchange { client, refresh_token, scope })` rotates the token atomically, refuses widening scope, and rechecks the current session. A removed session's NULL foreign key never becomes sessionless authorization. Reuse of a rotated token revokes that client/user token family.
- `introspect(client_authentication, token)` supports retained opaque hashes and RS256 JWT access tokens, returns current claims, and hides unknown/unrelated tokens as inactive. An ID token is never accepted as an access token. Refresh-token introspection is distinguished by `token_type: refresh_token`.
- `introspect_mcp` also checks the resource and issuing client's explicit MCP/Hosting metadata. `authorize_mcp(..., required_scope)` enforces read/write scope pairs and the distinct registry/hosting roles. Each downstream tool must still enforce its current tenant/resource permissions and registry mapping.
- `userinfo(access_token)` emits profile/email fields only under the granted OIDC scopes. `revoke(client_authentication, token)` persists opaque revocation and legacy JWT revocation markers.
- `end_session(cookie, LogoutRequest)` accepts a signed ID-token hint only for the current browser session and an exactly registered logout redirect. Expired signed ID tokens remain usable as logout hints. It returns `Complete { redirect, set_cookie }`, or `ConfirmationRequired` without mutation. Missing/mismatched hint confirmation UI remains parent-owned; after explicit confirmation use the native session logout path, not a forged OAuth hint.
- `discovery()` and `jwks()` expose the canonical metadata/public keys. There is no dynamic registration endpoint. Token/authorization/introspection/userinfo responses require `Cache-Control: no-store`; token responses also require `Pragma: no-cache`. `Error::code()` and `status()` expose safe protocol error mapping.

`prompt=none` uses protocol login/consent errors. `prompt=consent` forces consent. `prompt=login` returns `LoginRequired`; the parent must continue the original request without that prompt only after its own verified interactive sign-in flow. `max_age` uses the persisted authentication time. Rich essential-claim requests, request objects/PAR, pairwise subjects, DPoP and private-key client authentication are not implemented; bound DPoP clients/resources and non-public subject types fail closed.

## Persistence and cutover

Existing quoted `oauthClient`, `oauthClientResource`, `oauthResource`, `oauthAccessToken`, `oauthRefreshToken`, `oauthConsent`, and `jwks` tables are retained. Tokens/client secrets use unpadded base64url SHA-256 hashes. Private JWKs preserve the existing JSON-string-wrapped encrypted envelope. Existing RSA public keys and canonical IDs are retained. New IDs come only from the injected durable allocator.

Apply `migrations/0001_oauth_authorization.sql` offline through the native migration journal, and classify `identity_oauth_request` as authentication-only in policy installation. Never run migrations or key provisioning at ordinary startup. `provision_client(offline_connection, registration)` and `provision_signing_key(offline_connection)` are explicit operator/offline primitives, not DCR or browser APIs.

New access tokens are revocable opaque tokens: ordinary tokens are capped at eight hours; the configured MCP resource is capped at five minutes. Refresh tokens are capped at eight hours and remain session-bound. ID tokens are RS256, five minutes, with issuer/client audience, nonce, authentication time and access-token hash. Existing opaque tokens/refreshes and retained RSA JWT access tokens can be validated. Existing in-flight authorization codes/consent pages use different transient state and must restart at cutover; clients and grants are preserved. Unsupported/custom token-prefix or encryption plugins require a separately tested transition.

The native allocator cutover must also cover the retained `sync_customer_tenant` trigger, which still creates organization IDs through the legacy SQL allocator. This crate does not modify that trigger.

## Verification

`cargo test -p webdock-oauth` checks configuration and a Node-generated legacy token-hash vector. Ignored `retained` and `tenancy` tests require the parent's explicitly owned local `webdock_rls_test` fixture and guarded environment configuration; they never reset its schemas. They create isolated test identities/clients and exercise real MFA, PKCE, consent, signed ID tokens, legacy JWT verification/revocation, token/code expiry, scope denial, refresh replay, logout, NULL-session denial, public clients, cross-tenant denial, explicit-grant precedence and customer archival. The runtime connection is the restricted bootstrap role.

This is not an OpenID conformance certification or a completed browser/Console bridge. Native mail, physical authenticator/browser acceptance, external identity activation and the remaining account/tenant administration remain separate gates.

Trusted first-party facades can use `validate_portal_client_in(tx, credentials, redirect_uri)` and `introspect_portal_in(tx, credentials, token)` when already holding an authentication transaction. They preserve the exact client/resource/session/preview policy of the ordinary wrappers and acquire no additional database connection. They are backend methods, not public introspection exceptions.
