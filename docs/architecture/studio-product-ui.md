# Studio product UI and identity boundary

Business screens live in Studio: `/tenants`, tenant profile/usage/Mail/credentials/tracking, `/admin`, `/admin/plans`, `/offers`, `/people`, `/sites`. The existing operator project/customer registry shares the same navigation and spacing. Auth keeps sign-in, password/passkey/MFA, account security, OAuth client consent and invitation acceptance. Both applications link to one another.

Studio's public JSON types are declared in its dependency-free `studio-contracts.ts`. Do not import Auth implementation files even for inferred types: Vercel installs the selected workspace's dependencies, so those imports would pull unavailable Auth SDK types into Studio's TypeScript build. Runtime session/credential payloads remain separately guarded by the delegated API.

This is a UI and authorization-boundary migration, not a database split. The existing authoritative management services stay on their current backend behind a fixed server-only delegated API during migration. Studio does not get direct identity-database access or the platform encryption key. It forwards its sealed OIDC access token and confidential client credentials to `/api/studio`; the backend pins `SPITZLI_STUDIO_CLIENT_ID`, verifies the credentials/token through native introspection and validates the existing `sid`/subject/session expiry. Native session headers are reconstructed only inside that backend for existing services and native invitation/admin hooks. Native session cookies/tokens never leave it. The operation allowlist excludes arbitrary auth calls and every original service rechecks current tenant/operator permissions.

Studio-only SSO permits a verified, setup-complete customer with an active tenant membership to hold a portal identity without creating a local Payload user. That does not authorize native Payload access. Operators still need their local mapping and completed MFA setup. SSO cookies remain host-only; new return paths are explicitly allowed only for Studio. Ordinary CMS applications retain their existing path and mapping restrictions.

Auth cutover is staged with `STUDIO_UI_ENABLED=true` and `SPITZLI_STUDIO_ORIGIN`. Its GET/HEAD business URLs then redirect to the same Studio path with query parameters preserved. Old POSTs are not forwarded across domains. Old offer links continue through the redirect; newly issued offers use the Studio origin. Deploy the backend with the pinned client first, then Studio, then enable the Auth redirect flag.

## UI and spacing

Shared operator/customer shell; desktop sidebar and collapsed mobile menu; contextual tenant tabs; role-aware credential/tracking links. Spacing uses 20/24/28/40px sections, 44px controls, consistent field labels and narrow-screen stacking. Company/address, allocation changes and provider settings use expandable sections. Actual measurements and configured allowances stay distinct. Read-only profiles omit empty fields. Light and dark themes remain supported.

## E2E verification, 2026-10-04

Disposable local PostgreSQL fixtures included five seeded identities, two tenants, a mapped Payload operator, real local OAuth clients, an invitation, plans/offers and scoped provider data. Browser work created a third customer and a sixth account. Actual local Next.js servers ran on Studio on port 3120 / Auth on port 3125; OAuth/PKCE, signed sessions, delegation, SQL persistence and server actions were real. External turboSMTP/DNS calls used a guarded fixture provider; no real email or DNS change occurred.

Verified through the collaborative browser:

- Operator SSO with exact return to plan management; 16 operator routes loaded.
- Plan creation, assignment, retained extras, capacity changes, Studio offer generation and customer acceptance; database checked exact baseline, extras, revision and accepting actor.
- Customer creation in the registry with automatic tenant provisioning and Mail subaccount provisioning.
- Customer address save, invitation delivery into the test sink, password setup/email proof, sign-in, invitation acceptance and continuation into Studio.
- Actual customer-admin password sign-in from an offer link, including return to that offer.
- Cross-tenant screens denied, native Payload APIs denied (403), registry API denied (401), root controls unavailable to customers. Ordinary member profile is read-only and invitations/credential creation are unavailable.
- Forged customer `APIS` permission rejected; normal SMTP key creation, one-time secret display, reload without secret and credential revocation persisted.
- Mail quota update, sender ownership and provider registration, tracking creation/verification/enablement, opening tracking change, and parent-domain removal blocked while tracking depends on it.
- Root settings save and provider connection test; credentials replaced using fixture values only.
- Storage snapshots displayed separately for production/preview; intentional refresh failure retained 125 MB and marked it stale/unavailable rather than zero.
- Legacy Auth business URL redirected to Studio; invitation completion links back to Studio.
- Desktop 1280/1440 and mobile 360 views checked for padding, navigation, theme contrast and horizontal overflow. Mobile menu and duplicate navigation were refined from screenshots.

Fixtures were deleted and original local platform settings/credentials restored. The fixture tests did not deliver real SMTP mail, publish real DNS or enroll a physical passkey. Existing signature/MFA regression tests cover auth behavior; production read-only storage refresh is checked separately. This report does not claim exhaustive testing of every external provider failure or every browser/device.

Final automated checks: 62 Auth tests, 39 Studio tests and 13 SSO tests passed, with TypeScript/lint/build checks. Studio deployment `webdock-admin-go2n046fi-spitzli.vercel.app` and Auth cutover deployment `webdock-auth-g0ifbrjwn-spitzli.vercel.app` are live. After the user's production sign-in, seven Studio management routes returned 200, the real Stall Blob inventory refreshed successfully, the 1280×800 sidebar fit without internal scrolling, and the 390px mobile page had no horizontal overflow. Legacy Auth tenant/plan/people/site/offer redirects and the reciprocal Account/Studio links were checked. No production customer plan, offer, Mail credential or domain was created during verification.

## Storage remains explicit

Production Spitzli/Stall still use disjoint prefixes of a shared legacy Blob store. Separate per-project and per-environment stores are the target because a store-wide credential can cross prefix boundaries. Public website images remain public even with separate stores; confidential documents/backups require private storage. No media migration or deletion happens as part of this UI deployment. URL-preserving copy/cutover and restore-tested backups remain tracked separately.
