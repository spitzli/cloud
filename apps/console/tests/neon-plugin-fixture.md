This fixture imports the actual Neon `Create` and `Resource` components from
`plugins/neon/ui/index.tsx`, the actual Console API client/plugin SDK, shared query
client, session and i18n contexts, Console CSS and Spitzli brand. The session has
`role: "user"`, `customerScope: "101"` and normal write access, except in the
explicit read-only scenario. Resource views have no creation feature enabled:
local detach remains independent of that feature.

Only the fixture API transport is synthetic. It accepts the native plugin invoke
route and returns raw native response contracts: panel version 2, discovery
inventory, attachment resource, URI value and detach result. Its synthetic data
and demonstration password are not provider credentials. It cannot contact Neon
or create/delete any provider resource. Workspace Connections remains the link
for workspace-managed API keys; the plugin fixture contains no API key form.

Build this explicitly on the authorized EU build host, using the existing locked
dependencies. From the repository root:

```sh
cd apps/console
bun --bun vite build --config tests/neon-plugin-fixture.vite.config.ts
```

The only HTML input is `tests/neon-plugin-fixture.html`. The output is
`.runtime-test/neon-plugin-fixture/` at the repository root, including
`tests/neon-plugin-fixture.html`, bundled assets and Console public fonts/logo.
The ordinary Console Vite entry does not import fixture code. Do not deploy this
fixture as the customer application.

Once the EU-built output has been transferred to the local workspace, serve it
from the repository root with a foreground, loopback-only static server:

```sh
node apps/console/tests/neon-plugin-fixture-serve.mjs
```

It serves `http://127.0.0.1:3138/tests/neon-plugin-fixture.html` for five minutes
and stops automatically. `NEON_PLUGIN_FIXTURE_DIR`, `NEON_PLUGIN_FIXTURE_PORT`
and `NEON_PLUGIN_FIXTURE_LIFETIME_MS` override the output directory, port and
lifetime (at most fifteen minutes). It builds nothing, proxies no API, has no SPA
fallback and sends `Cache-Control: no-store`.

Run the acceptance script in another terminal, from the repository root:

```sh
NEON_PLUGIN_FIXTURE_URL=http://127.0.0.1:3138/tests/neon-plugin-fixture.html bun apps/console/tests/neon-plugin-browser.mjs
```

Playwright Chromium must already be available. If needed, point the existing
`PLAYWRIGHT_CHROMIUM_EXECUTABLE` setting at an installed Chromium binary. The
script starts no build or server, takes no screenshots, blocks external network
requests and requires the explicit fixture HTML URL.

The script checks real customer selection of project/branch/database/role/compute,
branch-dependent resets, disabled non-Frankfurt projects, unchanged attachment
idempotency on retry, initially hidden URI, reveal/hide, pooled selection, pending
reveal cancellation and late-response discard, no URI/password in shared React
Query caches, detach cancellation, local detach during a provider outage,
read-only controls and inventory requests, and 320px create/resource/revealed-URI
layout. The visibility test dispatches a controlled document visibility event in
the real browser; it does not claim to test operating-system tab scheduling.

For genuine browser captures, the same built entry accepts these initial states:

- Add `locale=de` to any view for German controls and field labels; other locale values use English. Customer data remains unchanged.
- `?view=create` (default): real attachment form, initially unselected.
- `?view=resource`: attached customer database with URI hidden.
- `?view=read-only`: settings without reveal, detach or branch inventory.
- `?view=failed`: unavailable provider with local detach still available.
- `?view=pending`: URI request waits until explicitly completed.
- `?view=create&retry=1`: first attach response fails after the synthetic local
  commit, then the unchanged submission can be retried.

Use the real controls to prepare selected/revealed states. Every state displays
the synthetic-data banner. Caption any published screenshots as synthetic
acceptance data rendered by the actual plugin; screenshots are not proof of a
live customer database or live provider integration.

The test-only `window.__neonPluginFixture` object exposes safe request/cache
diagnostics and controls for holding/completing a credential response or making
the provider unavailable. Its diagnostics never expose the URI response. No
mock, fixture HTML or diagnostic hook is imported by the production entry.

To regenerate unedited publication PNGs after acceptance passes:

```sh
NEON_PLUGIN_FIXTURE_URL=http://127.0.0.1:3138/tests/neon-plugin-fixture.html \
NEON_PLUGIN_SCREENSHOTS_DIR=/absolute/path/to/cloud-plugin-neon/screenshots \
bun apps/console/tests/neon-plugin-capture.mjs
```

The capture script uses customer authority, blocks external requests, checks the
URI was neither requested nor displayed, disables write actions and saves image
hashes/browser version to `provenance.json`. It also captures German mobile UI.
