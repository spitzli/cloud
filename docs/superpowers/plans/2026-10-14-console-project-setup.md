# Guided console project setup implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans for native execution, or superpowers:subagent-driven-development if the user selects delegation. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Deliver a real project setup flow that connects Git or a supported container image, provisions or reuses PostgreSQL and native Mail, wires application access, and shows resumable progress and observed readiness.

**Architecture:** Extend the existing hosting command service and PostgreSQL journals. Scope Git sources to application targets, persist service bindings, and execute confirmed setup revisions through a restricted hosting worker. The Console composes those services without provisioning from page renders or carrying credentials through the browser.

**Tech Stack:** Existing TypeScript, Zod, PostgreSQL, Bun API, Node 24, Next.js 16.3.8, React 19.2.8, Python hosting/build agents and gettext catalogs. No additional framework or provider.

**Spec:** `docs/superpowers/specs/2026-10-14-console-project-setup-design.md`, approved by the user's “go” on 2026-10-14.

**Status:** Plan for review. No implementation steps have run. Recommended execution: native, sequentially in the current isolated T3 worktree, with a final independent review. Tasks share migration, authorization and runtime contracts; they must not be implemented concurrently against changing interfaces.

## Global constraints

These requirements are copied from the approved specification:

- Diese Spezifikation betrifft Webdocks vorhandene Console, API und verwaltete Kubernetes-Infrastruktur. Sie führt keinen neuen Hostinganbieter ein.
- Die erste Lieferung arbeitet mit dem vorhandenen Produktionskontext; sie täuscht keine Staging-Isolation vor.
- Die neue Oberfläche erweitert keine Rollen stillschweigend.
- Ein erkanntes Datenbankpaket löst keine Bestellung aus.
- Runtime-Secrets gelangen weder in Git noch in Statusantworten oder Logs.
- Build-Secrets bleiben separat und müssen ausdrücklich ausgewählt werden.
- Deaktivierung behält Daten; Löschen ist ein separater Vorgang.
- Die UI wird nicht als abgeschlossene Lieferung mit lediglich weiterführenden Links auf getrennte Formulare gewertet.

Keep `docs/architecture/mail-deployment-decision.md` authoritative: outgoing-only never provisions Stalwart; explicitly enabled incoming Mail uses one retained CE instance per customer. Keep the existing production instance, encryption keys, volumes and replica ownership intact. Do not run live migrations, deploy, create paid resources or send real email as part of local implementation verification.

## Review focus

1. An old release is completed after a second application's source changes: source identity must remain exact through claim, publication, cancellation and rollback (task 1).
2. Mail key creation commits immediately before worker interruption: resume must recover the encrypted same key, not create another unrecoverable key (task 4).
3. A database password contains URL delimiters, or its CA rotates while the application is stopped: preserve TLS verification and observe the new binding revision before reporting connected (task 3).
4. A changed database offer or revoked membership is encountered after a saved confirmation: require renewed consent or block before new side effects, retaining completed resources (task 5).
5. OAuth return, browser reload or a stale tab submits an old setup: retain the draft, reject mismatched revisions and never re-order a service (tasks 6 and 7).

## Working baseline and verification harness

The worktree was initially clean; the prior commit `24cca21` contains only the specification. Recheck `git status --short` before execution. Use the current worktree; do not create a second worktree or modify the main checkout.

Dependencies are installed. The login shell defaults to Node 22, while the project requires Node 24. Use the installed Node 24 path for verification:

```bash
export PATH=/home/newt/.nvm/versions/node/v24.21.0/bin:$PATH
node --version
npm run test -w @spitzli/hosting-contracts
npm test
```

Use `apps/api/scripts/fixture.ts` and `fixture-rls.ts` for disposable PostgreSQL. Never borrow production environment files. The API test runner refuses unspecified database targets and blocks external fetches. Register new native integration suites in `apps/api/scripts/test.ts`; otherwise they would run incorrectly as unit tests. Extend its `rls` set for the new restricted-role suite.

Before editing Next.js code read the installed guides:

- `node_modules/next/dist/docs/01-app/02-guides/forms.md`
- `node_modules/next/dist/docs/01-app/01-getting-started/05-server-and-client-components.md`
- `node_modules/next/dist/docs/01-app/02-guides/server-and-client-boundary.md`

Use append-only offline migrations. The next available migration at planning time is `0022`; recheck before adding files. Update both schema builders used by fixtures and offline SQL. Never edit an already applied migration or run DDL from a request.

## Task 1: Git sources and releases belong to an application

**Files**

- Modify `packages/hosting-contracts/src/git-deployments.ts` and its existing `.test.ts`.
- Modify `apps/api/src/modules/hosting/git-deployment-schema.ts`, `git-deployments.ts`, `git-publication.ts`, `git-worker-api.ts`, `git-reconciliation.ts`, `git-vercel-publication.ts` and `authorization.ts`.
- Add `apps/api/drizzle/0022_git_application_sources.sql`.
- Extend `apps/api/tests/ported-auth/git-deployments.test.ts`, `git-publication-head.test.ts`, `git-vercel-publication.test.ts` and `git-worker-api.test.ts`.
- Update existing Git Console callers in `apps/admin/src/app/(console)/(portal)/hosting/projects/[projectID]/git/deployments.tsx` and the build detail page.

**Interfaces**

Keep `git.source.configure.targetID` required. Add optional `targetID` to `git.source.get` and `git.builds.request`; add optional `sourceID` to build/release list filters. Add `git.sources.list(projectID)` returning all safe source views. Source views expose `id`, `targetID`, `repositoryName` and the current existing fields. Builds and releases expose `sourceID` and `targetID`.

An absent target is compatible only for zero/one source; more than one yields a public 409 `Select an application for this Git operation.` Release mutations resolve the source through the release's persisted `source_id`, never through an optional caller target.

- [x] **Write contract and cross-source regression tests.** In the existing durable Git queue test, use its existing `actor`, `project`, `target`, `binding`, `connection`, `call` and `key` fixtures. After configuring the first source, bind a second verified Vercel target through the same local provider adapter, then add:

```ts
const secondTarget = 'prj_second' + key();
await database.query(
  'UPDATE webdock_auth.hosting_vercel SET projects=projects || $2::jsonb WHERE customer_id=$1',
  [customer, JSON.stringify([secondTarget])],
);
await bindGitTarget(actor, {
  action: 'git.targets.bind', projectID: project,
  targetID: secondTarget, mode: 'byok',
}, async () => {});
const second = await call({
  action: 'git.source.configure', projectID: project,
  connectionID: connection.id, repositoryID: binding.repositoryID,
  branch: 'main', rootDirectory: 'apps/api', recipe: 'vercel',
  targetID: secondTarget, revision: 0, healthPath: '/api/health',
});
assert.notEqual(second.id, configured.id);
assert.equal((await call({
  action: 'git.source.get', projectID: project, targetID: target,
})).source.rootDirectory, '.');
await assert.rejects(call({
  action: 'git.builds.request', projectID: project, idempotencyKey: key(),
}), /Select an application/);
```

Add explicit targets to subsequent legacy fixture requests once two sources exist. Also assert one-source legacy requests still work; completing source A after editing B does not supersede A; a rollback of B cannot select A's target. Test migration on populated sources/builds/artifacts/releases and reapplication of the fixture schema.

- [x] **Run the regression and observe the expected failure.** Use the local fixture with Node 24 and the existing no-network preload. The second configuration currently conflicts with the project's sole source. The contract test currently rejects the target selector.

- [x] **Implement exact source identity throughout.** Add release `source_id`, backfilled through `git_release.artifact_id → git_artifact.build_id → git_build.source_id`, then validate a composite FK containing customer/project. Replace project-only source uniqueness with `(project_id,target_id)`. Do not backfill by source revision: revisions can coincide across applications.

```sql
ALTER TABLE webdock_auth.git_release ADD COLUMN source_id varchar;
UPDATE webdock_auth.git_release r SET source_id=b.source_id
FROM webdock_auth.git_artifact a
JOIN webdock_auth.git_build b ON b.id=a.build_id
WHERE r.artifact_id=a.id;
ALTER TABLE webdock_auth.git_release ALTER COLUMN source_id SET NOT NULL;
ALTER TABLE webdock_auth.git_source DROP CONSTRAINT git_source_project_id_key;
ALTER TABLE webdock_auth.git_source
  ADD CONSTRAINT git_source_project_target_key UNIQUE(project_id,target_id);
ALTER TABLE webdock_auth.git_release
  ADD CONSTRAINT git_release_source_owner_fk
  FOREIGN KEY(source_id,customer_id,project_id)
  REFERENCES webdock_auth.git_source(id,customer_id,project_id);
```

Use `s.id=b.source_id` for build joins and `s.id=r.source_id` for release joins. Scope desired-release clearing, obsolete work, automatic-policy authorization and release serialization to that source. Keep cluster/quota locks intact. Include the exact source ID and revision in automatic publication authorization so one enabled source cannot authorize another disabled source.

Search all callers with `rg -n 'git_source|git.source|git.builds.request' apps packages` and update artifact import, worker API and build-time environment selection as well as the main service. Existing Vercel publishing must remain intact.

- [x] **Verify and commit.** Run contracts, all existing Git regression tests and Admin/API typechecks. Commit only task files with `feat: scope git deployments to applications`.

## Task 2: Prepare an application without deploying a placeholder

**Files**

- Modify `packages/hosting-contracts/src/index.ts`, `apps/api/src/modules/hosting/apps.ts`, `schema.ts`, `git-deployments.ts`, `git-publication.ts`, and `apps/api/src/modules/mcp/hosting-mcp.ts`.
- Add `apps/api/drizzle/0023_application_drafts.sql` and `apps/api/tests/application-drafts.integration.test.ts`.
- Update consumers of `HostingAppView` in `apps/admin/src/components/hosting/` and `apps/admin/src/app/(console)/(portal)/hosting/apps/[appID]/`.

**Interfaces**

Add `apps.prepare` accepting project, name, resource configuration, subscription revision and idempotency key. It returns `{appID, revision, lifecycle:'draft'}`. Add a persisted `lifecycle` value `draft | deployed` to `hosting_app`; existing rows are `deployed`. A draft's `spec` contains resource/port/healthcheck settings with no image. Runtime paths must parse the existing full `appSpecSchema` before generating an operation.

```ts
type PreparedAppSpec = Omit<AppSpec, 'image' | 'template'>;
type ApplicationSpecView =
  | { lifecycle: 'draft'; spec: PreparedAppSpec }
  | { lifecycle: 'deployed'; spec: AppSpec };
```

Keep the application's stable ID when a first artifact is published. Drafts expose `Awaiting first deployment`, not `Ready`. Prepare creates no Kubernetes object, no running healthcheck template, and no billable storage allocation. Quota is checked again at publication; no capacity promise is made while waiting for a build.

- [x] **Write a real database test.** Reuse the isolated hosting actor/project fixture setup from the existing hosting integration tests, with a unique fixture customer. Assert the returned app ID exists once after repeated `apps.prepare`, its lifecycle is draft, and no apply operation exists:

```sql
SELECT count(*)::int AS n FROM webdock_auth.hosting_operation
WHERE app_id=$1 AND action='apps.create';
```

Assert start/restart/scale of a draft is rejected, editing the draft cannot deploy it, and publication without a valid artifact cannot transition it. Add a successful first-artifact test and an application deletion test that does not attempt to delete nonexistent runtime resources. Register the suite as a native integration test.

- [x] **Run it red**, then implement the discriminated lifecycle and explicit prepare command. Keep environment storage available for drafts; deny executor claims for drafts. Publish the immutable image and transition lifecycle in the same authorized transaction that queues the first real apply operation. Render draft-safe UI in all view consumers; never assert `image` exists on an un-narrowed view.

- [x] **Run the draft tests, existing application lifecycle tests and typechecks.** Commit `feat: prepare applications before their first release`.

## Task 3: PostgreSQL service bindings and runtime access

**Files**

- Add `packages/hosting-contracts/src/service-bindings.ts` and its `.test.ts`; export its schemas/types through `index.ts`.
- Add `apps/api/src/modules/hosting/service-bindings.ts`, `database-binding.ts` and `binding-runtime.ts`.
- Modify `database-credentials.ts`, `database-observations.ts`, `apps.ts`, `service.ts`, `agent-api.ts`, `environment.ts`, and `apps/api/src/modules/mcp/hosting-mcp.ts`.
- Add `apps/api/drizzle/0024_application_service_bindings.sql`; extend `apps/api/src/db/database-rls-schema.ts` and `database-cluster-rls.ts`.
- Add `apps/hosting-agent/service_bindings.py` and `test_service_bindings.py`; integrate with `executor.py`, `observer.py` and `daemon.py` capability reporting.
- Add `apps/api/tests/service-bindings.integration.test.ts` and `service-bindings-rls.integration.test.ts` to the appropriate test-runner sets.

**Interfaces**

Commands: `services.list({projectID,appID?})`, `services.bind({projectID,appID,service,revision,idempotencyKey})`, `services.disconnect({projectID,bindingID,revision,idempotencyKey})`. All use the standard discriminated hosting command schema.

```ts
type ServiceSelection =
  | { kind: 'postgres'; databaseID: string; variable: string }
  | { kind: 'mail'; connectionID: string; domainID: string; variable: string };
type ServiceBindingView = {
  id: string; projectID: string; appID: string;
  service: ServiceSelection; revision: number; observedRevision: number;
  state: 'pending' | 'connected' | 'blocked' | 'disconnecting' | 'disconnected';
  reason: string | null; credentialRevision: number | null;
};
```

`hosting_service_binding` stores customer/project/app, typed service IDs, desired/observed revision, state, managed variable names, encrypted secret material and operation ID. Enforce ownership with FKs where possible and transactionally for polymorphic service references. One active owner per app/managed variable, enforced with a partial unique index. No credential values in the safe view.

- [x] **Write URL and access tests before implementation.** New helper `databaseConnectionURL(input)` takes `{host,port,database,username,password,caPath}` and returns a URL; use the standard URL API.

```ts
const value = databaseConnectionURL({
  host: 'db.internal', port: 6432, database: 'portal',
  username: 'wd_app_42', password: 'a@b:/?#%',
  caPath: '/var/run/webdock/services/42/ca.crt',
});
const parsed = new URL(value);
assert.equal(decodeURIComponent(parsed.password), 'a@b:/?#%');
assert.equal(parsed.searchParams.get('sslmode'), 'verify-full');
assert.equal(parsed.searchParams.get('sslrootcert'), '/var/run/webdock/services/42/ca.crt');
```

Integration tests create two customer fixtures and verify foreign database/app rejection, same-project requirement, read-only rejection, stopped/paused database blockers, manual variable collisions and repeat binding idempotency. Assert all status JSON excludes passwords and migration roles. Run these tests red.

- [x] **Implement database binding on the server.** Extract the confirmed credential reader from `displayDatabaseCredentials` into an internal transaction helper; retain the existing Studio-only display gate. For binding, additionally require a ready endpoint and same project, derive only the application role, and use the observed CA and credential revision. Store encrypted values in the same transaction as the binding. Do not call the browser credential-reveal endpoint or return credentials through Server Actions.

Runtime payloads carry a versioned, bounded service-binding section: IDs/revisions, CA files at derived paths, and permitted backend endpoint identities. Agent capability `serviceBindingsVersion:1` gates binding application. Mount CA Secrets read-only. Resolve network destinations from trusted backend metadata; clients cannot submit arbitrary IPs, namespaces or CIDRs. Reject cross-cluster bindings until a verified route exists.

NetworkPolicy selects the specific app and only the database destination/port. Use an additive policy with ownership labels; never change existing default-deny policies. A disconnected binding removes only its own rules, mounts and managed variables. The original database application password is shared by the existing database role, so disconnect removes this app's access rather than rotating the database role for every other consumer.

- [x] **Implement rotation and observation.** Database observation marks bindings pending when confirmed credential/CA revisions change. Reconcile managed Secrets with the existing app revision and queue the required rollout for deployed apps; drafts keep a pending binding until first deployment. The agent reports the binding revision actually applied and observed. Only that revision may become connected. A stopped app remains pending until restarted and observed.

- [x] **Test network isolation and lifecycle.** Python tests inspect generated mounts/policies and reject malformed service packets. Local cluster integration performs a TLS query with the real app role, rejects invalid CA/hostname and verifies a second customer's app cannot reach the endpoint. Disconnect one of two apps and verify the other retains access. Add deletion cleanup hooks to the existing project/app lifecycle, retaining database data. Run API/RLS tests and agent tests; commit `feat: bind applications to managed postgres`.

## Task 4: Native Mail bindings with recoverable key creation

**Files**

- Add `apps/api/src/modules/hosting/mail-binding.ts`.
- Modify `service-bindings.ts`, `binding-runtime.ts`, `apps/api/src/modules/mail/product/api-keys.ts`, `access.ts`, and the Mail key schema/RLS policy definitions.
- Add `apps/api/drizzle/0025_application_mail_bindings.sql` and `apps/api/tests/application-mail-binding.integration.test.ts`.
- Extend `apps/hosting-agent/service_bindings.py` tests for the native API destination.

**Interfaces**

Use the task 3 `mail` selection. `bindApplicationMail(actor,input)` is internal to `services.bind` and returns `ServiceBindingView`. Its key is always `sending_access`, bound to the selected `domainID`. Inject the configured key variable (default `WEBDOCK_MAIL_API_KEY`) and `WEBDOCK_MAIL_API_URL`; neither contains Turbo account credentials.

Extract the transaction body of `createMailKey` into `createMailKeyInTransaction(db,actor,input)`. Existing public callers continue using `createMailKey`, with the same validation and rate limits. The binding path uses one verified session scope and one transaction for Mail permission checks, key hash insertion, encrypted key storage and binding insertion. This is not a generic worker credential-minting bypass.

- [x] **Write the interruption/security regressions.** Exercise the real service against the local fixture. After a successful binding transaction, discard the response and repeat the same command. Assert same binding ID, one key row, same encrypted recovered value, and successful `authorizeMailKey` for that value. Assert the key cannot manage domains/keys, cannot send for another domain, and is absent from safe status responses. Verify unverified domain, foreign connection, revoked connection and read-only actor are rejected. Run red.

- [x] **Implement atomic binding and explicit network access.** Authorize native Mail management before minting. Check sender-domain readiness and connection readiness. Bind the app to the platform's configured native Mail API endpoint; derive the corresponding trusted cluster destination. If no verified egress route exists, block with setup guidance rather than opening unrestricted HTTPS egress. Do not alter incoming-Mail activation or provision Stalwart.

- [x] **Implement disconnect and rotation.** Revoke only the binding-owned key, remove its encrypted material and app configuration, and reconcile the app. Rotation creates the replacement and records its revision atomically; retain/revoke the old key according to observed rollout state so retries cannot orphan credentials. Manual key revocation marks the binding blocked. Customer Mail suspension remains authoritative.

- [x] **Verify and commit.** Test interruption after commit, rollback before commit, two applications sharing one domain, connection suspension and isolated key revocation. Assert Stalwart instance/volume counts unchanged for outgoing-only setup. Run the native Mail/API regression suites and commit `feat: bind native mail sending to applications`.

## Task 5: Confirmed setup journal and restricted execution

**Files**

- Add `packages/hosting-contracts/src/project-setup.ts` and `project-setup.test.ts`; export through `index.ts`.
- Add `apps/api/src/modules/hosting/project-setup.ts`, `setup-preview.ts`, `setup-execution.ts` and `setup-worker.ts`.
- Add `apps/api/drizzle/0026_project_setup.sql` and `0027_setup_worker_access.sql`.
- Modify `apps/api/src/worker.ts`, `jobs.ts`, `db/readiness.ts`, `db/database-rls-schema.ts`, `db/database-context.ts`, and `modules/hosting/service.ts`.
- Integrate commands in `apps/api/src/modules/mcp/hosting-mcp.ts` and validate existing Studio/REST dispatcher coverage.
- Extend `modules/hosting/authorization.ts`, database order primitives and task 3/4 binding primitives with narrow authorized internal entrypoints; do not bypass their validations.
- Add `apps/api/tests/project-setup.integration.test.ts`, `project-setup-rls.integration.test.ts` and `project-setup-worker.integration.test.ts`; register them in `scripts/test.ts`.
- Modify `apps/api/src/modules/lifecycle/project-deletion.ts` and `project-deletion-database.ts` for in-flight setup fencing.

**Interfaces and contracts**

```ts
type SetupState = 'draft' | 'awaiting-confirmation' | 'running'
  | 'blocked' | 'failed' | 'ready';
type SetupStep = 'application' | 'database' | 'bindings' | 'source'
  | 'build' | 'release' | 'readiness';
type SetupSource =
  | { kind: 'git'; connectionID: string; repositoryID: string;
      branch: string; rootDirectory: string; dockerfile: string;
      buildProvider: 'isolated' | 'github-actions'; workflowPath: string;
      artifactPrefix: string }
  | { kind: 'image'; reference: string; registryCredentialID?: string }
  | { kind: 'template'; template: 'healthcheck' };
type SetupDatabase =
  | { mode: 'none' }
  | { mode: 'existing'; databaseID: string; variable: string }
  | { mode: 'new'; name: string; planRevisionID: string; variable: string };
type SetupConfiguration = {
  name: string; source: SetupSource; spec: PreparedAppSpec;
  database: SetupDatabase;
  mail: { connectionID: string; domainID: string; variable: string } | null;
  autoPublish: boolean;
};
type SetupView = {
  id: string; projectID: string; revision: number; state: SetupState;
  configuration: SetupConfiguration; environmentNames: string[];
  appID: string | null; databaseID: string | null;
  steps: { name: SetupStep; state: 'pending' | 'running' | 'blocked'
    | 'failed' | 'succeeded' | 'skipped'; operationID: string | null;
    reason: string | null }[];
};
```

All schemas are strict and bounded; reuse existing ID, branch, path, resource and environment validators. Do not accept caller-supplied customer IDs, prices, readiness, network routes, step completion or observed revisions.

Commands:

- `setup.save({projectID,setupID?,revision,configuration,environment?,idempotencyKey}) → SetupView`; creation expects revision 0. Secret patches are encrypted and absent from the returned view.
- `setup.get({projectID,setupID}) → SetupView` and `setup.list({projectID,page,limit})`.
- `setup.preview({projectID,setupID,revision}) → SetupPreview`.
- `setup.confirm({projectID,setupID,revision,previewID,acceptedOfferIDs,idempotencyKey}) → SetupView`.
- `setup.resume({projectID,setupID,revision,idempotencyKey}) → SetupView`.

`SetupPreview` includes a server-generated ID, expiration, exact commit/digest, setup revision, subscription revision, resolved service/credential revisions, available capabilities, structured blockers and price rows with `new | existing | metered` classification. It embeds the existing database offer's actual price, terms, backup status and expiration. Store the canonical preview hash server-side; do not trust a browser hash or re-resolve a moving branch at execution.

`hosting_setup` owns the encrypted draft, validated configuration, preview, confirming subject and approved action scope. `hosting_setup_step` stores stable attempt identity, operation/resource IDs, state and claim lease/generation. Composite ownership constraints cover customer/project. Claim/completion updates require the unexpired generation; an obsolete worker cannot advance the setup.

- [x] **Write preview/confirmation regressions.** Test the real command service on local PostgreSQL: create/save/read draft; secrets are absent from every view; changed revision invalidates preview; exact accepted offer IDs are required; repeated confirm returns the same setup/resources; expired or repriced offer blocks before order creation. Test that a fresh preview invalidates the old one even when the user configuration is unchanged. Use deterministic provider fixtures for pinned SHAs/digests.

For the testable confirmation helper in `setup-preview.ts`, define `assertSetupConfirmation(input)` with the exact shape below and run its tests red:

```ts
assert.throws(() => assertSetupConfirmation({
  setupRevision: 3, submittedRevision: 2,
  previewID: 'current', submittedPreviewID: 'current',
  expiresAt: new Date('2026-10-14T12:00:00Z'),
  now: new Date('2026-10-14T11:00:00Z'),
  requiredOfferIDs: ['42'], acceptedOfferIDs: ['42'],
}), /changed/);
assert.throws(() => assertSetupConfirmation({
  setupRevision: 3, submittedRevision: 3,
  previewID: 'current', submittedPreviewID: 'current',
  expiresAt: new Date('2026-10-14T12:00:00Z'),
  now: new Date('2026-10-14T11:00:00Z'),
  requiredOfferIDs: ['42'], acceptedOfferIDs: [],
}), /price and terms/);
```

- [x] **Implement transactions and public commands.** Dispatch `setup.*` through `executeHosting` before the existing generic transaction, following the existing `git.*` pattern. Lock project and setup revision, validate live actor/preview permissions, run offer validation and persist confirmation before scheduling execution. Saving a new draft uses a stable idempotency key, so a lost creation response cannot create duplicates. Once running, changes require a new preview; do not mutate confirmed configuration underneath queued work.

```ts
if (cmd.action.startsWith('setup.')) {
  return executeProjectSetup(actor, cmd);
}
```

`executeProjectSetup(actor:HostingActor,cmd:HostingCommand)` parses its own command union and returns the above safe view/preview. It does not execute builds or poll infrastructure inside a request.

- [x] **Implement restricted worker admission.** Add explicit worker domain `hosting` and job `hosting.setup` to process ownership validation, scheduler names, SQL constraints and readiness checks. Preserve the worker-only pool profile: no business/bootstrap/admin connection URL in that process.

Use a signed system scope for setup dispatch, then a signed scope containing exact customer/project/setup/revision for execution. Extend the shared database scope type if required. New SQL policies validate the signed scope, live lease and persisted approval. They grant only the reads and writes needed for that setup; dispatcher functions return candidate IDs, not all tenant secrets.

`assertSetupExecution(db,claim)` checks that the approving user remains verified, active and authorized; customer/project remain active; the setup is confirmed at that revision; and the lease generation is current. Use a distinct internal `SetupExecution` token returned by that check, never a fabricated session or `git-policy` actor. Its type and construction stay inside `setup-execution.ts`. Public commands and worker adapters share internal transaction primitives after their respective real authorization checks. Reuse the task 4 Mail key transaction for worker execution only after this exact setup grants Mail management and the connection/domain are revalidated.

- [x] **Execute one bounded transition per claim.** `runSetupBatch({limit,deadline}) → {processed,failed}` claims at most `limit` setups and yields at `deadline`. The sequence is:

  1. Prepare application and persist its ID.
  2. Accept the confirmed database order, or reuse the selected DB; wait for observed readiness. Billing starts according to the existing order service.
  3. Create/reconcile database and Mail bindings with stable per-step keys. Persist identity immediately; do not wait for app observation yet.
  4. Configure the exact Git source or verified image/template.
  5. Request build for the preview's pinned commit; wait on its existing journal. A new branch head never silently substitutes for the approved commit.
  6. Publish using existing release policy. When production approval is required, expose the exact artifact's approval action and remain blocked; setup confirmation is not approval of an artifact that did not exist.
  7. Observe app revision/health and binding revisions; then mark ready.

Optional steps become skipped. Each step key is derived from `setupID`, confirmed revision and step name. Polls inspect stored operation IDs rather than reissuing mutations. Waiting for a resource is pending/running, not a failed job. Uncertain side effects enter reconciliation; deterministic validation failures become blocked with an actionable reason. Retryable infrastructure failures preserve IDs and release the lease with bounded backoff.

- [x] **Exercise crash, revocation and deletion races.** Kill the worker after database order acceptance and before setup-step completion; restart and verify one order/database. Let a lease expire and ensure the old worker's completion is rejected. Revoke membership between steps and verify no new key/build/rollout. Begin project deletion while a step is claimed; use the existing project deletion lock/fence to prevent subsequent provisioning and report retained resources. Test app deletion disconnects app-owned credentials and policies without deleting shared DB/Mail resources.

- [x] **Verify restricted roles and commit.** Run new integration/RLS suites, worker boot/drain tests and existing database order tests. Commit `feat: orchestrate resumable project setup`.

## Task 6: Source inspection, capability checks and OAuth return

**Files**

- Add `apps/api/src/modules/hosting/setup-source.ts` and `setup-capabilities.ts`.
- Extend `git-github.ts`, `git-oauth.ts`, `packages/hosting-contracts/src/git-oauth.ts`, and `apps/api/src/modules/integrations/github/byok-routes.ts`.
- Add `apps/api/drizzle/0028_setup_oauth_return.sql`.
- Extend `apps/api/tests/ported-auth/git-github.test.ts`, `git-oauth.test.ts`, and `apps/api/tests/ported-admin/git-connection-routes.test.ts`.
- Add `apps/api/tests/setup-source.test.ts` and `setup-source.integration.test.ts`.
- For a supported custom Dockerfile path, update `git-deployments.ts`, build packet contracts, `apps/build-worker/guest.py`, `worker.py` and their existing tests.

**Interfaces**

`setup.options({projectID})` returns permitted hosting choices, Git connections/repos, ready compatible DBs, plan catalog, native Mail connections/domains and capability-derived blockers. It also returns `canManage`, `canAssignHosting` and supported source kinds; these are informational and never replace mutation authorization.

`setup.inspectSource({projectID,setupID,revision})` returns `{sourceSHA,dockerfile,port,rootDirectory,suggestedServices,warnings}`. Values are suggestions only. `setup-source.ts` exposes pure `inspectRepositoryFiles(files:Record<string,string>)` for bounded metadata interpretation; provider reads always occur at the resolved SHA.

- [x] **Write inspection/path tests and run them red.** The helper recognizes explicit Dockerfile `EXPOSE` and package metadata, ignores instructions in comments/README, and never invokes a shell:

```ts
const result = inspectRepositoryFiles({
  Dockerfile: 'FROM node:24\nEXPOSE 3000\n',
  'package.json': JSON.stringify({dependencies:{pg:'8.16.3'}}),
});
assert.equal(result.port, 3000);
assert.deepEqual(result.suggestedServices, ['postgres']);
assert.equal(result.dockerfile, 'Dockerfile');
assert.equal(inspectRepositoryFiles({Dockerfile:'EXPOSE $PORT'}).port, null);
```

Provider tests reject `..`, absolute paths, symlinks, oversized responses and repository permission removal. Limit to ten files and 256 KiB combined, with a 64 KiB per-file cap and request timeout. Unknown framework or dynamic configuration produces no guessed required value.

- [x] **Implement source verification with existing trust boundaries.** Use existing installation-scoped provider tokens and verified repository identities. Check the selected branch's SHA and root directory; preserve them in the preview. If supporting a custom Dockerfile path, resolve it inside the checkout, reject symlink escapes and pass it as a fixed subprocess argument to the build worker. GitHub Actions sources require the existing configured workflow/artifact contract; do not claim to install a workflow without repository write access.

The image lane supports cached immutable images and the existing operator-installed, per-project registry allowlist. Read `apps/hosting-agent/registry.py`: managed pull is already implemented and must be preserved. Only resolve tags on configured approved registries with explicit project-scoped access; persist the digest and verify target architecture. Never fetch an arbitrary user-supplied registry URL through privileged control-plane networking. Unsupported registry imports remain visible blockers. A customer-provided registry credential is encrypted separately and never injected into application env or returned in options.

- [x] **Bind OAuth continuation to the saved draft.** Add optional `setupID` to `git.oauth.begin`, validate ownership and store it in `git_flow`. Derive the return path server-side from the stored project/setup; accept no arbitrary return URL. Retain the existing actor/session/state/expiry/one-use checks. OAuth success, cancellation and failure return to the saved draft with a safe status message; selection and retry remain possible.

- [x] **Verify and commit.** Test default branches other than `main`, private repo revocation, different OAuth session, forged setup ID, replay, external redirect input, tag drift after preview and unsupported agent capabilities. Commit `feat: inspect setup sources and preserve github drafts`.

## Task 7: Console wizard, service management and project overview

**Files**

- Add `apps/admin/src/lib/project-setup-actions.ts` and `project-setup-view.ts`.
- Add `apps/admin/src/components/hosting/setup-wizard.tsx`, `setup-source-fields.tsx`, `setup-service-fields.tsx`, `setup-review.tsx`, `setup-progress.tsx`, `project-overview.tsx`, and `service-bindings.tsx`.
- Add `apps/admin/src/app/(console)/(portal)/hosting/projects/[projectID]/setup/page.tsx`, `setup/[setupID]/page.tsx`, `overview/page.tsx` and `services/page.tsx`.
- Update `apps/admin/src/lib/actions.ts`, `components/hosting/project-context.tsx`, `project-applications.tsx`, and `lib/workspace-navigation.ts`.
- Update existing project creation/overview pages, Git configuration pages and application overview/settings pages under `(console)`.
- Extend existing Console CSS with scoped setup classes; reuse current forms, tables, notices and responsive tokens.
- Add `apps/admin/tests/project-setup.test.ts`, `project-setup-render.test.ts` and extend navigation/i18n tests.
- Extract and translate through `packages/i18n/scripts/extract.py`, `locales/de/` and `scripts/compile.py`.

**Interfaces and route behavior**

`setup/page.tsx` loads authorized options and creates no draft during GET. The first explicit form action calls `setup.save` and redirects to `setup/[setupID]`. `saveSetup`, `previewSetup`, `confirmSetup` and `resumeSetup` in the Server Action module call the existing `hostingCall` with validated command schemas. Each returns a structured safe result; authentication failures preserve the draft and use existing login return handling.

`setup-wizard.tsx` owns local unsaved input and step navigation; `SetupView` from task 5 remains durable authority. Source, Application, Services and Review are the editable stages. Saved state survives back/forward and OAuth. Progress displays the persistent step results and existing release approval/operation/log links. Secret inputs clear on successful save; the UI retains names only.

`project-setup-view.ts` exports `setupStage(state)` returning `edit | review | progress` and `canConfirmSetup({blockers,pending,requiredOfferIDs,acceptedOfferIDs})`. These are used by both rendered controls and tests; server checks remain independent.

- [x] **Write interaction tests and run red.** Use the existing happy-dom/React harness. Verify changing source or service invalidates the displayed preview, backing up keeps fields, selecting an existing DB hides order pricing, and the submit button remains disabled until every actual offer is accepted:

```ts
assert.equal(canConfirmSetup({
  blockers: [], pending: false,
  requiredOfferIDs: ['42'], acceptedOfferIDs: [],
}), false);
assert.equal(canConfirmSetup({
  blockers: [], pending: false,
  requiredOfferIDs: ['42'], acceptedOfferIDs: ['42'],
}), true);
assert.equal(setupStage('blocked'), 'progress');
```

Render tests assert a draft app has no public URL/ready badge, disabled features have reasons, permission-limited users have no mutation forms, and secrets never appear in HTML. Simulate failed saves and stale revisions without discarding input. Cover pressing Enter and repeated submit as well as clicks.

- [x] **Build the guided form with existing components.** Keep one primary action per stage, labelled inputs, keyboard-operable controls, linked field errors and an `aria-live` progress region. Optional services use explicit selectors; no prechecked paid purchase. Account/repository names remain untranslated. Default branch follows the selected repository until deliberately overridden. Domain verification and native Mail onboarding use existing APIs/forms inside the saved setup context; do not mislabel a link as a connected service.

Operator-only hosting assignment uses the existing `projects.create`/`projects.update` service and shows applicable choices before assignment. Nonoperators get a retained draft and a concrete blocker. Incoming Mail uses a separate explicit activation action in the Services area, not the application's outgoing-Mail checkbox.

- [x] **Connect project and application navigation.** New project creation redirects to `/hosting/projects/{id}/setup`; edits retain their current location. Add overview/services/setup links to project navigation. Keep the current base applications route working to avoid breaking old links. Project overview renders all authorized apps with pagination, exact source and observed status; services render only actual binding records. An application's source/build links include its exact target/source selector. Existing Vercel projects retain their existing configuration lane.

Add service binding/disconnect controls to the app detail page, showing resulting configuration changes and pending rollout. Project-level **Add service** can invoke the existing database/native Mail service forms and then connect to selected applications. Do not silently order a duplicate database or enable incoming Mail.

- [x] **Translate and verify.** Keep English `msgid` source with complete German catalog entries. Run extraction, compilation, catalog checks, Admin tests/typecheck/lint and then a Next build. Commit `feat: guide project setup in the console`.

## Task 8: Complete-flow verification, upgrade safety and delivery

**Files**

- Add `apps/api/tests/project-setup-e2e.integration.test.ts` using the task 5/6 local fixtures and actual command service.
- Add `docs/verification/2026-10-14-console-project-setup.md` with exact executed checks and outcomes.
- Add product usage documentation to the established repository docs location, following `docs/AGENTS.md`; explain actual supported sources, service costs, incomplete prerequisites, retry and data retention.
- Keep this plan's checkboxes current; do not mark capabilities verified without evidence.

- [x] **Run an upgrade scenario before broad acceptance.** Apply new migrations to a disposable copy of the old schema containing existing app, source, build, release and database rows. Verify IDs/history/data are retained, old single-source API calls still work, and adding a second app requires exact source selection. Verify migration failure rolls back instead of silently dropping history. Check offline RLS installation recognizes every new table.

- [x] **Run two complete local scenarios.** (1) Git with two applications from different subdirectories, a new paid-offer fixture DB, and native Mail key restricted to a fixture domain; (2) supported immutable image with an existing database. Use local provider/SMTP fixtures, no external mail. Exercise build failure and resume, source disconnect, Mail domain blocked, database CA change, deleted app and interrupted worker. Assert resource counts/IDs in the real database, not only mock call counts.

- [x] **Run the project's required checks with Node 24.** Start the disposable API and restricted-role fixtures through their scripts before running database suites. Do not conceal skipped integration tests as passes.

```bash
npm test
npm run test -w @spitzli/hosting-contracts
npm run test -w @spitzli/i18n
npm run catalog:check -w @spitzli/i18n
npm run check -w @spitzli/admin
npm run check -w @spitzli/api
npm run lint -w @spitzli/admin
npm run build:admin
npm run test:api
npm run test:rls -w @spitzli/api
python3 -m unittest discover -s apps/hosting-agent -p 'test_*.py'
python3 -m unittest discover -s apps/build-worker -p 'test_*.py'
git diff --check
```

- [x] **Verify in the shared T3 browser.** Call `preview_status`; if closed, call `preview_open`, then navigate to the local Console. Use a disposable signed-in fixture and the real local API. Verify desktop and mobile German/English flows, source selection, field persistence, OAuth continuation, explicit paid order, progress, error/resume, app logs and binding management. Record screenshots of review and project overview. If authentication/infrastructure is unavailable, report that precise unverified part; do not substitute a static mock and claim end-to-end completion.

- [x] **Review the whole change against the spec.** Check all ten acceptance items, especially Git target isolation, restricted worker access, secret redaction, exact cost consent, unchanged outgoing/incoming Mail distinction and retained volumes. For native execution use the required final independent reviewer; resolve material findings and rerun only affected checks. No additional agent delegation before execution method approval.

- [x] **Record and deliver the verified result.** State implemented behavior, tests, live activation prerequisites and any residual limitations. Link modified files and verification evidence. Production migration/deployment and live paid-resource acceptance are separate operational actions. If a PR is created or worked on, register its URL immediately with T3 `link_pull_request` and verify registration before finishing.

## Coverage and self-review

| Approved requirement | Owning task |
| --- | --- |
| Multiple apps and independent Git sources/releases | 1, 2 |
| Prepare Git application without placeholder runtime | 2 |
| Real database access, TLS, credentials, network and rotation | 3 |
| Restricted native Mail access and no outgoing Stalwart allocation | 4 |
| Durable drafts, exact consent, idempotency, retries and live permissions | 5 |
| Source metadata, default branch, pinned source, supported image paths | 6 |
| OAuth resume, unified setup, project/app/service navigation | 6, 7 |
| Costs, readiness, accessibility and translations | 5, 7, 8 |
| Deletion fences, retention, cross-tenant and restricted-role tests | 3–5, 8 |
| Existing Vercel behavior and migrated history | 1, 7, 8 |

Execution order is 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8. Unit/regression tests are written before their implementation; each task finishes its focused verification before dependent code starts. No optional `webdock.yaml`, preview environments, provider migration or unsupported worker runtime is introduced in this delivery.

## Execution outcome

Completed locally on 2026-10-14. Task 8 composes existing database/Mail/Git fixtures and new setup integration scenarios rather than duplicating provisioning infrastructure. Exact evidence, review fixes and unverified live infrastructure are recorded in `docs/verification/2026-10-14-console-project-setup.md`. Production activation is separate from this implementation.
