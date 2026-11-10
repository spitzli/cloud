# CMS module boundary

`modules/cms` owns the existing CMS contribution metadata and the customer CMS bridge orchestration. The manifest remains the installed `cms` product. `./ui` exposes the existing component labels, role-sensitive modules, canonical site-hub paths and name formatting. `apps/console/src/lib/cms-modules.ts` is a compatibility export; existing editors and `packages/cms-ui` are reused without moving or renaming application directories.

The server factories own bridge authorization policy, demo requests/calendar, website content, canvas operations, shop access and dispatch. Explicit ports supply live website grants, the verified request's local user/preview state, server bridge configuration, installed-product access and HTTP fetch. API `cms-bridge` files compose those ports through the existing authenticated portal request context. The module imports no application internals or provider implementations, creates no mutable global service locator, and never receives native cookies or delegated OAuth tokens as provider credentials.

## Runtime origins and security

`SPITZLI_DEMO_BRIDGES` remains the server-side JSON map keyed by Auth website binding ID, with each entry's exact origin, private bridge secret and existing optional kind/canvas/promotions/shop capabilities. It is not keyed by registry CMS-instance ID. Legacy canonical HTTPS `*.webdock.dev` origins retain their existing behavior.

For self-hosted websites, `SPITZLI_CMS_ALLOWED_ORIGINS` is an explicit JSON array of additional canonical HTTPS origins, for example `["https://cms.example.com"]`. Non-default HTTPS ports may be explicitly listed. Credentials, paths, fragments, noncanonical default ports and HTTP origins are refused. This allowlist is trusted server configuration, never a browser-selected destination; operators must list only CMS endpoints they control. A registered site's origin must still exactly match its configured bridge origin, with current website grants and the required role.

Mutations recheck those grants. Active tenant preview remains read-only; expired preview cannot read or write. Shop customer/order management restrictions, read-only orders/media, canvas revision checks and content response validation remain intact. Requests use the existing `/api/webdock/*` routes with a bridge Bearer secret, the existing successful-response size limits, timeouts, no-store and redirect refusal. Native cookies are not forwarded. Public dispatch responses project bridge capabilities without returning secrets.

The module's direct access and dispatch factories both require the host's installed-product gate, so disabled CMS cannot be accessed through an API wrapper that bypasses Studio operation routing. Internal registry inventory and platform identities remain independent of customer CMS. Existing site URLs, Auth bindings, Snowflake strings and Studio/SSO protocol contracts are unchanged.

## Verification and limits

`modules/cms/tests/runtime.test.ts` verifies exact arbitrary-origin allowlisting, fresh grant rechecks, preview write denial, credential isolation, direct disabled-module rejection and secret-free public projections. Existing API bridge-policy and Console metadata tests continue through compatibility exports. API and Console typechecks verify the factory composition and public metadata imports.

This is the existing Webdock bridge protocol, not a new direct Payload SDK adapter. Native editor components remain in their established packages and applications. Tests stub HTTP transport; no customer website, provider account or deployment is contacted. End-to-end website integration still depends on the target site's matching bridge implementation and operator-provided secret/configuration.
