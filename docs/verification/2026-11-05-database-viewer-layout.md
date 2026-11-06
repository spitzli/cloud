# Database viewer layout and Schattenclan check — 2026-11-05

Production verification resumed after the owner supplied an MFA-authenticated
operator browser session. Owner mode was entered for Schattenclan, inspected,
and exited. Final session check returned operator / preview=false / acting=false;
the retired database viewer session subsequently returned HTTP 401.

Confirmed in production:

- Schattenclan contains Lunares Admin-Panel and Lunares Discord Bot.
- Both active Neon attachments have identical provider project, branch, database,
  endpoint and customer connection references (read-only Core metadata audit).
- The running bot's BOT_DATABASE_URL and BOT_API_DATABASE_URL both match the
  attached Neon endpoint/database. Credentials were compared only in memory on
  the server and were never printed or retained in the report.
- The customer DB viewer opened through the ordinary launch/gateway flow and ran
  `SELECT current_database(), current_user, 1` successfully: neondb,
  lunares_schattenclan_browser, connection_ok=1. No customer tables were changed.
- The bot has one ready Deployment replica and a ready Pod with zero restarts.
  Its UID matches Core's observed UID. Namespace, Deployment, Service, PVC and PV
  ownership metadata were inspected without changing workload/storage resources.

Open production findings:

- Neon plugin details/branches failed to load in owner mode (browser fetch failures
  on the invoke endpoint); this remains unqualified despite correct stored linkage.
  Active Neon runtime generation 4 itself has one ready replica; older generations
  are intentionally scaled to zero. No root cause for the invoke failure is claimed.
- Core records bot desired revision 37, observed 34, failed / needs-reconciliation,
  with an ownership-conflict error. A ready running Pod is not proof of the desired
  rollout. No ownership guard was bypassed and nothing was restarted or relabelled.
- Admin-Panel is configured as provider=vercel and has no native hosting_app record.
  Its live Vercel deployment/backend connection was not established by this test;
  the empty native Applications list does not prove the external site is offline.
- The hosting directory misleadingly reports no cluster for the bot although Core
  holds a cluster ID, and reports no cluster for the Vercel project rather than its
  provider. The DB-page breadcrumb remained on Loading workspace.
- A network-lost owner-mode start committed on the server but left stale operator
  UI visible. It was recovered by checking current scope and a fresh bootstrap,
  without issuing a second start. A local regression now reproduces that lost
  response and the local UI fix re-reads exact customer/mode/current status before
  completing navigation, otherwise re-bootstraps. No mutation is blindly repeated.

Local implementation:

- `apps/console/src/Shell.tsx`: database viewer routes use the viewport width and
  remaining height; preserve the header, customer context, customer-mode banner
  and exit control. Directory routes retain their normal sidebar and spacing.
- `apps/console/src/Databases.tsx`: compact viewer heading and close link retain
  the workspace selection when returning to the database directory.
- `apps/console/src/styles.css`: remove obsolete fixed/minimum workbench heights,
  including the mobile 720px rule; let the workbench own its scrolling.
- `apps/console/src/database-workbench.css`: scoped Spitzli Cloud adapter for the
  embedded Tabularis surfaces, typography, purple accents, borders and light/dark
  mode. The pinned upstream runtime sets preset tokens inline, so scoped important
  token declarations intentionally override those. SQL syntax/semantic palettes
  remain supplied by Tabularis; its theme catalog and vendored artifact are unchanged.
- `modules/databases/src/ui/workbench.tsx`: attach the scoped theme class. Preserve
  the existing launch/authorization, cancellation, negotiation, transport and logout.
- `apps/console/tests/database-layout-browser.mjs`: genuine Console and Tabularis
  UI against explicitly local HTTP fixtures; no customer credentials or data.
- `apps/console/package.json`: include that regression in the browser test script.

The regression first failed because the primary sidebar remained visible. After
implementation it passes at 1440x900 and 390x844 in light/dark modes, checking full
width, bounded height, no horizontal overflow, live theme tokens, customer-owner
banner/exit visibility and close navigation preserving the customer ID. Existing
router search serialization retains Snowflake IDs as strings.

Commands from the repository root:

```sh
WEBDOCK_PREVIEW_PORT=0 bun apps/console/tests/database-layout-browser.mjs
bun run --cwd apps/console check
bun run --cwd apps/console lint
bun run --cwd apps/console test
bun run --cwd apps/console build
```

This geometry/theme fixture uses controlled error responses for database RPCs,
not real SQL. Tabularis may report unsupported WebSocket events for the deliberately
non-event fixture; it does not establish real query/subscription acceptance.
Build also retains the pre-existing large Monaco chunk warning. Neither warning
was suppressed. Real authenticated customer runtime, data linkage and query checks
remain required after login. At initial local qualification no production deployment or signed commit had been made.
Signing access was later restored; release and promotion evidence are tracked separately.

Final local results: browser geometry/theme regression PASS; TypeScript check PASS;
Console and plugin-UI ESLint PASS; 239 Console unit tests PASS (0 failures); production
Console build PASS with the existing Monaco chunk-size warning; diff check PASS.

The new customer-scope recovery regression passes, as do the post-fix Console
lint/typechecked production build and all 239 unit tests. Theme/full-page and
recovery changes remain local; production still uses the previous UI artifact.

Follow-up UI regressions from the production audit are now fixed locally:

- Customer owner-mode start recovers a lost response by inspecting the committed
  scope; the regression reproduces server commit followed by request abort and
  asserts only one start request.
- A real customer-role (not operator-role) fixture now checks the viewer workspace
  breadcrumb. The name comes from the existing authorized tenant query rather than
  the intentionally disabled operator customer-record lookup. No new polling/query
  is introduced.
- The hosting directory uses provider=vercel to show Vercel, and a non-null cluster
  ID to distinguish an assigned cluster with hidden name from an unassigned one.
  The failing customer-role fixture now passes for both cases. New copy is extracted,
  translated in the German workspace catalog and compiled.

Read-only cluster follow-up confirmed the bot is ready (one replica, Pod restart
count zero), and its namespace/workload/storage identifiers match Core's observed
record. This has not cleared the prior failed operation or established that desired
revision37 rolled out. SSH access worked after the owner's approval; it is no longer
a blocker. The temporary agent-owned browser tab was closed after exit; the user's
original tab and signed-in operator session are retained.

Fix-all follow-up: all four Neon view/branch reads (both projects) were verified
with genuine normal MFA+PKCE owner-mode authority through a stable HTTP client,
returning HTTP200 and valid panel blocks. Mode was exited afterward. The earlier
preview's Chromium ERR_NETWORK_CHANGED interrupted transport while Core continued;
no provider permission/credential/runtime changes were justified. Trigger of the
client network event remains unproven. The Vercel approved domain deployment and
health endpoint were also verified READY/fra1/200; its provider view now avoids
misleading native app reads. Bot cleanup fix has separate qualification evidence;
concurrent newer live bot changes were detected and reconciliation preparation
aborted instead of overwriting them.

Final fresh review: one material UI uncertainty finding, now fixed with a stalled
recovery-read regression (RED then GREEN). The follow-up current-scope request is
bounded to five seconds; success recovers only the exact requested active scope,
and uncertainty retires caches/re-bootstraps without repeating the start. No other
material/minor review findings. Vercel provider card and avoided native app polling
also pass the controlled browser test. All release/promotion gates remain explicit.

## Superseding live release

PR74 merged after Blacksmith success; exact reviewed source tree was retained.
The prebuilt Console candidate preserved the current production routes, CSP and
OIDC metadata, passed exact artifact checks, then was promoted to the canonical
Console domain with prior deployment retained for rollback. The exact EU-built
agent binary passed actual retained-configuration validation and now runs with
unchanged configuration/enrollment/certificates/keys/monotonic identity. No customer
workload was restarted by that component-only agent update. Fresh normal MFA/PKCE
operator inspection through Core -> new agent -> actual bot runtime passed.

A fresh genuine customer-role owner-mode check also passed project/provider
metadata and actual gateway SQL (neondb/customer browser role/connection_ok1).
Customer mode was exited afterward. Production index and referenced JS/CSS match
the qualified artifact bytes. These results supersede the earlier local-only UI
and release-pending statements; see the dated release JSON. Bot desired-state
reconciliation still awaits the source-of-truth decision because external releases
changed image/environment concurrently. No stale desired request was replayed,
no live newer configuration was overwritten and no success status was forged.
