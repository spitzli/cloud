# Native Console visual acceptance

The browser fixtures exercise the real compiled Console with disposable HTTP
responses. They do not contact a customer database, provider or email service.
For local acceptance, serve the exact artifact built on the approved EU builder:

```sh
WEBDOCK_BROWSER_DIST=/absolute/path/to/eu-artifact/console \
WEBDOCK_PREVIEW_PORT=3173 \
PLAYWRIGHT_CHROMIUM_EXECUTABLE=/absolute/path/to/installed/chromium \
bun --no-env-file apps/console/tests/ux-browser.mjs
```

`WEBDOCK_BROWSER_DIST` selects static Vite preview with environment-file loading
disabled. It does not compile application source on the workstation. The normal
source-development fixture is unchanged when this variable is absent. The server
binds loopback and closes in the browser script's `finally` block. Select one
case with `WEBDOCK_UX_CASE`; use `WEBDOCK_UX_SCREENSHOTS=true` to capture genuine
private synthetic screenshots under `.runtime-test/ui-ux/screens/`.

The suite covers actual owner/admin/member/operator routes, both themes and
320/375/414/768/1280px viewports, long names, focus/navigation, modal recovery,
mobile input readability, customer project and product menus, and read-only
resource controls. Read-only preview with owner membership must not enable
native writes; generic panels retain declared reads while disabling writes.

Run the Hosting, Mail and Plans journey fixtures against the same artifact.
Retained PostgreSQL UI tests use synthetic feature metadata and do not re-enable
retired production ordering. Archive/Object Storage component tests additionally
pin retry/cancellation and cache disposal. Neon has a separate explicit fixture
and capture tool; its API transport is synthetic and visibly labeled.

Screenshot evidence must identify its exact artifact/source, role, viewport and
fixture data. Do not depict fixture coverage as a live provider installation or
claim unsupported SQL/build/provider features from a styled page. Runtime
installation and real authorized provider acceptance are separate gates.
