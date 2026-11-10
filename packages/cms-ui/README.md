# Webdock CMS

**New demo policy (2026-10-05):** Prospect demos and Pizza2400 use central Webdock Studio management with an API-only Payload backend. Their /admin routes redirect centrally; no raw Payload panel, including operator fallbacks, should be added. The per-instance /cms integration described below is the legacy adapter for existing sites, not the default for new demos.

Shared content editing UI and access-controlled adapter for independent CMS instances.

Server contract: `handleCMSRequest(request, options, getPayload)` handles `/api/cms` (export GET/POST/DELETE wrappers). `cmsUser(payload, headers)` verifies the current instance session and returns a user with a supported CMS role or null. The app page redirects unauthenticated users through existing SSO before rendering `<CMSApp siteName siteURL accountURL accessURL technicalURL="/system" />`. Import `client.tsx` and `styles.css`. A client wrapper can supply `richTextEditor` for rich content.

Options: siteName, siteURL, explicit module allowlist `{slug,kind:'collection'|'global',label,titleField?,description?,previewPath?}`, locales/defaultLocale. Never include identity/system collections. Keep old `/admin` routes as redirects to `/cms`, and gate any technical `/system` fallback to `operator` before rendering upstream admin views.

Canonical source is this package. Standalone site repositories carry an identical vendored copy under `src/webdock-cms/` until a package publishing workflow is needed. Site adapters and rich-text components remain owned by their app.

Sync a standalone site with `node packages/cms-ui/sync.mjs /path/to/site-repository`, then run that site's typecheck and CMS browser suite. The source is vendored deliberately; no live cross-site dependency is introduced.

The CMS uses native access rules, validation, drafts, version history and media storage. Writes additionally verify same-origin requests, explicit modules/fields, current roles and the document timestamp inside a serializable transaction. Conflicts require reloading; creates are never automatically retried. Existing native fields/hooks remain authoritative and system/identity collections are not exposed.

The roles are Reader (view), Editor (create/edit/publish) and Administrator (also delete/restore). The operator remains platform-controlled. Apply `instance-kit.protectContent` to every collection/global so native REST calls cannot bypass the same rules. Version restoration is guarded in native beforeOperation hooks as well as the custom API. Versions, draft preview and upload actions are shown only where the configured site supports them. Globals without drafts explicitly save directly to the live website.

Unsupported or opaque fields are preserved rather than flattened. Rich-text editing is supplied by the site's existing editor dependency; incompatible node schemas remain read-only. `/system` is an operator-only compatibility escape hatch, including its metadata and server functions. This is not a new shared multi-tenant content database or a runtime collection designer.

## UI language and content language

`CMSApp` accepts optional `uiLocale: 'en' | 'de'` and `uiPreference: 'system' | 'en' | 'de'`. It always provides its own request-supplied i18n boundary, defaulting to English when embedded without an outer provider. The existing `locale` query/selector continues to select the **content language**; it is never inferred from the interface language. Parent apps pass their resolved request locale and host the shared LanguagePicker plus `/api/locale` route. Updating UI locale props preserves the mounted editor and its unsaved state.

The canonical library now has `@spitzli/i18n` as a peer dependency. Existing vendored standalone copies are unchanged until explicitly migrated together with that dependency and a language preference endpoint. Current monorepo consumer: `apps/website` imports this package directly. `apps/cms` is a retired shared-CMS landing page; it does not mount CMSApp. The external `spitzli` repository has a vendored `src/webdock-cms` copy and was not synchronized by this localization change.
