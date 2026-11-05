# Spitzli Cloud TODO

## Active execution queue — 2026-11-04

**Owner priority update — 2026-11-05:** finish measured Core Neon query/compute
optimisation first, then resume the remaining queue starting with native Git/build
onboarding. Local scheduling tests alone do not close the live cost-verification task.

Owner instruction: after the workspace-icon release, continue working through this backlog.
Finish the current release before starting dependent production changes. This queue orders the
existing tasks; it does not replace their acceptance criteria or turn historical notes into fresh
purchase/deletion/credential-rotation approval.

1. **Completed: live customer icon acceptance.** PR #66 is merged; CI, actual EU image
   qualification and Core/Console/Auth rollout are complete. A genuine customer USER/owner
   passed upload/reload/navigation/Auth-list/replace/remove and foreign-write refusal404.
   QA branding is restored; session and temporary local credentials are cleaned up.
2. **Reconcile the backlog against current evidence.** Several dated entries below describe an
   older state of Auth, docs, CI and plugin publication. Verify the current code and live receipts,
   close only proven completions, and preserve genuine remaining work. Newer owner decisions
   supersede older proposals; do not repeat successful migrations or provision duplicate servers.
3. **Complete customer Git/build onboarding and its release gates.** Prioritize the authorized
   GitHub App connection, exact-SHA checkout, durable dispatcher/private BUILD connectivity,
   scoped OCI/artifact handoff, logs, cancellation and digest deployment. Prove isolation with
   denied attempts before public admission; keep existing customer apps running on failures.
4. **Enforce build-minute plans and accounting.** Global finite defaults, per-customer/free-plan
   overrides, operator top-ups/corrections, atomic reservation and idempotent charging, and clear
   customer/operator usage. Follow the detailed Build-Minuten section below.
5. **Reduce Core Neon compute hours.** Measure idle/polling/connection behavior first, then test
   changes and cold-start latency without weakening authentication, readiness or queue leases.
6. **Complete the existing observability deployment.** Fleet collection, useful upstream Grafana
   dashboards, alerting, access protection and tested recovery; reuse the booked OBS server and
   scoped Wasabi storage rather than purchasing replacements.
7. **Deliver Pulumi and the managed-service operating model.** Inventory/import existing
   infrastructure safely, version configuration and secret references, then implement staged
   updates/rollback, certificate renewal alerts/drills, off-host backups/restores and operational
   runbooks. Continue with the remaining product/lifecycle tasks below after these foundations.

Do not silently open arbitrary public builds, buy additional services, rotate live credentials or
physically delete retained customer data while processing the backlog. Prepare and verify the
concrete change; obtain any genuinely missing authorization only when it is required.


## Open source plugin platform and reseller roadmap — 2026-11-04

Owner-confirmed direction: Spitzli Cloud is fully open source and plugin based,
with a small Core and extensible products. These are product/architecture tasks,
not approval to purchase infrastructure or activate an unfinished public service.

- [ ] **Keep Core small and move product behaviour behind plugins.** Define and
  test the boundary: Core retains identity, tenant/project authorisation, plugin
  admission/lifecycle, secure configuration, audit and shared contracts. Product
  logic and provider integrations extend the panel through versioned plugins.
  Inventory remaining coupled code and migrate incrementally; preserve working
  services, central security enforcement and compatible contracts. Keep Core and
  plugin implementations open source with their component-specific notices.
- [ ] **White-label reseller experience.** Plan a branded panel, login/account
  screens, custom domains, theme/logo, transactional email and selectable product
  plugins. Add delegated reseller/end-customer administration, catalogue/plan
  controls, quotas and audit. Prove denied cross-reseller and cross-customer access.
  Existing workspace icons do not constitute full platform white-label support.
- [ ] **BYOK, managed and self-hosted reseller operating models.** Support authorised
  provider accounts belonging to the reseller or end customer with explicit
  credential ownership, scoped access, billing allocation and revocation. Define
  Spitzli-operated white-label services and reseller-operated self-hosted Core plus
  plugins. BYOK is independent of who hosts the platform and may apply to either
  model. Do not assume three mutually exclusive architectures or approved prices.
- [ ] **Package managed K3s and the own-cluster panel.** Reuse existing approved
  cluster enrollment, scoped agents and workload controls. Define managed OS/K3s
  updates, monitoring, certificate renewal, backups/restore drills, maintenance,
  support and responsibility boundaries before selling the managed service.
  Qualify wider self-service separately; do not equate extra workers with HA.
- [ ] **Evaluate VPS hosting as an optional additional product.** Decide provider
  arrangement, region, specifications, management level, costs and recovery scope
  before offering it. VPS hosting remains under consideration, not an approved
  purchasable plan or server-order instruction.
- [ ] **Document and price the reseller offer.** Provide reproducible self-hosted
  deployment, plugin installation, migrations, staged upgrades, rollback and recovery
  instructions. Separate software/source licensing, platform access, managed
  operation, underlying infrastructure and provider billing. Update public copy,
  developer documentation and the designer brief with qualified availability;
  do not invent quotas, commercial commitments or full white-label readiness.


- [x] **Neon BYOK runtime and owned-workspace API qualification (2026-11-04).** Signed native `0.1.2` is installed through the normal lifecycle with the existing Core database and unchanged credentials/schema. Full selected inventory succeeds through the public Console gateway; connection revocation during concurrent reads returns no inventory. The `0.1.1` write-path baseline proved attach/replay, protected pooled/direct URI access and actual read-only TLS SQL, quota enforcement, project binding, and local detach without provider access. Temporary keys are revoked and test attachments removed. Ordinary-customer Neon browser acceptance is also verified with a genuine USER account, including limits, forbidden foreign/operator access, URI reveal/hide, detach and relogin. Public build isolation remains a separate open gate. See [evidence and test-version boundaries](docs/verification/2026-11-04-neon-owned-acceptance.json) and [the operational procedure](docs/operations/neon-plugin-rollout.md).

- [x] **Spitzli Cloud first-party rollout and internal address pilot (2026-10-31).** Website/Console/Login use the owned graphite/violet identity. Main/Console/API and operator Grafana/Prometheus HTTPS domains are live; existing-password/MFA/PKCE/operator browser flow passes on ordinary DNS/TLS. Exact `spitzli.site` allocation, per-host HTTPS, stop/start, deletion/no-reuse and unknown-host denial are verified. Wildcard DNS is active; admission is limited to the explicitly trusted internal customer. Existing customer domains/data are preserved. [Rollout and full file list](docs/operations/spitzli-cloud-rebrand-rollout-2026-10-31.md), [evidence](docs/verification/2026-10-31-spitzli-cloud-rollout.json).
- [x] **Public Core activation (2026-10-31).** Dedicated Core HTTP/private agent-control image is pinned by digest, native schema65 entries/max version66, enrolled RUN agentv3 with monotonic identity/certificate/storage binding preserved. Original25 customers and Lunares/Schattenclan resources are retained; one owned QA customer/project is archived with no running app. Operator/Core/OBS1Password URLs are updated/read-back verified.
- [x] **Native Core source checks on Blacksmith (2026-11-04).** The complete native quality job passed on head `d370a9c`: run `41856818349`, job `126668221149`, runner `blacksmith-4vcpu-ubuntu-2404-hax24d7necf4ks2z`. All current root workflows select Blacksmith. This closes the old root-source qualification wording; independent publisher setup and retirement of the former runner remain separate tasks.
- [x] **Public Spitzli docs publication (2026-11-04).** The canonical homepage, `guides/byok-neon` and `guides/git-deployments` return HTTP200, show the intended Spitzli titles/canonical URLs and link the Spitzli repository. The site no longer shows Starter Kit. This verifies the public serving slice; protocol identity migration and complete live link coverage are separate.
- [x] **Live workspace-icon customer acceptance (2026-11-04).** The genuine QA USER/owner passed upload200, reload/navigation and Auth-list persistence, replace200, remove200 and foreign-write denial404. Anonymous writes are denied401. QA branding was restored to null; the session was signed out and local password/cookie copies removed. Existing vault credentials remain unchanged. See [the completed rollout and live proof](docs/verification/2026-11-04-workspace-icons.json).
- [ ] **Qualified canonical protocol/publication migration.** Auth/Studio/Registry retain original issuer/RP/MCP audience during the approved transition. New navigation aliases work without claiming new protocol authority. The public Docs rebrand and representative canonical routes are now verified above; finish the remaining protocol qualification separately; do not silently rewrite retained passkeys, tokens, client secrets, signed registry URLs or customer URLs.
- [ ] **Public customer address/build release gate.** Keep automatic addresses on the trusted internal pilot until browser/cookie/PSL and hostile multi-tenant qualification pass. Permanent Core BUILD dispatcher, production OCI registry/artifact/source binding and private Core-BUILD connection are not activated. The existing extension catalogue is not an OCI registry. No per-build VM was introduced.
- [ ] **Prove tenant isolation through denied attempts (owner reference, 2026-11-04).** Use [the layer-by-layer isolation lab](https://itnext.io/kubernetes-multi-tenancy-layer-by-layer-isolation-proved-by-being-refused-cc2f92f41cfc) as a test reference for the existing build and plugin boundaries. In owned disposable namespaces, record denied foreign Secret/resource access and policy/namespace-label modification; privileged/hostPath/host-network Pods; quota overflow and NodePort/LoadBalancer creation; forged trusted labels/service accounts; and placement escapes through nodeName, selectors or broad tolerations. Test real Pod admission as well as workload templates. Verify allowed control traffic and forbidden cross-tenant traffic in both directions after policy convergence, including restart/IP-reuse cases and IPv4/IPv6/private/metadata destinations. An applied policy, security-ready label, network timeout without a positive control, or one refused Pod is not a complete isolation proof. Follow [Kubernetes multi-tenancy](https://kubernetes.io/docs/concepts/security/multi-tenancy/), [NetworkPolicy behavior](https://kubernetes.io/docs/concepts/services-networking/network-policies/) and [Pod Security Admission](https://kubernetes.io/docs/concepts/security/pod-security-admission/). Preserve current runtime safeguards; this reference does not select a new CNI, vCluster, VM or paid server. Keep public build admission closed until the existing release gate passes.
- [ ] **Native archived-project deletion.** Implement the guarded project lifecycle before physical deletion of archived project/customer metadata. Current native project DELETE returns405; customer DELETE protects its remaining project409. Own HTTPS qualification metadata110015806338760704/110015805512491008 is archived; app/Pod/Service/Ingress are deleted and its permanent hostname reservation is retained. Do not bypass this with direct provider/SQL deletion.

- [ ] **Observability foundation (owner proposal, 2026-10-30).** Plan a separate single-node K3s observability server with Prometheus, Alertmanager, Grafana, Alloy and monolithic Loki using private S3 storage. Keep it independent of BUILD/RUN, retain authenticated tenant boundaries, bounded ingestion/retention and an external dead-man check. Size and price from measured ingestion before ordering a server; qualify Wasabi's minimum-duration billing for short-lived logs. Provision through the subsequent Pulumi work. See [the operating proposal](docs/architecture/observability-foundation.md); no server purchase or deployment is implied.
  - Owner authorized and ordered a **12-month VPS6 EU**, instance `100210955`, after the offline proposal. [Purchase and pending SSH/1Password verification](docs/operations/observability-host.md). Do not buy a duplicate.
  - Live continuation: dedicated OBS K3s, fixed volumes, scoped Wasabi, Grafana, Prometheus, Alertmanager and Loki are running. Operator HTTPS and 1Password URLs are verified; see [operator access](docs/operations/observability-access.md). Seventeen upstream dashboards exist; only OBS itself is collected. Fleet collection, paging, SSO/MFA and complete recovery qualification remain pending.
  - **Reuse Grafana dashboards/panels before building custom monitoring charts (owner preference, 2026-10-30).** Select compatible maintained dashboards, adapt their queries to the actual metrics/labels, and version their definitions/provisioning in the repo. Use Grafana directly for operator monitoring. Build custom charts only for uncovered needs or necessary customer-facing product UX. Any later embedding must preserve authenticated tenant-scoped queries; never expose a shared operator datasource, anonymous panel or public snapshot as a customer isolation boundary.
- [ ] **After the Kubernetes build multi-tenancy work — Pulumi infrastructure (owner request, 2026-10-30).** Create reproducible Pulumi definitions for the existing Webdock infrastructure, starting with the separate build cluster. Inventory and safely import existing resources before managing them; keep provisioning separate from application rollout, reference secrets securely, and document preview/apply, drift detection and recovery. Do not replace running resources or rotate live credentials implicitly. Follow the [build multi-tenancy plan](docs/superpowers/plans/2026-10-29-build-multitenancy.md) first.
- [ ] **After the Kubernetes build multi-tenancy work — Managed service operating model (owner request, 2026-10-30).** Define how Webdock operates as a real managed service: ownership and support scope; OS/K3s/runtime/builder/application updates and security patches; version pinning, staging, maintenance windows, staged rollout and rollback; monitoring, alerts and incident response; tenant isolation, capacity and quotas; credential/certificate renewal; off-host backups, tested restores and disaster recovery; retention, customer communication and operating costs. Separate measured capabilities from future availability/support commitments and turn the concept into concrete runbooks and implementation tasks.
  - **Concrete renewal gate (2026-10-31):** Core/RUN agent mTLS leaf certificates expire2027-01-29T11:07:09Z. Implement expiry alerts and rehearsed server/client renewal before this date, preserve enrollment generation/monotonic counters, verify negative peer tests and encrypted recovery/1Password backup. Current leaves are manually issued; automated renewal is not installed.

- [x] **Act as customer (2026-10-23).** A platform operator can enter explicit customer-owner read/write mode with a visible banner and exit, retaining the original audit identity and exact session/customer epoch. Read-only preview remains separate. Identity/security, API-token management and public OAuth/MCP delegation stay outside acting mode. Native browser and restricted authority/queued-invitation regressions pass; see `docs/superpowers/plans/2026-10-23-act-as-customer.md`.
- [x] **Customer-owned workloads and Mail isolation (2026-10-23).** One explicitly enabled incoming CE engine per customer, shared across projects, with retained suspension and restart reconciliation. Workloads, limits, domains, usage and notices support customer ownership independently of project lifetime. Legacy project engines need explicit migration; public reception, incoming relay and plugin Webmail/outbox parity remain open. See the dated customer-workload and Mail-isolation plans.

- [x] **Native plugin installation and storage lifecycle (2026-10-24).** Signed Console installation, isolated metadata storage, enrolled-agent deployment and retained generation-based update/rollback are implemented and verified with PostgreSQL. See `docs/operations/native-plugin-installation.md` and `docs/verification/2026-10-24-native-plugin-updates.json`. Mail is also verified through real Console/CE installation, compatible update/rollback and retained mailbox/PVC lifecycle; see `docs/verification/2026-10-24-managed-mail-installation.json`. Further product onboarding, public delivery and production rollout remain separate.
- [x] **Immediate native plugin jobs (2026-10-24).** Durable enqueue/read/cancel, leased execution, bounded retries, retained idempotency and session/acting/generation authority checks are implemented. See `docs/verification/2026-10-24-native-plugin-jobs.json`.
- [ ] **Scheduled/system plugin jobs and installation-scoped operator actions.** Add explicit autonomous authority and recurring schedules beyond the immediate session-admitted job slice. Pool/capacity administration needs an explicit installation scope, with no dummy customer/project.
- [ ] **S3 and Managed Object Storage (owner scope, 2026-10-25).** Implement the shared S3/Wasabi foundation, durable build/release artifacts, managed customer object storage, PostgreSQL/Mail/app backup and restore, CMS/app files, protected exports/downloads, archived logs/diagnostics and an OCI registry storage backend. Build cache is explicitly excluded. MinIO already exists for local tests; the owner has a Wasabi account. See [the new-agent handover](docs/handovers/2026-10-25-s3-managed-storage.md) for scope, existing boundaries and acceptance requirements.
  - Locally qualified: private S3 transport, durable OCI/source/provenance artifacts, authenticated S3 registry, managed object storage with signed native/browser flows, dedicated PostgreSQL backup/PITR, opt-in CMS media, protected exports/diagnostics and completed build-log archives. Genuine matched Core/Mail/private-PG/CE backup → source unavailable → cold restore now passes in 150.154 seconds, with authenticated content, closed uncertain delivery, no egress and replay denial. Full Rust/Console/Python quality passes; see [final local acceptance](docs/verification/2026-10-29-s3-native-recovery-final.json). Historical evidence remains in the [stepwise plan](docs/superpowers/plans/2026-10-25-s3-managed-storage.md). Separate release qualifications remain: actual positive-object PostgreSQL MCP coverage beyond the qualified metadata/OAuth path, Wasabi account compatibility/production activation, public Mail ingress and the native incoming 10GiB lane; none is implied by the local 2GiB CE proof.
- [x] **Evaluate Jenkins (owner note, 2026-10-25).** [Dated evaluation](docs/architecture/jenkins-evaluation.md): keep the native isolated buildworker; consider Jenkins only as a separately admitted external CI producer when a concrete need exists. Reviewed controller/agent isolation, credentials, maintenance, S3 artifact handoff and retained deployment authority. No Jenkins installation or build cache.
- [ ] **Evaluate Kubernetes preview environments.** Review [MetalBear's preview-environment tools comparison](https://metalbear.com/blog/best-kubernetes-preview-environment-tools/) (added 2026-10-24). Compare isolated full-stack namespaces, request routing and virtual clusters for Webdock's Rust/Console/plugin stack. Assess PR lifecycle/URLs, independent databases/queues/PVCs, masked test data, TTL/cleanup, concurrency and costs. Treat the vendor comparison as research, not a tool decision; keep build-runner infrastructure and production separate.
- [ ] **For 2026-10-25 — Integrate native Tabularis Web.** Follow the owner's Discord announcement for Tabularis0.29 Docker/browser support and Helm chart0.29.0. Verify the announced `ghcr.io/tabularisdb/tabularis:0.29.0` image and `oci://ghcr.io/tabularisdb/charts/tabularis` chart, pin their digests and account for linux/amd64-only support. Integrate through the existing PostgreSQL resource flow with native authentication and customer/project-scoped database authority; configure `publicUrl`, private routing, ingress and TLS together. No implementation today. Sources: [image](https://github.com/TabularisDB/tabularis/pkgs/container/tabularis), [Compose](https://github.com/TabularisDB/tabularis/blob/main/docker/compose.yaml), [Helm](https://github.com/TabularisDB/tabularis/tree/main/charts/tabularis), [issues](https://github.com/TabularisDB/tabularis/issues).
- [ ] **Finish product ownership before removing Rust product code from Core.** Port retained Mail outbox/uncertain handoffs/Webmail, PostgreSQL Tabularis authority, and Hosting Git build/release workflows into their plugins; verify each replacement before deleting its Core routes, UI and dependencies. Next product additions are tracked in `docs/plugin-ideas.md`, starting with S3/backups and Git deployments.

- [x] **Priority 1 — Complete MCP and API coverage for working from T3 Code on a phone.** Inventory the existing Studio workflows and their MCP/API coverage, then fill the gaps so the operator can work with the agent from the Android T3 Code client without depending on its unavailable embedded browser preview. Reuse Studio authorization, scoped OAuth and audit rules. Verify client connection, consent, token expiration/reconnection, and representative read/write workflows end to end. Use the available T3 browser automation where supported and Playwright where appropriate; complete interactive sign-ins, MFA and consent requiring the shared preview at the PC. A separate phone-browser login does not authenticate the agent's browser. Document which workflows work remotely and which still require a PC step.
- [x] **Priority 2 — Make Webdock mobile-friendly.** Review and improve Studio, central sign-in/account/connections screens and the public Webdock site on narrow Android viewports. Cover navigation, customer/project forms, tables, search/filter/pagination, dialogs, GitHub repository selection and MCP client setup. Ensure touch-friendly controls, readable content, usable on-screen-keyboard behavior and no unintended horizontal overflow; verify light/dark appearance and real phone workflows.

- [ ] **Connect the T3 host to the production MCP server at the PC.** Register the host client's exact callback with read/write and optional renewal, complete personal OAuth/MFA consent and verify a live read. GitHub repository-selection verification also still needs the personal browser login. Implementation and disposable local-account checks do not replace this step.

- [x] **Move the Webdock workspace URL from `admin.webdock.dev` to `studio.webdock.dev`.** Update the Vercel domain, application origin, registered SSO callback/logout URLs, account continuation link and documentation together. Redirect the old address after verifying the new sign-in flow.

- [x] **Direct SSO entry for every Payload admin.** Unauthenticated `/admin` requests should go straight through the central Webdock login and return to the intended admin page. Do not show the Payload login form. Fully disable local Payload email/password login, password recovery and native JWT authentication. Finish identity mappings and operator MFA/customer onboarding before the per-site cutover so existing users are not locked out. Central Better Auth sign-in remains the identity entry point.
- [x] **Create a dedicated Webdock favicon.** Derive a recognizable small mark from the Webdock branding, provide suitable SVG/ICO assets, and check readability at 16/32 pixels in light and dark browser themes.
- [ ] **Plausible tracking module for Webdock, Spitzli and Stall.** Add per-site enablement and validated Plausible script configuration.
- [ ] **Dependency maintenance.** Review the remaining transitive audit findings in the pinned Payload/build-tool dependency trees (including braces/chokidar/micromatch/sass and esbuild/drizzle-kit). Do not use an indiscriminate major-version audit fix during the auth cutover.

- [x] **Studio functions and UX.** Search/filter/pagination, clearer details and history, accessible form feedback, responsive system light/dark UI.
- [x] **MCP read/write and OAuth.** Scoped operator tools, exact callback registration, PKCE/consent and current permission checks; no infrastructure actions.
- [x] **Passkeys.** Central enrollment and sign-in with required device verification; no extra OTP after a verified passkey. Password sign-in retains MFA.
- [x] **GitHub integration.** Spitzli-owned GitHub App, metadata-only selected repositories, operator-bound connection and verified repository picker.

- [x] **Account navigation and customer access management.** Studio links to My account and operator-only People & access. Provision invitation-only customer accounts, complete password setup by email, accept customer membership, assign per-website Reader/Editor/Administrator roles, revoke permissions, suspend/restore accounts and cancel/resend invitations. Customers find their authorized Payload managers under My websites. No public sign-up or customer self-service console.
- [x] **CMS product UI and central roles.** Shared `/cms` interface for independent sites, customer-friendly sections, media/relationships, content blocks, drafts and versions where supported. `/admin` redirects to CMS; `/system` is operator-only. Reader/Editor/Administrator permissions are enforced on native content operations too. Customer-facing product name is CMS.
- [x] **Finish live read-only Vercel connection.** Private Webdock Studio integration created and connected in production with exactly four read scopes. All Projects access enabled at the user’s request (16 Vercel projects visible). Verified live production/deployment/domain panels for Webdock, Spitzli and Stall. Encrypted token storage, project mapping and MCP/API hosting status are deployed. Public /terms, /privacy and /integrations/vercel are live. Private team dashboard installation URL replaces the public marketplace URL, which returns 404 for private integrations.

- [x] **Tenant-ready customers and invite-only onboarding.** Customer/contact/personal details and postal address are live in Studio. Atomic customer-to-organization provisioning preserves existing Spitzli/Stall mappings. Central /tenants portal has profile editing, invite-only setup, scoped membership/roles and archived-tenant enforcement, with separate CMS grants. Verified 39 admin tests, 14 auth tests, browser profile save and cross-tenant denial, mobile light/dark layouts and live production pages (2026-10-04).
- [x] **Mail (turboSMTP): provider setup and customer management.** Supplied Spitzli Development master Consumer credentials stored encrypted; API connection and sending IP verified, Mail enabled. Tenant subaccount provisioning/linking/limits/status, sender-domain registration/DNS/recheck/removal, custom tracking domains/settings and SMTP/API key lifecycle are implemented. RelayKit supplied the missing sender-domain contract. Scoped child live tests covered key create/revoke, sender create/list/delete and tracking create/detail/disable/delete; temporary resources cleaned. Individual tenants still need an explicitly provisioned/linked sending account; existing provider accounts are never assigned by guesswork.
- [x] **Auth database connection lifecycle.** Bind the PostgreSQL pool to Vercel Fluid Compute so idle connections close before suspension. Regression test, all 11 auth tests and build/lint pass; deployed 2026-10-04, live /connections and /sites checked concurrently and after idle, no new deployment error logs at verification.

- [x] **Central Webdock Root administration.** Platform-operator-only /admin on auth.webdock.dev, linked from Studio and account. Persisted name/support, live account/email branding, CMS customer-access and Mail switches, assigned sending IP, default quota/region, encrypted provider configuration and read-only connection test. Verified mobile root save, tenant denial, 25 auth tests, type/lint/build; live checks 2026-10-04.

- [ ] **After Mail: discuss domain management with the user.** User asked to be contacted once Mail is complete. Clarify Namecheap API versus a domain-reseller API; assess central domain inventory/management, availability checks and whether registration/purchase through API is supported. No domain purchases, provider enrollment or DNS changes authorized by this TODO.
- [x] **Mail full customer self-service.** Customers manage sender domains, public DNS instructions, custom tracking, opening/click tracking and SMTP/API credential create/revoke/replacement in Webdock. Master credentials stay server-side; all provider actions use explicit child authorization. 43 auth/integration tests pass; mobile browser flows and cross-tenant denial verified. No customer turboSMTP login required.
- [ ] **BYOK / own infrastructure, self-hosting and reseller packaging.** The newer owner-confirmed open-source/plugin and white-label roadmap above supersedes this older Enterprise-only proposal. Hosted/managed service packaging and prices still require decisions; source availability is not an Enterprise-plan promise.
- [ ] **Pricing ideas — retain for later decision (2026-10-04).** Previous proposal: Basic €9.90/month, Plus €19.90/month as intended end-customer prices; website creation, migration, custom changes and domains charged separately. User now wants to explore a cheaper tier for rarely updated, low-traffic websites, e.g. €4.90/month billed annually (€58.80/year), with bounded usage and no included manual content changes. These are discussion proposals, not agreed or published prices. User reports an existing free turboSMTP 100k plan and access to employee special pricing; use €0 for that existing allowance in current cost estimates, with the allowance shared across customers rather than promised per customer. Low traffic/build frequency reduces variable costs, but platform maintenance, support and shared infrastructure still need covering.

### Product and pricing exploration — ideas only, 2026-10-04

- [x] **Plan and offer administration requested after the ideas discussion.** Own reusable/custom plan templates, tenant assignments, persistent extra allowances, private expiring tenant-bound offers with commercial terms and explicit administrator acceptance. Acceptance assigns automatically, without checkout/payment or sending mail. Revision history, stale/revoked offer protection, operator controls and mobile UI implemented. Usage shows measured Blob inventories and actual Mail provider snapshots separately from configured allocations. See [implementation and limitations](docs/architecture/plans-and-storage.md).
- [x] **Blob measurement and scheduled refresh.** Existing active Spitzli/Stall media prefixes mapped explicitly with encrypted credentials and complete inventory. Hourly authenticated batch refreshes mappings due after 24 hours; errors preserve stale snapshots and never show zero. Preview usage stays separate. Shared legacy store is labelled honestly; no files moved/deleted.
- [x] **Studio UI consolidation and UX.** Customer/tenant, plans/offers, Mail, access and platform settings moved into the Studio UI with a shared role-aware shell, compact mobile menu, tenant navigation, consistent padding and progressive disclosure. Auth retains identity/account security and forwards legacy business links after cutover. Host-only cookies and scoped native-session delegation preserved. Independent public DTOs keep Studio builds free of Auth implementation dependencies. Local browser E2E covered real SSO, new-customer provisioning, invitation/password setup/acceptance, plan/offer changes, scoped Mail operations, cross-tenant/root denial and mobile/light/dark layout. See [architecture and verification](docs/architecture/studio-product-ui.md).
- [ ] **Follow-up: enforce allocated capacity across product writes.** Plan assignments currently record allocations; storage admission/reservations across native CMS uploads, editor/site creation limits, automatic provider Mail quota synchronization and reliable transfer accounting are not yet connected. Existing provider Mail limits remain authoritative. Do not sell configured allocations as fully enforced infrastructure limits.

- [ ] **Project-isolated Blob storage and recovery strategy.** Proposed design: own media store per project and environment; production and preview write access separated; explicit tenant/project/store registry; usage per project aggregated against plan baseline plus tenant extras. Private backups with separate credentials, daily incremental recovery points and candidate 30-day retention; restore verification and costs required before any service promise. Inventory existing stores before migrations and preserve existing URLs through a controlled cutover. See [tenant plans, usage and storage design](docs/superpowers/specs/2026-10-04-tenant-plans-usage-design.md). Strategy recorded; infrastructure and usage functionality not yet implemented.

No implementation, published offer, provider purchase or final entitlement decision is authorized by this section. Proposed quantities are starting hypotheses to validate against measured costs and provider enforcement capabilities.

- [ ] **Low-cost Website Mini.** Explore €4.90/month, billed €58.80 annually, for an existing small business/personal website: one website, HTTPS, CMS access for one editor, connection of one customer-owned domain (apex + www treated as one), 500 MB of customer-uploaded media and 100 outgoing transactional emails per month. Explore 10 GB/month of website transfer as a planning allowance, only advertise after reliable per-site measurement and an excess-use policy exist. Mail example: contact-form notifications and confirmations; bulk campaigns excluded from this initial tier. Website creation/migration, manual edits, domain registration/renewal and incoming mailboxes are separate. Platform-generated login/invitation emails should not consume the customer's 100-email allowance. Annual payment reduces payment/admin overhead; VAT presentation and cancellation terms still need deciding before publication.
- [ ] **Explain the offer on the public website.** Lead with the customer outcome, e.g. “Deine kleine Website zuverlässig online — Inhalte selbst bearbeiten, eigene Domain verbinden und Kontaktanfragen erhalten.” Show “4,90 €/Monat · 58,80 € jährlich” together, plus applicable tax wording. Use plain labels: “1 Website”, “500 MB für Bilder und Dateien”, “100 versendete E-Mails pro Monat”, “Eigene Domain anschließen”. Explicitly distinguish connecting an existing domain from buying a domain, outgoing website email from an inbox, and self-editing from paid changes by Webdock. Show inclusions, exclusions, limits, renewal prices and upgrade options beside the pricing cards; do not promise unlimited use or a drag-and-drop builder we do not provide. Keep Basic €9.90 and Plus €19.90 as candidate tiers; differentiate through useful capacity/features after validation, rather than inventing support commitments.
- [ ] **Tenant usage dashboard and accounting.** Define a single entitlement record per tenant with plan, period, quantities and explicit operator overrides. Track customer media bytes from actual stored objects (including retained variants/versions), reconcile periodically and show used/available storage. Count email recipients accepted for delivery, including To/CC/BCC, rather than API calls; deduplicate retries/events, identify accounting period/timezone and reconcile against provider counters. Confirm turboSMTP counter semantics before setting quotas. Map every child account and website/project to its tenant. Obtain per-project transfer/runtime/build/storage costs through an authorized provider API if available; confirm permissions and granularity, since the existing four-scope Vercel integration does not establish billing/usage access. Separate measured values, delayed values and estimates; do not use page-view analytics as a proxy for billable traffic. Keep a root overview of total costs, shared 100k Mail usage, tenant outliers and estimated contribution after costs.
- [ ] **Enforce allowances predictably.** Server-side storage admission checks before upload, with concurrent reservations and final-size reconciliation. For outgoing mail, enforce provider-side child quotas wherever supported: direct customer SMTP/API credentials bypass a Webdock-only counter. Verify which limits are hard, their reset period, and how shared/auth mail is accounted for. Warn at e.g. 80% and 100%, expose upgrade/extra-capacity options, and require explicit agreement before paid overages. At a storage cap block new uploads while retaining existing content and allowing cleanup; at a sending cap reject new sends clearly rather than silently losing mail. For traffic spikes, investigate abuse/caching and notify the customer; do not automatically take down a live site based on delayed counters. Decide grace periods, spend protection and the response to sustained excess before selling the allowance. Do not promise a hard infrastructure spending cap unless the provider actually enforces it.
- [ ] **Domains as an optional Webdock product.** First support connecting existing domains with readable DNS/verification status. Then research Namecheap versus reseller APIs for availability, registration, transfer, DNS, renewals and customer registrant details. Show first-year and renewal price per TLD, with registration separate from Website Mini; retain customer ownership and allow transfer out. Plan explicit purchase confirmation, auto-renew opt-in, expiry notices, failed-payment handling, transfer locks/codes and audit history. Scope DNS rights separately from read-only Vercel access. Provider suitability, margins, premium domains, taxes and obligations need research before enabling purchases.
- [ ] **Incoming Mail / real mailboxes.** Treat the user's request as an idea for receiving email at an own-domain address, with clarification before implementation. Explore a separately priced mailbox add-on with IMAP/SMTP, webmail, aliases, spam filtering, mailbox storage, account/password management and migration. A candidate small mailbox might have 1–5 GB, but choose capacity and price only after provider/reseller cost research. Compare forwarding-only as a simpler, clearly labelled alternative; receiving email into an app/webhook is another distinct use case. Current Webdock Mail covers outgoing turboSMTP delivery and does not provide customer inboxes. Investigate a managed mailbox reseller rather than operating our own mail server. Keep mailbox storage/quotas distinct from website media and transactional send quotas; coordinate MX and SPF/DKIM/DMARC without overwriting an existing mail provider.

- [ ] Neon Core operations: backup/restore drills, production-branch protection,
  reviewed credential custody, connection/egress monitoring and branch TTL cleanup.
  Keep native MFA/passkey identity and custom organization authorization.
- [ ] **Reduce Neon Core database compute hours (owner request, 2026-11-03).**
  Measure the current CU-hour baseline and identify background queries, health
  checks, dispatcher/reconciliation polling and connection-pool activity that
  keep compute awake. Review autoscaling bounds, scale-to-zero suitability,
  idle connection handling, query efficiency and unused branch compute. Preserve
  authentication/MFA, queue leases, reconciliation and service availability;
  measure savings and cold-start latency before changing production defaults.
  The first [bounded idle-polling increment](docs/superpowers/plans/2026-11-05-neon-idle-polling.md)
  is live for both the digest-verified dispatcher and the exact EU-built RUN
  agent. Owned PostgreSQL18 migration/readiness, dispatcher idle/shutdown,
  144 Kubernetes-only all-target tests (25ignored), actual host validation and
  normal operator MFA/PKCE Core-to-agent inspection passed. Enrollment,
  certificate/key hashes and monotonic counters are preserved. The uncontrolled
  database snapshots span different loads and a four-hour gap; they do not prove
  reduced aggregate traffic or CU-hour savings. Attribute the remaining traffic
  and increased peer-connection counts before closing this cost task. See
  [the exact live rollout and measurement limits](docs/verification/2026-11-05-neon-idle-polling.json).
- [ ] Realtime capacity: benchmark revision invalidation/fanout before large tenant
  counts; fixed counters currently intentionally contain no tenant/customer rows.
- [ ] Existing agent lint debt outside the Neon/read-loop change:
  `crates/agent/src/config.rs` collapsible-if and
  `crates/agent/src/recovery/config.rs` large enum variant. No lint suppression
  was added during the migration.

### Customer UX verification — 2026-11-01

- [ ] **Native GitHub onboarding is a release blocker for self-service Git builds.**
  Native discovery and the first local protocol/schema/provider-boundary slice are recorded in
  [the implementation brief](docs/superpowers/specs/2026-11-04-native-git-connection-design.md).
  The [signed connection-authority contract increment](docs/superpowers/plans/2026-11-04-git-connection-authority-contract.md)
  separates connection operations from source leases and binds their complete expected scope.
  This contract alone does not implement live session admission, durable replay handling,
  provider OAuth, callback/UI integration or production activation.
  Port the existing authorized GitHub App connect/callback flow into the native
  repository adapter boundary; preserve tenant/session binding, signed state,
  installation ownership, exact repository permissions and reconnect/revocation.
  `plugins/github-repository/README.md` currently explicitly leaves OAuth onboarding
  pending. Configure the actual reachable project-scoped registry and repository
  runtime before enabling `WEBDOCK_BUILD_REGISTRY_HOST`; never fabricate a
  connection or use a global registry credential. Acceptance: a genuine customer
  connects their selected repository, builds one exact SHA, sees durable logs and
  deploys its digest; other customers' installations remain inaccessible.
- [ ] **Complete retained service usage reporting.** Native workspace plans and
  project limits work, but legacy Hosting applications are not plugin-unit usage
  samples. Reconcile their allocations/measurements before presenting total
  CPU/RAM/storage usage or selling combined hard limits. Empty telemetry must stay
  labelled as unreported, never as no running applications.
- [ ] **Database browser infrastructure as managed IaC.** Preserve the restored
  per-subject runtime grants, TLS CA, project credentials and gateway secret in
  Pulumi/secret references. Publish pinned images to the approved registry for
  placement/failover instead of depending on the wd-eu-02 image cache. Add
  certificate/health monitoring, retained browser-state policy and renewal drills.
  Do not replace actual application/database PVCs. The old database.webdock.dev
  bookmark still needs the separately controlled legacy DNS migration.
- [ ] Refresh the existing 1Password runtime entry with the new database gateway
  origin/private Host-config reference and retain the new invocation signer there.
  The 1 November CLI request timed out; use the owner-encrypted audit custody and
  exact read-back comparison, preserving every existing password/MFA field.
- [ ] Embedded database editor accessibility: remove its nested main landmark in
  the maintained UI package; keep the Console's skip-link/main region intact.
- [ ] Retained database editor capability handling: when the real editor bundle is
  loaded in the owned startup fixture, the SDK attempts WebSocket subscriptions
  despite negotiated `events: false` and emits unhandled rejections. Qualify a fix
  in the maintained UI dependency without suppressing errors or advertising a
  capability the server does not provide. This fixture intentionally rejects SQL
  RPC and does not establish a working SQL editor or a production reproduction.

- [ ] **Retained TypeScript Hosting composition boundary (2026-11-04).**
  `modules/hosting/tests/boundaries.test.ts` rejects existing SQL service logic in
  `legacy/control-plane/src/modules/hosting/plugin-provisioning-store.ts`.
  The npm scope migration changes only its SDK import; the same forbidden SQL
  is present in pre-migration commit `bccb838f`. Reconcile this retained legacy
  ownership separately without weakening the boundary test or changing native
  Rust Hosting authorization. The broader module suite currently has 166 passes,
  four skips and this one failure; it is not fully green.

### Build-Minuten je Kunde und Plan

- [ ] Globales Standardkontingent für Build-Minuten festlegen; pro Kunde und über
  kundenspezifische Pläne überschreibbar. Auch kostenlose Pläne mit manuell
  zugewiesenen Build-Minuten unterstützen; keine Zahlung für einen Free-Plan
  voraussetzen. Der Operator kann Kontingente zuweisen, Minuten zubuchen und
  fehlerhafte Abzüge mit protokolliertem Grund korrigieren.
- [ ] Tatsächliche Build-Laufzeit serverseitig messen und vom Kundenkontingent
  abbuchen. Messbeginn/-ende, Abrechnungsperiode, Rundung sowie die Behandlung von
  fehlgeschlagenen, abgebrochenen und wiederholten Builds transparent festlegen.
  Warteschlangenzeit separat anzeigen. Reservierung und endgültigen Verbrauch
  atomar verbuchen, auch bei parallelen Builds; Webhook-Duplikate, Wiederanläufe
  und Dispatcher-Neustarts dürfen keine Doppelabbuchung verursachen.
- [ ] In Kunden- und Operatoransicht enthaltene, verbrauchte, reservierte und
  verbleibende Minuten sowie Ablauf/Erneuerung des Kontingents zeigen. Bei
  ausgeschöpftem Limit neue Builds verständlich stoppen; bestehende Anwendungen
  laufen weiter. Nachbuchung/Upgrade anbieten, aber keine automatische bezahlte
  Erweiterung oder Belastung ohne ausdrückliche Vereinbarung.
- [ ] Core agent-control rollout reliability: a Recreate Pod replacement can leave
  the private systemd TCP pass-through stale. Current verified hook: after private
  controller readiness, restart only `webdock-core-agent-control.service` on Core
  and verify a fresh real RUN inspect. Keep the listening socket, mTLS, enrolled
  generation and customer workloads unchanged. Qualify automatic connection
  draining/reconnect through a disposable controller outage before removing this hook.

## Independent plugin release follow-up (2026-11-01)

- [ ] **Use Blacksmith exclusively for our own GitHub CI (owner instruction, 2026-11-01).** Migrate Spitzli platform/plugin/SDK GitHub Actions execution to [Blacksmith](https://www.blacksmith.sh/), including reusable workflows. The owner waived EU-only placement for our own Blacksmith GitHub CI on 2026-11-01; record and verify the configured/observed runner region (currently dashboard US West) and qualify the supported managed builder before migration. This exception does not change EU requirements or isolation for customer builds or other services. Blacksmith App installation and exact16-repository selection are confirmed; the isolated runner/build probe passed in `us-west` ([job](https://github.com/spitzli/cloud/actions/runs/41319592763/job/124886768224)). Root native source checks and normal workflow publication are verified, including the successful quality job for `62d1236` in PR #71. This supersedes the older root-source-pending state; independent publisher qualification and retirement of the former runner remain separate open gates. See [the current CI record](tools/plugin-ci/BLACKSMITH.md). The old root manual distribution workflow is disabled; the old runner/host remain unchanged. Do not configure an alternative runner fallback. This supersedes the earlier self-hosted runner choice for our own GitHub CI. Retire the existing trusted CI runner and its credentials/cache safely after replacement checks pass; update CI documentation and infrastructure plans. Customer build-cluster provisioning remains a separate workstream.
- [ ] **Update documentation alongside the plugin migration (owner reminder, 2026-11-01).** Update public developer/customer docs and internal operations guides with canonical Spitzli plugin/SDK repositories, version pins, actual CI region and the own-Blacksmith exception, registry publication and signature verification, installation/update steps, and troubleshooting. Explain v1/v2 compatibility, provider catalog entries requiring separate runtime binding, and the maintenance-only Postgres policy with Neon BYOK as default. Refresh links and examples; distinguish published images from signed registry admission and actual activation. Validate navigation and links before marking complete.
  Source overhaul and the exact public package are qualified: 44 pages, strict Mintlify build and link checks passed. The canonical public homepage and representative Neon/Git guides were verified on 2026-11-04; all44 navigation pages and declared internal MDX links additionally passed the live HTTP/title check on 2026-11-05. Exact dashboard/source mapping and complete page-body equivalence remain separately unverified; dashboard login is needed only if those settings cannot be confirmed from the established deployment path; see [the completion record](docs/operations/documentation-overhaul-2026-11-01.md). Keep this item open until the real site and final plugin admission status are updated.
- [ ] Move Core compatibility/UI/test source copies to reviewed versioned plugin inputs after the independent runtime releases are qualified; preserve Archives Core acceptance tests.
- [ ] Provision repository-scoped registry publisher tokens through the owner account for unattended publication; initial releases may be submitted through the authenticated registry MCP and must still pass pinned-key CI verification.
- [ ] Align the requested Pulumi/managed-service maintenance plan with the Blacksmith-only GitHub CI decision, including managed CI configuration and safe retirement of the existing runner. Preserve the build-cluster egress guard, rootless BuildKit, volatile runtime directory and pinned tool update/recovery requirements for any remaining customer-build services.
- [ ] Until the existing trusted CI runner is retired, reserve/coordinate its CPU/RAM/cache usage with Kubernetes build capacity before enabling concurrent customer builds; the host service is outside the Kubernetes scheduler. Never route arbitrary imported source into the shared trusted CI cache.
- [ ] Replace persistent trusted CI state with disposable runners before admitting untrusted external source. Fork PRs remain excluded; CI qualification does not pass the public customer-build isolation gate.

### Verified plugin delivery status (2026-11-01)

- [x] Publish all nine native plugins and Managed CMS in independent Spitzli repositories, with immutable OCI images and original signed tags.
- [x] Admit and publish all ten new registry records; verify the existing Ed25519 key, exact manifest bytes and all23 declared signed assets. Keep provider bindings/customer activation separate.
- [x] Qualify and roll out the unchanged foreign-node retained-volume controller path; registry revision5 ready with unchanged Deployment UID, PV/PVC specs and other protected deployments.
- [ ] Renew the rejected registry-owner GitHub OAuth connection and set up repository-scoped publisher credentials/release hooks for future unattended versions. Initial publication used the existing authorized MCP publisher path.
- [ ] Qualify durable bounded retry/diagnostics for registry asset hashing after transient fetch failures; metadata acceptance alone does not imply signed integrity.
- [ ] Investigate the one unreproduced generic Runtime error in owned retained-update qualification; preserve error-code/callsite evidence without logging credentials.
- [x] Verify the qualified docs at docs.spitzli.cloud (2026-11-04). The canonical homepage and representative Neon/Git guides return200 with intended content and canonical links; Starter Kit is gone. On 2026-11-05, all44 navigation pages and their declared internal MDX link targets returned200 with their expected page titles; see [the full navigation check](docs/verification/2026-11-05-public-documentation-navigation.json). Exact provider-dashboard settings and complete page-body/source equivalence are not inferred from the serving check.
- [ ] Qualify distribution/bootstrap of the registry-server OCI image on replacement/future RUN nodes; the exact image is currently preloaded on all three existing nodes.

Do not rerun the old self-hosted registry-only CI jobs to change their historical
status: the Blacksmith-only owner instruction applies to new/rerun GitHub CI.
