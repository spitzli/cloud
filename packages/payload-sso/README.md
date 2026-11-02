# Payload SSO adapter

Authorization Code + PKCE/OIDC via openid-client, using encrypted, host-only browser cookies. Each protected request introspects the opaque token, checks its client binding, and intersects its current central role with the local projection. Local users are matched only by immutable `authSubject`. No email-based linking or user creation occurs during login.

`configurePayloadSSO` returns the strategy, login/callback/logout/refresh handlers and Payload hooks. Add `authSubjectField` and the hooks to the Users collection. Success clears any previous native Payload cookie. Refresh never mints a native JWT or extends the original eight-hour expiry. Logout clears local credentials even when the provider is unavailable. Studio enables `centralLogout`: its browser form continues to the registered OIDC end-session endpoint, terminating that browser's central session while other devices remain signed in. Existing/expired local cookies without an ID-token hint use the provider's browser logout confirmation. Enable this option only with a registered post-logout URL. Global sign-out across devices remains available at the central account page.

Production requires HTTPS, __Host- cookies and an exact registered app origin/callback. Gated localhost HTTP exists only for development and is rejected in production. Callback URLs are reconstructed from trusted configuration to tolerate Next.js internal proxy URLs; raw Host or request URL host must match, and forwarded-host claims are not trusted.

Native Payload authentication must be disabled for an SSO-only cutover. `disableLocalStrategy: {enableFields:true, optionalPassword:true}` preserves the existing schema but rejects old passwords, recovery and JWTs. An explicit temporary compatibility flag on old sites is rollout support, not central-policy enforcement.

Enforced native admin entry points use `adminSSORedirect` to send login/recovery requests straight to the SSO endpoint and keep local user/account management out of the UI. `returnTo` accepts only normalized `/admin` paths, rejects external destinations and authentication routes, and travels inside the encrypted login flow. Callback query parameters cannot change it. `protectUsers` hides projections and denies normal create/update/delete/unlock access while enforcement is enabled; trusted server-side operations may still use `overrideAccess`.

Run `bun run test:sso` from the monorepo root. External website repositories vendor this file and carry the same protocol and actual-Payload policy tests. Keep them synchronized deliberately.
