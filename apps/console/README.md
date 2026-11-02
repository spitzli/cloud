# Webdock browser Console

A browser-only React, TypeScript, Vite and Tailwind package. The build produces static files in `dist/`. No Next server, business implementation, session token storage or server credentials are included.

## Run and verify

From the repository root with Bun (the only JavaScript runtime and package manager here):

```sh
bun run --filter @webdock/console dev
bun run --filter @webdock/console lint
bun run --filter @webdock/console typecheck
bun run --filter @webdock/console build
bun run --filter @webdock/console test
bun run --filter @webdock/console test:browser
```

Scripts call `bun --bun` so ESLint, TypeScript and Vite run on Bun even though their launchers declare Node. Unit tests run with `bun test --isolate`: each test file gets a fresh global object, because the DOM tests install happy-dom globals (including `navigator`, which Bun predefines without `languages`).

The workspace runs type-aware `typescript-eslint` strict/stylistic presets and the React Hooks recommended rules over source and TypeScript tests. CI treats all lint warnings as failures. Inline ESLint overrides and TypeScript suppression comments are forbidden. Unhandled promises, unsafe `any` flows, non-null assertions, non-exhaustive union switches, else-if chains and blocks deeper than three levels fail lint. Cyclomatic complexity is limited to 20 in TypeScript logic and 40 in TSX, where conditional JSX and optional display values also count as branches. These limits are ceilings, not targets.

TypeScript checks indexed access, exact optional properties, unused declarations, implicit returns, override declarations and switch fallthrough in addition to `strict`. Optional UI props explicitly accept `undefined` where callers forward absent query values; wire data still passes through runtime Zod validation. Lint dependencies are pinned because the strict preset may gain rules in minor releases.

The browser check uses Playwright Chromium and starts a test-only Vite server on port 3121, or the next free port when a local dev Console already uses it. Install the matching Chromium runtime with `bunx playwright install chromium` if needed, or set `PLAYWRIGHT_CHROMIUM_EXECUTABLE` to an existing compatible browser. HTTP fixtures exist only in `tests/preview-server.ts`; they are not included in production builds. That fixture-only check verifies UI behavior. A separate real account browser workflow is described below.

## Runtime integration

Serve `/console-config.json` from the Console's origin, without cacheable secrets:

```json
{
  "apiOrigin": "https://console.example.test",
  "authOrigin": "https://auth.example.test",
  "sso": true,
  "enabledFeatures": [],
  "browserFacade": true
}
```

`apiOrigin` and `authOrigin` must be absolute HTTP(S) origins without user information, paths, query strings or fragments. Production deployments should use HTTPS. With `browserFacade: true`, the Rust Console origin owns these browser endpoints:

| Source contract | Browser facade |
| --- | --- |
| `GET /v1/auth/session` | `GET /api/console/session` |
| `/v1/operator/registry/{customers,projects,audit-events}[/:id]` | `/api/console/registry/{collection}[/:id]` |
| `/api/auth/:operation` | `/api/console/auth/:operation` |

The facade must preserve response status, JSON shape and authorization. It must handle its own HttpOnly session, CSRF/origin checks, expiry and revocation. Auth operations used here are `sign-in/email`, `sign-out`, `two-factor/verify-totp`, `request-password-reset`, `reset-password` and `change-password`.

When the flag is absent or false, requests retain their source paths and configured API/auth origins. That mode does not make host-only auth cookies available to the legacy delegated registry API on another origin. The native first-party SSO handoff handles distinct-host installations; do not broaden cookie domains or put bearer tokens in browser storage.

Session responses retain `{data: {user, session} | null}`. Registry lists retain the direct `{docs,page,totalPages,totalDocs,hasNextPage,hasPrevPage}` envelope; reads and writes return direct records. The source-derived schemas in `src/api.ts` follow the current protocol and registry contracts. They are not yet generated from Rust OpenAPI; replace them with generated bindings once that pipeline exists.

Serve SPA fallback for Console routes, but never rewrite missing API endpoints or `/console-config.json` to HTML. The server should set an appropriate CSP, prevent framing of authenticated core pages, and use a separate HTTPS origin with no shared core cookies for custom extension assets.

## Implemented screens

- Neutral light/dark shell, mobile navigation, customer/project context and command search.
- Operator customer/project list, search, status filter, detail, create, edit, archive and restore; all current customer profile/address fields and project fields.
- Read-only audit list and event detail.
- Sign-in, authenticator verification, password reset and password change; account session summary.
- Real pending/error/empty states, aborted list/search requests, unsaved navigation and unload protection.

Only the exact `operator` session role exposes registry operations. The server remains authoritative. `enabledFeatures` is reserved for server-provided feature discovery; an enabled string alone does not make an unported screen available.

## Extension UI integration

`PluginPanel` renders version-1 declarations for text, status, tables, forms and actions using React's text escaping and native form controls. Unknown fields, HTML blocks, duplicate form names and oversized declarations are rejected. Destructive declarations require explicit confirmation.

`CustomPluginUI` accepts a validated launch descriptor `{version:1,installationId,url,actions}` and an `invoke` callback. The URL must use a separate HTTPS origin from the Console, API and Auth. Pass all core origins, including the central auth origin when the browser facade is used. The iframe permits scripts and its own origin but no navigation, popups, forms or privileged browser permissions. The bridge checks the exact source window, origin, installation, protocol version, request ID, declared action and bounded scalar input. Duplicate IDs, more than four concurrent actions and more than 256 requests per frame instance are rejected. Revocation unmounts the frame and aborts pending calls. Only validated display panels or a fixed error cross back into the frame; raw session/API responses are never forwarded.

The host-supplied action callback must reauthorize installation, project grant, action, runtime generation and idempotency on the server for every invocation. The custom-panel callback is not yet wired to a migrated launch/action API. The Extensions screen separately supports installed availability changes and their durable operations; source installation, project grants and custom-panel transport remain unported. No arbitrary plugin module is imported into the Console bundle.

## Remaining parity

This package is an implementation slice, not the completed Console migration. Account profile/external-identity administration, complete tenant/team administration, application creation/deployment/environment/deletion and metrics, source integrations, operations/infrastructure, plugin installation/grants and real custom-UI launch transport, Mail configuration/Webmail/product screens, storage and CMS still require porting and end-to-end acceptance. Canonical account security, first-party browser SSO, workspace/site lists, existing application lifecycle/settings/logs, incoming Mail lifecycle and directories, registered/managed database viewers and substantial managed-database plan/order/maintenance UI are implemented. Complete database runtime, migration and distribution acceptance remains open. Full translation coverage remains incomplete; the retained locale preference/catalog now reaches the database workbench. No legacy runtime is retired by this package.


## Canonical account surface and real browser verification

The same build serves the canonical Auth origin when `/console-config.json` returns `surface: "account"`, its own `apiOrigin`/`authOrigin`, and `consoleOrigin`. Account security, password/reset, authenticator QR/recovery, passkeys, sessions, consent and logout run there. Console uses `sso: true` to navigate through `/api/sso/login`; it never performs a sibling-origin passkey ceremony. The account bundle is loaded separately from Console.

For local real tests, use Auth `http://localhost:3125` (WebAuthn RP `localhost`), Console `http://127.0.0.1:3121`, Studio3120 and Core3130. The explicit fixture helper `cargo run -p webdock-control-plane --example account_browser_fixture` requires the owned restricted local fixture variables; it only replaces its own previously recorded test account/client and writes credentials into a private ignored file. `bun crates/control-plane/tests/start-account-browser.mjs` starts the owned Core process using those values without printing them. Run Vite on Console3121 and Auth3125, then `bun apps/console/tests/account-browser.mjs` from the repository root. The browser test uses real Core/DB services and a virtual WebAuthn authenticator, not fake API responses; no mail is sent. Never point these helpers at a retained customer database.

Set `WEBDOCK_ACCOUNT_FIXTURE=account-browser-session-isolation.json` consistently for the fixture helper, Core starter and browser test to use an independent account profile. The basename is validated; private credentials and cookie output stay profile-specific. This avoids resetting the default account when database acceptance fixtures reference it. Reset is transactional: a dependent-resource refusal leaves the recorded account/client/projection intact. The helper's `--repair` mode is restricted to its exact recorded local fixture and restores only missing matching setup records; it is not customer recovery tooling.

Set `WEBDOCK_ACCOUNT_FORCE_INITIAL=prompt` or `max_age` for the browser test to exercise forced reauthentication during first setup. Each initial-setup run needs a fresh or explicitly reset independent profile and a restarted fixture Core. Successful MFA rotates the real session; recovery codes must remain visible until acknowledged before SSO continues.

For production, package `dist` with Core and configure `WEBDOCK_UI_DIST`; no Vite or JavaScript server is required. Core serves the SPA only on configured Auth/Console/Studio hosts, reserves API paths, and applies explicit frame/connect CSP allowlists for approved plugins and database gateways.


## Native database workbench

`/databases` lists authorized customer bindings and managed databases. Operators can create registered bindings, configure/revoke explicit user grants and disable connections. The real retained `@tabularis/web-ui` React workbench is lazy-loaded at `/databases/:id`; no SQL editor is reimplemented. Its geometry reader needs the standard browser `buffer` shim, loaded only with that chunk. Browser code carries a one-use launch code and the gateway's scoped HttpOnly cookie, never database credentials or OAuth bearers.

Core requires the `databases` feature, restricted business/worker pools and exact gateway configuration. Add the approved gateway origin to `WEBDOCK_UI_CONNECT_ORIGINS`; Core includes only its exact WebSocket equivalent. Bundled font data and editor workers remain local. Global plugin discovery/keybinding requests in the retained workspace stay denied and do not prevent SQL queries or scoped session teardown.

The owned local proof uses `database_browser_fixture`, `start-database-browser.mjs` and `tests/database-browser.mjs`, after the real account browser fixture has completed security setup. It uses an isolated SQLite file, pinned existing runtime, private-CA TLS proxy and native Rust gateway. The browser executes a real query and proves that expiring its saved first-party authority blocks the live gateway session. Fixture credentials remain in ignored private files. Reuse the recorded fixture or explicitly clean only its recorded IDs; helpers must never target another database.


## Native application workbench and installed extension lifecycle

`/hosting` selects an authorized workspace and lists configured hosting projects. Workspace and registry links lead to `/hosting/projects/:projectID`, then `/hosting/apps/:appID` with distinct overview, settings and log routes. Native `POST /api/console/hosting` commands own all business checks; frontend requests reuse the retained browser-safe hosting command/app schemas. Recorded state and desired/observed revisions are shown separately from live utilization. The shell resolves application and operation context through these same authorized native queries and then reads the returned project/customer relationship. It shares query keys with the workbench, supports nonoperator accounts and direct overview/settings/logs/operation links, and never uses an operator Registry lookup to infer hosting ownership. URL customer selectors cannot override an application's returned customer context.

Existing deployed applications support start, stop, restart, rollback, scale and configuration updates. Overview controls permit another action only after a confirmed terminal operation and a fresh application read; pending, uncertain and reconciliation outcomes stay blocked. Settings use a reviewed app/allowance revision, retain unsaved inputs on conflict, and reload only after an explicit discard/review action. Image/arguments/port/health/resources are editable where the current template permits them; persistent storage and environment values are preserved. Runtime capability or quota rejection remains a server error, never a synthetic completion.

Mutation retries are disabled by default. An uncertain transport/server response retains the exact request body and idempotency key; “Retry same request” resubmits that object. A new explicit action creates a new key. A recorded receipt is distinct from runtime completion. `/hosting/operations/:operationID` polls queued/running operations and stops on success, failure, reconciliation or an error. The operation ID stays in the URL for review after navigation. An unresolved request without a receipt is held only in memory; review the server state before abandoning it or issuing another command.

Logs begin with the last sanitized stored snapshot. An explicit `apps.logs` request polls `GET /api/console/hosting/reads/:operationID`, showing the actual bounded result, truncation and failures. This is a snapshot flow, not a log stream. No metrics or inspection values are invented; their UI remains out of scope.

`/extensions` is operator-only installation administration. It reads `GET /api/console/plugins/installed`, submits revision-checked `state` commands, and links to `/extensions/operations/:operationID` for actual phase/status/condition polling. Desired availability, recorded availability and runtime state remain distinct. The current controller's disable/drain behavior retains deployed processes and data; these controls do not uninstall, shut down processes or erase data. Blocked or failed operations require review, and unknown outcomes retain their exact request for retry.

All of these queries/mutations remain private under the existing session-cache barrier. A failed required context lookup also withholds cached workspace, project and application names and links immediately. Preview can read authorized hosting records, but cannot mutate applications, request live logs or administer installation availability. Shared React components, native dialog/forms, TanStack Router/Query and existing Zod contracts provide the interface; no new frontend dependency was added.

Fixture browser coverage is in `tests/workbench-browser.mjs` and `tests/workbench-fixtures.ts`, invoked by the existing `test:browser` command. It exercises workspace navigation, successive actions without leaving the overview, dirty/conflict preservation, exact uncertain-request retry, operation success/failure, real-shaped log polling, preview denial and extension revision/blocked states. Actual hook/component regressions in `tests/hosting-state.test.tsx` cover delayed application refresh and success-to-denial context changes. These fixtures are not production fallbacks.

For a real native acceptance run, the owning harness supplies a private file through `WEBDOCK_HOSTING_BROWSER_FIXTURE` with `origin`, `cookie: {name,value}`, `customerID`, `customerName`, `projectID`, `projectName`, `appID`, and `appName`. Optional `updateCPU` selects an approved fixture resource amount; optional `expectedLog` identifies known sanitized output. Run `bun apps/console/tests/hosting-native-browser.mjs` against that owned loopback Core serving built `dist`. The script uses actual HTTP/runtime operations and never provisions or resets fixtures itself. The owning harness controls runtime preparation and cleanup; the script prints only an acceptance summary. Native proof status must be recorded from its actual run, not inferred from fixture-browser success.


Native acceptance recorded on 2026-10-20: `native_browser::console_drives_real_native_hosting_and_docker` passed with the built Console, native Core/restricted PostgreSQL, persisted AgentControl and real Docker. It covered restart/stop/start, durable terminal receipts, a sanitized log read, a revision-checked settings update and 320/375/768/1280px layouts without page errors. Installed-extension browser lifecycle currently has isolated HTTP-fixture coverage; this does not claim a real-controller browser run for extensions.

Set `WEBDOCK_HOSTING_BROWSER_SCREENSHOT_DIR` explicitly to capture desktop and mobile application overview PNGs during a native acceptance run. Without that option, the script writes no screenshots. Keep this output outside tracked source and under the owning fixture's cleanup policy.

### Native incoming Mail administration

`/mail?customer=<decimal-id>` reads customer membership and the native Mail service/directory. The service is never activated by visiting the page. Lifecycle actions use current server permissions and the exact decimal revision; preview disables every action. Suspension requires explicit confirmation and explains retained messages, credentials and storage.

Lost or ambiguous responses block further lifecycle writes for that customer and session, including navigation away and back. There is no automatic mutation retry. A newer terminal service revision resolves the latch; `needs_review` permits only actions allowed by the fresh server response and is never presented as successful execution. Unchanged or provisioning reads do not resolve uncertainty. Activate is disabled when incoming mail is already requested. Conflicts require an explicit refresh/review. Service or directory denial withholds cached private details. Session retirement uses the shared private-query isolation boundary.

Recorded service state, requested direction, and mailbox requested/applied configuration revisions remain distinct. The directory shows mailbox/domain states and existing grant requests; queued configuration is not labeled ready. The configuration surface now offers verified-domain attachment, mailbox creation on a ready attached domain, explicit grants/revocation, and server-authorized operator retries. Webmail launch remains separate. The native launch response is an exchange code and gateway origin, so a browser token URL is not invented here. The backend configuration executor and full Mail product parity remain separate gates.

Run `bun tests/mail-browser.mjs` from this directory for the documented controlled HTTP browser check. It owns the same test preview port (3121) as `test:browser`, so run them sequentially. This verifies UI behavior only; it is not a live Mail or production delivery test. Mounted and contract regressions are included in `bun run test`.

The separate real acceptance harness is `cargo test -p webdock-agent-control --test native_mail native_console_drives_incoming_mail_activation_and_retained_suspension -- --ignored --nocapture`. It requires the explicitly owned restricted PostgreSQL fixture environment, the already available reviewed `WEBDOCK_MAIL_RUNTIME_IMAGE` digest, both `WEBDOCK_MAIL_NATIVE_DOCKER_TEST=1` and `WEBDOCK_MAIL_BROWSER_TEST=1`, Node24, Chromium and a built Console. It creates a fresh database and isolated engine, serves actual Core HTTP on an ephemeral loopback port, runs the enrolled daemon, and removes only its owned resources afterward. The browser descriptor is private and temporary; no API responses are mocked. Optional `WEBDOCK_MAIL_BROWSER_SCREENSHOT_DIR` captures synthetic fixture screenshots outside tracked source. The actual run on2026-10-20 passed31.69s: explicit activation, agent-confirmed readiness, retained suspension/resume, exact revision-bound requests and responsive navigation. Its synthetic native session is separate from the real SSO acceptance above.


### Mail configuration requests

Configuration forms use the native `manageTenantMailboxes` contract. Eligible domains/people and current management flags come from the authorized directory. Quota is a decimal string in bytes, validated against the native 1 MiB–8 GiB bounds; capacity and ownership are rechecked by the server. An absent `canRetryConfiguration` flag denies retry for older API responses. Retry eligibility is never inferred from `state`: restriction-only completion can report `succeeded` while the full applied revision still trails the requested revision.

Requests are not automatically repeated. An uncertain creation blocks further provisioning requests for that customer/session until a fresh customer-bound directory contains the exact requested mailbox address/name/quota (or attached domain). Unrelated revision changes do not unblock creation. This confirms desired-state custody only; resource `pending`/`needs_review` remains visible and is not engine readiness. Conflicts require an explicit refresh before new submission. Pending/running configuration polls the real directory endpoint.

Revocation has an independent request lane: `canManage` continues to allow it during draining, blocked provisioning, or an uncertain creation, while `canConfigure` gates additions. Accepted revocation removes gateway authority immediately; remote configuration is separate. Grant/revoke forms require explicit target confirmation, cleared when the selected target changes. No per-grant applied status is invented from a configuration-wide revision. Write denial retires cached service/directory data before checking current authorization; all request ledgers use the shared session cache boundary.

`bun tests/mail-configuration-browser.mjs` exercises the configuration forms against isolated HTTP fixtures on port 3121, including actual browser polling and responsive widths. Run sequentially with the other browser scripts. Screenshots go to ignored `.runtime-test/mail-configuration-ui-screens/`. This script does not exercise a native mail engine; the separate native lifecycle harness is unchanged.
