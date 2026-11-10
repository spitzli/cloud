# Slim admin core

Owner direction, 2026-10-17: prioritize a small, plugin-based admin platform. Remove billing, without creating a billing plugin for now. Authentication has two separate plugins: Better Auth and Clerk. Analytics light appearance is disliked; a dark theme is deferred until after the core reduction.

The core owns identity contracts, authorization, project/workspace identity, the admin shell and plugin lifecycle/capability enforcement. Product functionality is optional. New installs must not implicitly enable every product. Existing explicit installation configuration and production data must remain intact.

Authentication plugins are trusted server extensions selected at startup, not untrusted project containers: they establish the identity on which project plugins depend. Provider implementation and configuration validation belong outside core. Preserve local identity IDs, RLS, bans, membership revalidation, and OAuth/MCP security semantics. Do not silently switch production identity providers.

Billing is removed from active composition, routes, operations, navigation and dependencies. Historical database migrations/data remain for retention; no replacement payment integration is built.

The existing product modules can remain optional migration units while their lifecycle is extracted; do not label them fully isolated runtime plugins until their remaining dependencies are removed. Existing analytics container/API/MCP capabilities continue to function.

No production rollout or data deletion belongs to this structural change until the new composition passes local checks.

Authentication implementation is packaged in `@spitzli/auth-better-auth` and
`@spitzli/auth-clerk`, each with an explicit trusted authentication manifest.
`@spitzli/auth-configuration` validates installation settings outside core;
core defines only identity, authentication plugin, and lazy loading contracts.
Better Auth remains the absent-setting default for existing installs; the selected
plugin loader imports only its selected entry. Clerk's SDK verification and local
mapping checks do not construct Better Auth. Its internal session bridge signs the
existing protocol cookie using the Better Auth plugin's compatibility helper.

This is a compatibility extraction, not complete independence of protocols:
OAuth/OIDC, MCP token issuance and legacy account-management endpoints still use
Better Auth, including for installations whose browser identity is Clerk. The host
still composes the product-specific Better Auth authorization/mail hooks. Those
paths may initialize the Better Auth runtime; the Clerk token verifier itself does
not. Local IDs, session rows, RLS and live provider revalidation remain authoritative.

## Delivered boundary and remaining extraction

CMS writes and retained-instance archive protection are supplied by the CMS module through host injection; core keeps operator transactions, audit and revocation checks. Legacy CMS DTO/input/query aliases remain compatibility contracts in core. The existing shared platform schema and product policy hooks still need subsequent extraction before the application can be distributed without legacy product packages. Optional modules and trusted auth plugins are distinct from isolated project runtime plugins.

## Owner update: CMS removed, Stall independent

2026-10-17: CMS is removed from active Webdock module selection, Console routes, registry collections and API/MCP mutations. The shared CMS app is archived outside workspace builds. Historical records remain; archived CMS inventory still prevents unsafe project archival. Stall Eichenbruch is a separate Payload repository, deployment, schema and local user store. Its public site must remain live, and its login is being detached from mandatory Webdock SSO. This supersedes the earlier optional CMS extension plan.

Core transports default to auth, portal and registry. MCP no longer requires hosting or lifecycle. Clerk's native Console transport is independent of Better Auth; legacy OAuth APIs/MCP are an explicitly selected compatibility capability. Shared historical database migrations are still retained, not a claim of a completely product-free SQL distribution.


## Role authority correction — 2026-10-18

The selected authentication provider now owns role and membership authority. Better Auth uses local roles; selected Clerk uses fresh Backend user privateMetadata.webdockRole for the global operator role and live organization memberships/permissions for workspace rights. Local identities remain stable resource mappings and deny-only bans/enrollment controls. Earlier text describing local roles as authoritative under Clerk is superseded. No runtime hot-swap or production role migration is implied; switching provider remains coordinated startup configuration.
