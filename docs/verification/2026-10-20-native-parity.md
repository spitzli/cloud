# Native platform parity inventory

Source inspection: 2026-10-20, isolated Rust worktree. This records remaining callable contracts, not completion percentages. Retired TypeScript backends under `legacy/` remain behavior references; Rust is the sole active platform API.

## Subsequent checkpoints

The historical inventory below predates substantial changes. The completed
[Core plan](../superpowers/plans/2026-10-22-core-platform.md) supersedes its gaps for
plugin Kubernetes workloads, plans/limits/usage, verified domains and TLS, API tokens,
notifications/log history and project placement. The active React Console/account app is
`apps/console`; retired TypeScript applications and deployment paths are archived.

The [Console review](../superpowers/plans/2026-10-22-console-ux.md) and
[customer acting plan](../superpowers/plans/2026-10-23-act-as-customer.md) record native
account/MFA/passkey/PKCE and actual operator/customer browser acceptance. Account onboarding
therefore no longer has the blanket browser-verification gap stated below. Explicit customer-owner
acting preserves the operator audit identity and scopes persisted work to the exact session epoch.

[Customer-owned workloads](../superpowers/plans/2026-10-23-customer-workloads.md) and
[Mail isolation](../superpowers/plans/2026-10-23-customer-mail-isolation.md) implement retained
customer infrastructure independently of projects and one explicitly activated Stalwart CE engine
per customer. Existing project Mail data requires explicit migration; public reception, incoming
relay and the complete plugin Webmail/outbox product remain open.

The [native initial installer](../operations/native-plugin-installation.md) now provides signed
catalog review, private metadata storage, enrolled-agent runtime deployment and retained recovery.
The [local acceptance](2026-10-23-native-plugin-installation.json) covers fresh native bootstrap,
Console installation, dedicated PostgreSQL SQL write/read and runtime replacement. Generation-based
[updates and rollback](2026-10-24-native-plugin-updates.json) now retain storage and keys while deploying fresh runtime generations, with logical-schema compatibility gates and actual PostgreSQL SQL acceptance.
[Immediate plugin jobs](2026-10-24-native-plugin-jobs.json) add native enqueue/read/cancel,
leased execution, bounded retries and session/acting authority revalidation. Recurring
schedules, autonomous system jobs, purge and installation-scoped operator actions remain
subsequent slices. Removing Rust product code from
Core still requires plugin parity for Git builds/releases, Tabularis authority and durable
Mail/Webmail handoffs. These checkpoints do not establish retained-data parity or a production
cutover. The public website/CMS remains a separate TypeScript application.

## Historical inventory

| Slice | Native implementation present | Remaining contract or workflow |
| --- | --- | --- |
| Public APIs and MCP | Independent Registry bearer CRUD/archive, five Registry MCP tools, 31 Hosting bearer/tool mappings and retained plugin tools | Remaining Hosting enrollment, overview/relocation, setup/services, provider, Git and database tool families, plus provider-specific Registry hosting status. See `crates/control-plane/RESOURCE_API.md` for exact supported contracts. |
| Studio compatibility | Same-origin React facade and individual native product routes | Complete retained Studio operation envelopes and public protocol-host routing. |
| Hosting applications | Lifecycle, quotas, environment, cluster/project management, retained storage, runtime reads | `setup.*`, `services.*`, `byok.*`, project overview/relocation and cluster enrollment management. Provider-specific behavior must remain external plugins. |
| Git | Existing-source configuration, isolated builds, publication, approval/rollback and independent repository execution | OAuth connection/onboarding, target binding, webhook processing, reconciliation and retained alternative build/publication integrations. |
| Incoming Mail | Native session/grant checks, JMAP/SMTP, gateway, durable send/account notifications, typed administration, real Console/Core/daemon/Docker lifecycle and domain/mailbox/grant configuration; separately reviewed Kubernetes lifecycle/configuration/network/retained-profile acceptance | Direct Core/daemon/Kubernetes composition, independent Core runtime-UID persistence across completed operations and explicit reconciliation of an initially unverified instance. Queued configuration changes do not establish runtime readiness. Do not provision incoming engines for outgoing-only users. |
| Mail product API | Native single-send queue, status/cancel, API keys and provider callbacks | Retained `/api/mail/v1/*` batch/list/reschedule, sender domains, suppressions, logs, analytics, webhooks/replay/events and product Studio commands. Queue ownership requires explicit cutover. |
| Storage | Agent application/database filesystem and retained-volume helpers | Active tenant storage connection inventory, refresh/remove and scheduled refresh; encrypted credentials, revision fences and complete-inventory accounting. External measurement adapters belong in plugins. |
| Plugins | Registry/admission, scoped host services, action API/MCP, independent providers, availability controller and continuation fence. Manifest protocol 2 with offerings, self-service features, action audiences and resource gates; per-feature access modes (everyone, selected customers, operator assigns) with allow lists; Core resource index; customer project pages with an Add menu and plugin resource pages (declarative panel v2). Verified with restricted-role policy-matrix tests and a real Core/echo-demo browser run (2026-10-21). First-party product plugins with their own database and React UI bundled by the Console (2026-10-22): `postgres` (shared databases on loopback shared servers, dedicated instances), `mail` (sending domains with DNS ownership, platform or own turboSMTP delivery, send API and MCP tool), `hosting` (apps from container images with environment, logs, power, memory); operator resource list across customers. Verified with plugin tests against disposable PostgreSQL/Docker, Console browser tests and live Core runs. | Source installer, actual installation/update/rollback, durable jobs, action-level gRPC operation watching, installation-scoped operator actions, plugin workload capability (dedicated instances and apps use a local-dev Docker placement only), TLS to shared servers, mailboxes/webmail, DKIM, sending limits, app domains/TLS/Git builds/volumes, managed templates and full isolated custom-UI acceptance. Core product code (`crates/databases`, `crates/mail`, `crates/hosting`, Console product pages) is not yet removed. |
| Platform plans/settings | Feature configuration and resource-quota enforcement | Active settings, plans/subscriptions and offer acceptance operations. Absence of a separate Billing product does not remove these callable contracts. |
| Native databases | Domain administration, gateway, persisted Core/daemon dispatch, actual CNPG lifecycle and revoked-operation reconciliation | Docker binding authority/enforcement, usage observations, physical-only shared-pool readiness and migration execution. |
| Distribution and migration | Offline schema/role tooling, atomic operator/registry/OAuth/key bootstrap, runtime adapters and generic plugin chart | Images/releases, full Compose/Helm installation, retained-data upgrade/rollback with new writes and complete legacy runtime retirement. |

The current local-filesystem Kubernetes storage binding deliberately requires a single configured node. The owned Kind results do not establish multi-node storage placement. Renewed operation inspection is verified for Docker and native/retained Kubernetes; the separately reproduced Hosting completion deadline defect is fixed and independently verified. Incoming Mail retained-profile validation passed independent review and the exact historical workload migration passed actual Kind acceptance.

## CMS classification

Current legacy `full-installation.ts` and `full-products.ts` do not mount the CMS module; legacy registry instance writes return404. Retained CMS bridge modules are therefore not evidence of currently callable Control Plane behavior. The bundled website still has Webdock-owned Node handlers under `apps/website/src/app/api/{cms,webdock/cms}` and the associated `cms-ui/server`/`cms-bridge/server` logic. Those remain a separate first-party rewrite/retirement requirement. The external CMS engine can remain an integrated dependency. Native registry preserves the historical archive blocker.

## React surface

The native Console already exposes account/security, registry/workspaces and managed database administration. Missing complete surfaces include:

- Hosting application workbench now supports existing application navigation, settings, lifecycle, stored logs and durable operation polling. A real Core/daemon/Docker browser test verifies consecutive restart/stop/start actions, settings and responsive workspace context. Write-only environment editing, application creation and plan-confirmed deletion are verified against the real daemon/Docker runtime. Draft preparation/first deployment from a source and retained-storage removal remain open.
- Runtime inspection/metrics: native `get_read()` now has a typed, authorized HTTP polling contract with stored-result validation and final browser reauthentication. The real restricted-role test covers logs, target/session denial and preview rejection. A complete real metrics presentation remains open.
- Installed-extension lifecycle now exposes installed rows, availability changes and operation progress. Source installation, grants, jobs and full isolated custom UI remain open.
- Project extension UI: discovery/invocation are present, but grant directories/source installation and compatibility between the retained MessageChannel bridge and the new custom-UI component remain incomplete.
- Mail administration has customer navigation, incoming service state, explicit lifecycle controls and typed domain/mailbox/grant configuration. Actual Console/Core/agent/Docker acceptance covers lifecycle, configuration, restriction and authorized retry, including exact callback replay after lost acknowledgements. Webmail launch, reading and draft saving are verified end to end in Chromium against the real gateway and engine; queued Webmail sending in the browser and the remaining Mail product screens are still open.

Session/authorization cache isolation passed focused regression tests and independent review. Confirmed preview responses invalidate protected data even when the preview boolean remains true. Real native account browser checks cover ordinary setup and forced `prompt=login` / `max_age` setup, including the MFA session rotation and recovery-code acknowledgement barrier.

Invitation onboarding: operators choose Managed or Self-service per customer and invite people from the customer page, with a copyable one-use link. A new invitee gets one setup mail that creates the account (password, then an optional passkey), signs in and continues into the invitation; self-service owners then complete the customer details. Core sends account mail itself to a loopback catch-all (`WEBDOCK_ACCOUNT_SMTP`, Mailpit in Tilt). The restricted-role access test covers the whole sequence from invitation to acceptance; an in-process SMTP fixture covers the mail path; fixture browser checks cover the Console panel. The account pages for this flow have no real browser run yet.

## Next independently testable slices

1. Extend the reviewed Registry/Hosting bearer boundary to remaining callable native families, preserving each domain's live OAuth authority and scope ceiling.
2. Complete explicit inspection/reconciliation of initially unverified Mail instances, independent Core runtime-UID custody and direct Core/Kubernetes composition.
3. Extend the verified existing-application workbench to the remaining Hosting workflows and the verified Mail administration view to Webmail.
4. Complete plugin installer/jobs, Docker database bindings and the remaining product APIs before distribution/retirement.

The database reconciliation and frontend session-cache findings are closed with scoped regression evidence and independent review. This does not close the broader domain rows above. Detailed test evidence and the overall open gates remain in `2026-10-19-rust-platform.md`.
