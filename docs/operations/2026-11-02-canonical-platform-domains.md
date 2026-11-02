# Canonical Spitzli platform domains

The owner corrected navigation on 2026-11-02: new Spitzli entry points must not
send people back to Webdock. Account UI now stays at `https://auth.spitzli.cloud`.
Old account, Studio, website, registry and monitoring page URLs forward to the
matching Spitzli service. The old Docs DNS record still requires an interactive
Vercel confirmation; its replacement handler is prepared. New public docs are
already live at `https://docs.spitzli.cloud`.

## Public origins and protocol identities

`WEBDOCK_AUTH_ORIGIN` is the canonical account origin. During this migration,
`WEBDOCK_PASSKEY_RP_ORIGIN=https://auth.webdock.dev` and
`WEBDOCK_OAUTH_ISSUER_ORIGIN=https://auth.webdock.dev` explicitly retain the
existing WebAuthn RP and OAuth issuer (`/api/auth`). Both settings default to the
canonical Auth origin outside a migration. Startup validates exact origins and
production TLS; it does not accept wildcard origins or change keys.

Identity builds its WebAuthn service with the original RP and exactly the old RP
and new account origins. Password reset links, cookies and Managed Auth requests
use the new account origin. The public old-RP `/.well-known/webauthn` document
allows only `https://auth.spitzli.cloud`, with credential-free JSON/CORS.
Existing cross-site passkeys require browser Related Origin Requests support;
a hardware ceremony has not been qualified here. Password and MFA are retained.
Do not delete the old RP domain or change every stored protocol URL globally.
A future RP retirement needs controlled new-key enrollment, not a credential reset.

OAuth and the first-party browser session verifier use the same original issuer.
Existing client IDs, signing keys, resource identifiers, tenant permissions and
registered callbacks are preserved. Old authorization page navigation forwards
to new Auth, including its original raw query. Credential-bearing protocol
requests and discovery documents remain available for existing clients.
Private API/dispatcher Host contracts retain their existing technical API origin;
`api.spitzli.cloud` remains the public alias, and old API root navigation forwards
there. Versioned/private API paths are not redirected across origins.

## Registry

The same deployed registry image, keys, database and volumes remain in use.
Only `WEB_BASE_URL=https://registry.spitzli.cloud` was added through the normal
Core Hosting API; `BASE_URL` stays at the existing registry protocol authority.
The existing RUN ingress/certificate handles legacy navigation using the scoped
`registry-canonical-navigation.yaml` Middleware/IngressRoute. DNS and TLS keys
were not moved. `registry-proxy.Caddyfile` serves the new hostname through a fixed
verified HTTPS upstream. It never rewrites the incoming Origin.

`X-Spitzli-Registry-Proxy: canonical` prevents a proxy redirect loop. It is a
routing hint, never authentication; forging it can only opt out of a navigation
redirect. Registry permission checks still apply. API, MCP, discovery, OpenAPI,
health, authenticated `/auth/me` and machine asset/download paths are excluded.
Provider start and callback navigation forwards to the new host before app
processing. This preserves the new host-only state cookie while the provider's
original registered redirect URI is still used for the PKCE/code exchange.
An authentication flow started on the old origin may need to be restarted.

## Backend delivery and rollback

The production backend is a minimal three-file patch on its already deployed
source baseline, not an unrelated main upgrade. All production SQL and other
application source were byte-identical; no production migration was run.
The immutable backend image is
`webdock.local/core/backend@sha256:2734a7491c6375839dd65e6428acf4a4d0364c6757452a519716ac2410827c25`,
source checkpoint `3342b745d5d7eefbca9be2b16b0b87ef12717e07`.
The original backend image and private rollback snapshots remain on Core.
HTTP Core and its matching initialization image were updated; the private
agent-control workload and unrelated secret bytes were preserved.

Anonymous bootstrap, exact public runtime configuration, preserved RP metadata
and real existing-operator password/MFA/PKCE were checked. Focused Rust tests
and Clippy passed on EU BUILD; gateway regression tests passed locally. This is
not a claim of real hardware-passkey or arbitrary public build isolation readiness.
The source/publication and live infrastructure evidence is retained in the
ignored dated canonical-auth working directory, without credentials in Git.
