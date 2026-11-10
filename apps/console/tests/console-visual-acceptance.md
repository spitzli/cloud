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


## Customer with several projects

Start the real Console against an isolated HTTP mock (loopback only):

```sh
WEBDOCK_PREVIEW_PORT=3187 bun --no-env-file run --cwd apps/console dev:customer
```

The printed URL opens a synthetic customer with two workspaces and different
owner/member roles. Nordlicht has three projects, two databases called
`production`, a Mail domain, an empty project and a long project name. Studio
Linden has its own separate project and database. Projects created here live
only in memory. The language picker also works locally. No provider, email or
customer database is called. Restarting the mock restores the original data.
Unhandled API calls return an explicit fixture error instead of reaching the
development Core proxy. Fixture dispatch lives entirely under `tests/` and
requires `WEBDOCK_CUSTOMER_FIXTURE=1`; it is absent from the application bundle.

Run the repeatable checks against a compiled Console artifact:

```sh
WEBDOCK_BROWSER_DIST=/absolute/path/to/console/dist \
WEBDOCK_PREVIEW_PORT=3189 \
bun --no-env-file run --cwd apps/console test:customer-ux
```

The suite checks 18 customer routes at 320/375/414/768/1280px in light and dark,
plus scoped project search, workspace switching, project breadcrumbs, product
filters, creation destinations, one-project inventory failure, owner/member
controls, create success and recovery, wrong-workspace URLs and German labels.
It blocks external requests. Results and actual browser screenshots are written
to `.runtime-test/customer-ux/`. These are synthetic UI checks, not live auth,
provider integration or full-platform E2E qualification.
