import { gitWorkerAPI } from "../../src/modules/hosting/git-worker-api";
import { bindGitTarget } from "../../src/modules/hosting/git-targets";
import { reconcileGitRelease } from "../../src/modules/hosting/git-reconciliation";
import test from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { database } from "../../src/db/runtime";
import { hostingSchemaSQL } from "../../src/modules/hosting/schema";
import { gitDeploymentSchemaSQL } from "../../src/modules/hosting/git-deployment-schema";
import {
  executeGitDeployment,
  registerGitConnection,
  enrollGitWorker,
  claimGitBuild,
  prepareGitActionsArtifact,
  completeGitBuild,
  receiveGitEvent,
  processGitChecks,
  processGitEvents,
  claimGitRelease,
  completeGitRelease,
} from "../../src/modules/hosting/git-deployments";
import { createGitHubProvider } from "../../src/modules/hosting/git-github";
import type { HostingActor } from "@webdock/hosting-contracts";
const url = new URL(process.env.DATABASE_URL!);
if (
  !["localhost", "127.0.0.1"].includes(url.hostname) ||
  url.pathname != "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable local DB required");
const key = () => randomBytes(16).toString("hex");
test.after(() => database.end());
test("durable Git queue fences stale workers and tenant publication", async (t) => {
  await database.query(hostingSchemaSQL);
  await database.query(gitDeploymentSchemaSQL);
  await database.query(gitDeploymentSchemaSQL);
  // This dedicated disposable fixture must not lease work left by an interrupted prior run.
  await database.query(
    "UPDATE webdock_auth.git_connection SET state='disconnected' WHERE account_login='fixture'",
  );
  while (
    (
      await processGitEvents({
        resolveSource: async () => {
          throw Error("Inactive fixture");
        },
      } as unknown as ReturnType<typeof createGitHubProvider>)
    ).processed
  ) {}
  const user = (
    await database.query(
      `INSERT INTO webdock_auth."user"(id,name,email,"emailVerified",role,"twoFactorEnabled","mustChangePassword","createdAt","updatedAt") VALUES(webdock_auth.next_snowflake(),'Git test',$1,true,'operator',true,false,now(),now()) RETURNING id`,
      [`git-${key()}@example.invalid`],
    )
  ).rows[0].id;
  const session = (
    await database.query(
      `INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","createdAt","updatedAt") VALUES(webdock_auth.next_snowflake(),$1,$2,now()+interval '1 hour',now(),now()) RETURNING id`,
      [key(), user],
    )
  ).rows[0].id;
  const actor: HostingActor = {
    subject: user,
    sessionID: session,
    source: "studio",
    scopes: ["hosting:read", "hosting:write"],
  };
  const customer = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Git test') RETURNING id",
    )
  ).rows[0].id;
  const customerB = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Git test B') RETURNING id",
    )
  ).rows[0].id;
  const project = (
    await database.query(
      "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES(webdock_auth.next_snowflake(),'Git test',$1,'active') RETURNING id",
      [customer],
    )
  ).rows[0].id;
  await database.query(
    "INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,mode) VALUES($1,$2,'vercel','selfservice')",
    [project, customer],
  );
  const target = "prj_git" + key();
  await database.query(
    "INSERT INTO webdock_auth.hosting_vercel(customer_id,team_id,configuration_id,encrypted_token,projects) VALUES($1,$2,$3,'unused',$4)",
    [customer, "team_" + key(), "cfg_" + key(), JSON.stringify([target])],
  );
  await assert.rejects(
    bindGitTarget(
      actor,
      {
        action: "git.targets.bind",
        projectID: project,
        targetID: target,
        mode: "byok",
      },
      async () => {
        throw Error("provider denied");
      },
    ),
  );
  assert.equal(
    (
      await database.query(
        "SELECT count(*)::int n FROM webdock_auth.git_vercel_target WHERE project_id=$1",
        [project],
      )
    ).rows[0].n,
    0,
  );
  const targetBinding = await bindGitTarget(
    actor,
    {
      action: "git.targets.bind",
      projectID: project,
      targetID: target,
      mode: "byok",
    },
    async () => {},
  );
  assert.equal(targetBinding.targetID, target);
  const binding = {
    installationID: String(Date.now()),
    repositoryID: "987654321",
    accountID: "111",
    accountLogin: "fixture",
    owner: "fixture",
    name: "app",
    fullName: "fixture/app",
    defaultBranch: "main",
    permissions: { contents: "read", checks: "write" },
  };
  const connection = await registerGitConnection(actor, binding, customer);
  t.after(async () => {
    await database.query(
      "UPDATE webdock_auth.git_connection SET state='disconnected' WHERE id=$1",
      [connection.id],
    );
  });
  await assert.rejects(registerGitConnection(actor, binding, customerB));
  const fake = {
    resolveSource: async () => ({
      sha: "a".repeat(40),
      repositoryID: binding.repositoryID,
      repositoryName: binding.fullName,
      branch: "main",
    }),
  } as unknown as ReturnType<typeof createGitHubProvider>;
  let activeTarget = target;
  const call = (cmd: Record<string, unknown>) => executeGitDeployment(actor,
    ['git.source.get', 'git.builds.request'].includes(cmd.action) ? {targetID: activeTarget, ...cmd} : cmd, fake);
  const configured = await call({
    action: "git.source.configure",
    projectID: project,
    connectionID: connection.id,
    repositoryID: binding.repositoryID,
    branch: "main",
    rootDirectory: ".",
    recipe: "vercel",
    targetID: target,
    revision: 0,
    healthPath: "/api/health",
  });
  assert.equal(configured.revision, 1);
  assert.equal(configured.healthPath, "/api/health");
  // Two sources must coexist without changing the first application's release state.
  const secondTarget = 'prj_second' + key();
  await database.query('UPDATE webdock_auth.hosting_vercel SET projects=projects || $2::jsonb WHERE customer_id=$1', [customer, JSON.stringify([secondTarget])]);
  await bindGitTarget(actor, {action:'git.targets.bind',projectID:project,targetID:secondTarget,mode:'byok'}, async () => {});
  const second = await call({action:'git.source.configure',projectID:project,connectionID:connection.id,repositoryID:binding.repositoryID,branch:'main',rootDirectory:'apps/api',recipe:'vercel',targetID:secondTarget,revision:0,healthPath:'/api/health'});
  assert.notEqual(second.id, configured.id);
  assert.equal((await call({action:'git.source.get',projectID:project})).source.rootDirectory, '.');
  await assert.rejects(executeGitDeployment(actor, {action:'git.builds.request',projectID:project,idempotencyKey:key()}, fake), /Select an application/);
  const allSources = await call({action:'git.sources.list', projectID:project});
  assert.equal(allSources.docs.length, 2);
  const idem = key();
  const build = await call({
    action: "git.builds.request",
    projectID: project,
    idempotencyKey: idem,
  });
  assert.equal(
    (
      await call({
        action: "git.builds.request",
        projectID: project,
        idempotencyKey: idem,
      })
    ).id,
    build.id,
  );
  await assert.rejects(
    executeGitDeployment(
      { ...actor, scopes: ["hosting:read"] },
      {
        action: "git.builds.request",
        projectID: project,
        idempotencyKey: key(),
      },
      fake,
    ),
  );
  const org = (
    await database.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customer],
    )
  ).rows[0].organization_id;
  await database.query(
    "INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id,expires_at) VALUES($1,$2,$3,$4,now()+interval '10 minutes')",
    [session, user, customer, org],
  );
  await assert.rejects(
    call({
      action: "git.builds.cancel",
      projectID: project,
      buildID: build.id,
    }),
  );
  await database.query(
    "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1",
    [session],
  );
  for (const country of ["us", "USA", "", "ZZ"])
    await assert.rejects(
      enrollGitWorker(actor, {
        country,
        isolation: "microvm",
        evidence: "Verified isolation and operator location record",
        capacity: 1,
      }),
      /capacity/,
    );
  const unknownLocation = await enrollGitWorker(actor, {
    country: "ZZ",
    isolation: "microvm",
    evidence:
      "Disposable local KVM isolation verified; geographic location unverified",
    capacity: 1,
  });
  await database.query(
    "UPDATE webdock_auth.git_worker SET enabled=false WHERE id=$1",
    [unknownLocation.id],
  );
  const enrolled = await enrollGitWorker(actor, {
    country: "US",
    isolation: "microvm",
    evidence: "Disposable test verified isolation fixture",
    capacity: 1,
  });
  const first = await claimGitBuild(enrolled.credential);
  const checkLease = (generation: number) =>
    gitWorkerAPI(
      new Request(
        `https://auth.example/api/hosting/git/worker/builds/${first!.buildID}/lease?generation=${generation}`,
        {
          headers: { Authorization: `Bearer ${enrolled.credential}` },
        },
      ),
      ["builds", first!.buildID, "lease"],
    );
  assert.deepEqual(await (await checkLease(first!.generation)).json(), {
    active: true,
  });
  assert.equal((await checkLease(first!.generation + 1)).status, 409);
  assert.equal(first?.buildID, build.id);
  assert.equal(await claimGitBuild(enrolled.credential), null);
  await database.query(
    "UPDATE webdock_auth.git_build SET lease_until=now()-interval '1 second' WHERE id=$1",
    [build.id],
  );
  const next = await claimGitBuild(enrolled.credential);
  assert.ok(next!.generation > first!.generation);
  const completion = {
    buildID: build.id,
    generation: first!.generation,
    status: "succeeded" as const,
    logs: "fixture",
    artifact: {
      kind: "vercel" as const,
      digest: "sha256:" + "b".repeat(64),
      storageKey: `${customer}/${build.id}/${next!.generation}/${"b".repeat(64)}.tar`,
      sizeBytes: 10,
    },
  };
  await assert.rejects(completeGitBuild(enrolled.credential, completion));
  const done = await completeGitBuild(enrolled.credential, {
    ...completion,
    generation: next!.generation,
  });
  assert.equal(done.release?.status, "awaiting-approval");
  assert.equal(
    (
      await database.query(
        "SELECT publication_target FROM webdock_auth.git_release WHERE id=$1",
        [done.release!.id],
      )
    ).rows[0].publication_target.healthPath,
    "/api/health",
  );
  await assert.rejects(
    completeGitBuild(enrolled.credential, {
      ...completion,
      generation: next!.generation,
    }),
  );
  const approvals = await Promise.allSettled(
    [key(), key()].map((idempotencyKey) =>
      call({
        action: "git.releases.approve",
        projectID: project,
        releaseID: done.release!.id,
        revision: 1,
        idempotencyKey,
      }),
    ),
  );
  assert.equal(approvals.filter((r) => r.status === "fulfilled").length, 1);
  assert.equal(approvals.filter((r) => r.status === "rejected").length, 1);
  const approved = (
    approvals.find((r) => r.status === "fulfilled") as PromiseFulfilledResult<
      Awaited<ReturnType<typeof executeGitDeployment>>
    >
  ).value;
  assert.equal(approved.status, "queued");
  const otherWorker = await enrollGitWorker(actor, {
    country: "DE",
    isolation: "microvm",
    evidence: "Second independent verified fixture worker",
    capacity: 1,
  });
  assert.equal(await claimGitRelease(otherWorker.credential), null);
  const release = await claimGitRelease(enrolled.credential);
  assert.equal(release?.releaseID, approved.id);
  await database.query(
    "UPDATE webdock_auth.git_release SET provider_deployment_id='dpl_fixture' WHERE id=$1",
    [approved.id],
  );
  await assert.rejects(
    completeGitRelease(enrolled.credential, {
      releaseID: approved.id,
      generation: release!.generation,
      status: "needs-reconciliation",
      providerDeploymentID: "dpl_other",
    }),
  );
  await completeGitRelease(enrolled.credential, {
    releaseID: approved.id,
    generation: release!.generation,
    status: "needs-reconciliation",
  });
  assert.equal(
    (
      await database.query(
        "SELECT provider_deployment_id FROM webdock_auth.git_release WHERE id=$1",
        [approved.id],
      )
    ).rows[0].provider_deployment_id,
    "dpl_fixture",
  );
  assert.equal(await claimGitRelease(enrolled.credential), null);
  const uncertain = (
    await database.query(
      "SELECT revision FROM webdock_auth.git_release WHERE id=$1",
      [approved.id],
    )
  ).rows[0];
  const reconcile = {
    action: "git.releases.reconcile" as const,
    projectID: project,
    releaseID: approved.id,
    revision: uncertain.revision,
  };
  const pending = await reconcileGitRelease(actor, reconcile, async () => ({
    status: "needs-reconciliation",
  }));
  assert.equal(pending.status, "needs-reconciliation");
  const failed = await reconcileGitRelease(actor, reconcile, async () => ({
    status: "failed",
    providerDeploymentID: "dpl_fixture",
  }));
  assert.equal(failed.status, "failed");
  const event = {
    deliveryID: key(),
    event: "push",
    installationID: binding.installationID,
    repositoryID: binding.repositoryID,
    branch: "main",
    sha: "a".repeat(40),
  };
  const poison = { ...event, deliveryID: key() };
  await receiveGitEvent(poison);
  await processGitEvents({
    resolveSource: async () => {
      throw Error("provider unavailable");
    },
  } as unknown as ReturnType<typeof createGitHubProvider>);
  assert.equal(
    (
      await database.query(
        "SELECT attempts FROM webdock_auth.git_receipt WHERE delivery_id=$1",
        [poison.deliveryID],
      )
    ).rows[0].attempts,
    1,
  );
  await receiveGitEvent(event);
  assert.equal((await processGitEvents(fake)).queued, 2);
  assert.equal((await database.query("SELECT count(DISTINCT source_id)::int AS n FROM webdock_auth.git_build WHERE project_id=$1 AND idempotency_key LIKE $2", [project, 'webhook:' + event.deliveryID + '%'])).rows[0].n, 2);
  await call({action:'git.source.configure',projectID:project,connectionID:connection.id,repositoryID:binding.repositoryID,branch:'main',rootDirectory:'apps/api',recipe:'vercel',targetID:secondTarget,revision:second.revision,enabled:false});
  assert.equal((await receiveGitEvent(event)).duplicate, true);
  await receiveGitEvent({
    deliveryID: key(),
    event: "installation",
    installationID: binding.installationID,
    action: "suspend",
  });
  assert.equal(await claimGitBuild(enrolled.credential), null);
  await assert.rejects(
    call({
      action: "git.builds.request",
      projectID: project,
      idempotencyKey: key(),
    }),
  );
  const checkFailure = {
    reportCheck: async () => {
      throw Error("provider unavailable");
    },
  } as unknown as ReturnType<typeof createGitHubProvider>;
  await processGitChecks(checkFailure);
  const pendingChecks = (
    await database.query(
      "SELECT count(*)::int n FROM webdock_auth.git_check_outbox WHERE version>processed_version",
    )
  ).rows[0].n;
  assert.ok(pendingChecks > 0);
  // A second tenant cannot inspect builds or mutate a source using another project ID.
  await database.query(
    "UPDATE webdock_auth.\"user\" SET role='user' WHERE id=$1",
    [user],
  );
  const orgB = (
    await database.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customerB],
    )
  ).rows[0].organization_id;
  await database.query(
    'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
    [orgB, user],
  );
  for (const command of [
    { action: "git.builds.get", projectID: project, buildID: build.id },
    { action: "git.source.get", projectID: project },
    { action: "git.releases.list", projectID: project },
    { action: "git.builds.cancel", projectID: project, buildID: build.id },
  ])
    await assert.rejects(call(command));
  await database.query(
    "UPDATE webdock_auth.\"user\" SET role='operator' WHERE id=$1",
    [user],
  );
  await registerGitConnection(actor, binding, customer);
  await call({
    action: "git.source.configure",
    projectID: project,
    connectionID: connection.id,
    repositoryID: binding.repositoryID,
    branch: "main",
    rootDirectory: ".",
    recipe: "vercel",
    targetID: target,
    revision: 1,
    healthPath: "/api/ready",
    autoPublish: true,
    buildEnvironment: [{ name: "BUILD_FIXTURE", value: "build-private-value" }],
  });
  assert.equal(
    (
      await database.query(
        "SELECT publication_target FROM webdock_auth.git_release WHERE id=$1",
        [done.release!.id],
      )
    ).rows[0].publication_target.healthPath,
    "/api/health",
  );
  const publicSource = await call({
    action: "git.source.get",
    projectID: project,
  });
  assert.deepEqual(publicSource.source.buildEnvironmentNames, [
    "BUILD_FIXTURE",
  ]);
  assert.ok(!JSON.stringify(publicSource).includes("build-private-value"));
  const automatic = await call({
    action: "git.builds.request",
    projectID: project,
    idempotencyKey: key(),
  });
  const automaticLease = await claimGitBuild(enrolled.credential);
  assert.equal(automaticLease?.buildID, automatic.id);
  assert.equal(
    automaticLease!.buildEnvironment.BUILD_FIXTURE,
    "build-private-value",
  );
  const automaticResult = await completeGitBuild(enrolled.credential, {
    buildID: automatic.id,
    generation: automaticLease!.generation,
    status: "succeeded",
    logs: "token=build-private-value",
    artifact: {
      kind: "vercel",
      digest: "sha256:" + "c".repeat(64),
      storageKey: `${customer}/${automatic.id}/${automaticLease!.generation}/${"c".repeat(64)}.tar`,
      sizeBytes: 123,
    },
  });
  assert.equal(automaticResult.release?.status, "queued");
  assert.equal(
    (
      await call({
        action: "git.builds.get",
        projectID: project,
        buildID: automatic.id,
      })
    ).logs,
    "token=[redacted]",
  );
  // A provider receipt only becomes ready after an independent healthy observation.
  await database.query(
    "UPDATE webdock_auth.git_release SET status='failed' WHERE id=$1",
    [approved.id],
  );
  const publishing = await claimGitRelease(enrolled.credential);
  assert.equal(publishing?.releaseID, automaticResult.release?.id);
  await assert.rejects(
    completeGitRelease(enrolled.credential, {
      releaseID: publishing!.releaseID,
      generation: publishing!.generation,
      status: "ready",
      providerDeploymentID: "dpl_test",
    }),
  );
  await database.query(
    "UPDATE webdock_auth.git_release SET provider_deployment_id='dpl_test',publication_started_at=now() WHERE id=$1",
    [publishing!.releaseID],
  );
  const healthy = await completeGitRelease(enrolled.credential, {
    releaseID: publishing!.releaseID,
    generation: publishing!.generation,
    status: "ready",
    providerDeploymentID: "dpl_test",
    healthVerified: true,
    region: "fra1",
  });
  const rollback = await call({
    action: "git.releases.rollback",
    projectID: project,
    releaseID: healthy.id,
    revision: healthy.revision,
    idempotencyKey: key(),
  });
  assert.equal(rollback.status, "queued");
  assert.notEqual(rollback.id, healthy.id);
  assert.equal(
    (
      await database.query(
        "SELECT publication_target FROM webdock_auth.git_release WHERE id=$1",
        [rollback.id],
      )
    ).rows[0].publication_target.healthPath,
    "/api/ready",
  );
  const preflight = await claimGitRelease(enrolled.credential);
  assert.equal(preflight?.releaseID, rollback.id);
  const retryable = await completeGitRelease(enrolled.credential, {
    releaseID: rollback.id,
    generation: preflight!.generation,
    status: "needs-reconciliation",
  });
  assert.equal(retryable.status, "awaiting-approval");
  assert.equal(
    (
      await database.query(
        "SELECT approved_by FROM webdock_auth.git_release WHERE id=$1",
        [rollback.id],
      )
    ).rows[0].approved_by,
    null,
  );
  // Retention and rollback must share project serialization, even while both requests wait.
  await database.query(
    "UPDATE webdock_auth.git_release SET status='failed' WHERE id=$1",
    [retryable.id],
  );
  await database.query(
    "UPDATE webdock_auth.git_release SET created_at=now()-interval '40 days' WHERE id=$1",
    [healthy.id],
  );
  await database.query(
    "UPDATE webdock_auth.git_artifact SET created_at=now()-interval '40 days' WHERE id=$1",
    [healthy.artifactID],
  );
  for (let i = 0; i < 3; i++)
    await database.query(
      "INSERT INTO webdock_auth.git_release(customer_id,project_id,artifact_id,status,source_revision,connection_generation,environment_revision,subject,environment_identity,source_id) SELECT customer_id,project_id,$2,'ready',source_revision,connection_generation,environment_revision,subject,environment_identity,source_id FROM webdock_auth.git_release WHERE id=$1",
      [healthy.id, approved.artifactID],
    );
  const blocker = await database.connect();
  await blocker.query("BEGIN");
  await blocker.query(
    "SELECT id FROM webdock_auth.git_source WHERE project_id=$1 FOR UPDATE",
    [project],
  );
  let rollbackFinished = false,
    retentionFinished = false;
  const racingRollback = call({
    action: "git.releases.rollback",
    projectID: project,
    releaseID: healthy.id,
    revision: healthy.revision,
    idempotencyKey: key(),
  })
    .then(
      (value) => ({ ok: true as const, value }),
      () => ({ ok: false as const }),
    )
    .finally(() => {
      rollbackFinished = true;
    });
  const racingRetention = gitWorkerAPI(
    new Request("https://auth.example/api/hosting/git/worker/retention", {
      method: "POST",
      headers: {
        authorization: "Bearer " + enrolled.credential,
        "Content-Type": "application/json",
      },
      body: "{}",
    }),
    ["retention"],
  ).finally(() => {
    retentionFinished = true;
  });
  try {
    let waiting = 0;
    for (let i = 0; i < 100; i++) {
      await blocker.query("SELECT pg_stat_clear_snapshot()");
      waiting = (
        await blocker.query(
          "SELECT count(*)::int n FROM pg_stat_activity WHERE wait_event_type='Lock' AND query LIKE '%webdock_auth.git_source%'",
        )
      ).rows[0].n;
      if (waiting >= 2) break;
      await new Promise((resolve) => setTimeout(resolve, 10));
    }
    assert.ok(
      waiting >= 2,
      "rollback and retention both wait for the same source lock",
    );
    assert.equal(rollbackFinished, false);
    assert.equal(retentionFinished, false);
  } finally {
    await blocker.query("COMMIT");
    blocker.release();
  }
  const [rollbackRace, retentionRace] = await Promise.all([
    racingRollback,
    racingRetention,
  ]);
  assert.equal(retentionRace.status, 200);
  const cleanup = await retentionRace.json();
  const artifactState = (
    await database.query(
      "SELECT retained,storage_key FROM webdock_auth.git_artifact WHERE id=$1",
      [healthy.artifactID],
    )
  ).rows[0];
  if (rollbackRace.ok) {
    assert.equal(artifactState.retained, true);
    assert.ok(!cleanup.storageKeys.includes(artifactState.storage_key));
  } else {
    assert.equal(artifactState.retained, false);
    assert.ok(cleanup.storageKeys.includes(artifactState.storage_key));
  }
  const sourceRow = (
    await database.query(
      "SELECT id FROM webdock_auth.git_source WHERE project_id=$1",
      [project],
    )
  ).rows[0];
  await assert.rejects(
    database.query(
      "INSERT INTO webdock_auth.git_build(customer_id,project_id,source_id,source_sha,source_revision,connection_generation,environment_revision,subject,idempotency_key) VALUES($1,$2,$3,$4,1,1,1,$5,$6)",
      [customerB, project, sourceRow.id, "b".repeat(40), user, key()],
    ),
  );
  for (let i = 0; i < 10; i++)
    await call({
      action: "git.builds.request",
      projectID: project,
      idempotencyKey: key(),
    });
  await assert.rejects(
    call({
      action: "git.builds.request",
      projectID: project,
      idempotencyKey: key(),
    }),
  );
  await call({
    action: "git.source.configure",
    projectID: project,
    connectionID: connection.id,
    repositoryID: binding.repositoryID,
    branch: "main",
    rootDirectory: ".",
    recipe: "vercel",
    targetID: target,
    revision: 2,
  });
  const recovered = await call({
    action: "git.builds.request",
    projectID: project,
    idempotencyKey: key(),
  });
  assert.equal(recovered.status, "queued");
  assert.equal(
    (
      await database.query(
        "SELECT count(*)::int n FROM webdock_auth.git_build WHERE project_id=$1 AND source_id=$2 AND failure_code='SOURCE_CHANGED'",
        [project, configured.id],
      )
    ).rows[0].n,
    11,
  );
  const foreignProject = (
    await database.query(
      "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES(webdock_auth.next_snowflake(),'Foreign git fixture',$1,'active') RETURNING id",
      [customerB],
    )
  ).rows[0].id;
  await database.query(
    "INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,mode) VALUES($1,$2,'vercel','selfservice')",
    [foreignProject, customerB],
  );
  await assert.rejects(
    database.query(
      "INSERT INTO webdock_auth.git_source(customer_id,project_id,connection_id,repository_id,branch,root_directory,recipe,target_id) VALUES($1,$2,$3,$4,'main','.','vercel',$5)",
      [customer, foreignProject, connection.id, binding.repositoryID, target],
    ),
    (error: unknown) =>
      (error as { constraint?: string }).constraint ===
      "git_source_project_customer_fk",
  );
  await call({action:'git.source.configure',projectID:project,connectionID:connection.id,repositoryID:binding.repositoryID,branch:'main',rootDirectory:'.',recipe:'vercel',targetID:target,revision:3,enabled:false});
  // Registry-only writes cannot create an application operation; their failures must be retryable.
  const cluster = (
    await database.query(
      "INSERT INTO webdock_auth.hosting_cluster(name,provider,country,region,location_evidence) VALUES('Git registry fixture','k3s','DE','fixture','Local test only') RETURNING id",
    )
  ).rows[0].id;
  await database.query(
    "UPDATE webdock_auth.hosting_project SET provider='k3s',cluster_id=$2,namespace=$3 WHERE project_id=$1",
    [project, cluster, "wd-" + project],
  );
  const app = (
    await database.query(
      "INSERT INTO webdock_auth.hosting_app(project_id,name,spec) VALUES($1,'git-fixture',$2) RETURNING id",
      [project, JSON.stringify({ image: "fixture", template: "custom" })],
    )
  ).rows[0].id;
  await call({
    action: "git.source.configure",
    projectID: project,
    connectionID: connection.id,
    repositoryID: binding.repositoryID,
    branch: "main",
    rootDirectory: ".",
    recipe: "dockerfile",
    targetID: app,
    revision: 0,
  });
  activeTarget = app;
  const ociBuild = await call({
    action: "git.builds.request",
    projectID: project,
    idempotencyKey: key(),
  });
  const ociLease = await claimGitBuild(enrolled.credential);
  assert.equal(ociLease?.buildID, ociBuild.id);
  const ociDone = await completeGitBuild(enrolled.credential, {
    buildID: ociBuild.id,
    generation: ociLease!.generation,
    status: "succeeded",
    logs: "",
    artifact: {
      kind: "oci",
      digest: "sha256:" + "d".repeat(64),
      storageKey: `${customer}/${ociBuild.id}/${ociLease!.generation}/${"d".repeat(64)}.tar`,
      sizeBytes: 123,
    },
  });
  await call({
    action: "git.releases.approve",
    projectID: project,
    releaseID: ociDone.release!.id,
    revision: 1,
    idempotencyKey: key(),
  });
  const ociPublication = await claimGitRelease(enrolled.credential);
  const spoof = await gitWorkerAPI(
    new Request(
      "https://auth.example/api/hosting/git/worker/release-complete",
      {
        method: "POST",
        headers: {
          authorization: "Bearer " + enrolled.credential,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({
          releaseID: ociPublication!.releaseID,
          generation: ociPublication!.generation,
          status: "ready",
          operationID: "123",
          healthVerified: true,
        }),
      },
    ),
    ["release-complete"],
  );
  assert.equal(spoof.status, 409);
  assert.equal(
    (await spoof.json()).error,
    "Publication requires observed health and verified target location.",
  );
  assert.equal(
    (
      await database.query(
        "SELECT status,operation_id FROM webdock_auth.git_release WHERE id=$1",
        [ociPublication!.releaseID],
      )
    ).rows[0].status,
    "deploying",
  );
  assert.equal(
    (
      await database.query(
        "SELECT operation_id FROM webdock_auth.git_release WHERE id=$1",
        [ociPublication!.releaseID],
      )
    ).rows[0].operation_id,
    null,
  );
  await database.query(
    "UPDATE webdock_auth.git_release SET publication_started_at=now() WHERE id=$1",
    [ociPublication!.releaseID],
  );
  const registryFailure = await completeGitRelease(enrolled.credential, {
    releaseID: ociPublication!.releaseID,
    generation: ociPublication!.generation,
    status: "needs-reconciliation",
  });
  assert.equal(registryFailure.status, "awaiting-approval");
  await database.query(
    "UPDATE webdock_auth.git_release SET status='needs-reconciliation' WHERE id=$1",
    [registryFailure.id],
  );
  const registryRecovery = await reconcileGitRelease(actor, {
    action: "git.releases.reconcile",
    projectID: project,
    releaseID: registryFailure.id,
    revision: registryFailure.revision,
  });
  assert.equal(registryRecovery.status, "failed");
  // A changed source must not make an already-started publication impossible to observe.
  await database.query(
    "UPDATE webdock_auth.git_release SET status='needs-reconciliation',publication_started_at=now() WHERE id=$1",
    [registryFailure.id],
  );
  await call({action:'git.source.configure',projectID:project,connectionID:connection.id,repositoryID:binding.repositoryID,branch:'main',rootDirectory:'.',recipe:'dockerfile',targetID:app,revision:1,enabled:false});
  const replacementApp = (
    await database.query(
      "INSERT INTO webdock_auth.hosting_app(project_id,name,spec) VALUES($1,'replacement',$2) RETURNING id",
      [project, JSON.stringify({ image: "fixture", template: "custom" })],
    )
  ).rows[0].id;
  await call({
    action: "git.source.configure",
    projectID: project,
    connectionID: connection.id,
    repositoryID: binding.repositoryID,
    branch: "main",
    rootDirectory: ".",
    recipe: "dockerfile",
    targetID: replacementApp,
    revision: 0,
  });
  const staleRelease = (
    await database.query(
      "SELECT revision FROM webdock_auth.git_release WHERE id=$1",
      [registryFailure.id],
    )
  ).rows[0];
  const recoveredAfterChange = await reconcileGitRelease(
    actor,
    {
      action: "git.releases.reconcile",
      projectID: project,
      releaseID: registryFailure.id,
      revision: staleRelease.revision,
    },
    async (_db, original) => {
      assert.equal(original.target_id, app);
      assert.equal(original.recipe, "dockerfile");
      return { status: "failed" };
    },
  );
  assert.equal(recoveredAfterChange.status, "failed");
  assert.equal(
    (
      await database.query(
        "SELECT count(*)::int n FROM webdock_auth.git_release WHERE project_id=$1 AND status IN ('deploying','needs-reconciliation')",
        [project],
      )
    ).rows[0].n,
    0,
  );
  activeTarget = replacementApp;
  const replacementBuild = await call({
    action: "git.builds.request",
    projectID: project,
    idempotencyKey: key(),
  });
  const replacementLease = await claimGitBuild(enrolled.credential);
  assert.equal(replacementLease?.buildID, replacementBuild.id);
  const replacementDone = await completeGitBuild(enrolled.credential, {
    buildID: replacementBuild.id,
    generation: replacementLease!.generation,
    status: "succeeded",
    logs: "",
    artifact: {
      kind: "oci",
      digest: "sha256:" + "e".repeat(64),
      storageKey: `${customer}/${replacementBuild.id}/${replacementLease!.generation}/${"e".repeat(64)}.tar`,
      sizeBytes: 123,
    },
  });
  await call({
    action: "git.releases.approve",
    projectID: project,
    releaseID: replacementDone.release!.id,
    revision: 1,
    idempotencyKey: key(),
  });
  const replacementPublication = await claimGitRelease(enrolled.credential);
  assert.equal(replacementPublication?.releaseID, replacementDone.release!.id);
  await completeGitRelease(enrolled.credential, {
    releaseID: replacementPublication!.releaseID,
    generation: replacementPublication!.generation,
    status: "failed",
  });
  // Actions receipts preserve connection authority and cannot be claimed by source executors.
  await database.query(
    "UPDATE webdock_auth.git_connection SET permissions=permissions || jsonb_build_object('actions','read') WHERE id=$1",
    [connection.id],
  );
  await database.query(
    "UPDATE webdock_auth.git_source SET build_environment_encrypted=NULL WHERE project_id=$1",
    [project],
  );
  const actionsSource = await call({
    action: "git.source.configure",
    projectID: project,
    connectionID: connection.id,
    repositoryID: binding.repositoryID,
    branch: "main",
    rootDirectory: ".",
    recipe: "dockerfile",
    targetID: replacementApp,
    revision: 1,
    buildProvider: "github-actions",
  });
  assert.equal(actionsSource.buildProvider, "github-actions");
  await assert.rejects(
    call({
      action: "git.builds.request",
      projectID: project,
      idempotencyKey: key(),
    }),
    /configured workflow/,
  );
  const publisher = await enrollGitWorker(actor, {
    country: "DE",
    isolation: "artifact-only",
    evidence: "Fixture trusted artifact publisher; no source execution",
    capacity: 1,
  });
  const artifact = {
    id: "123456",
    name: "webdock-" + "a".repeat(40),
    sizeBytes: 123,
    digest: "sha256:" + "f".repeat(64),
    downloadURL: "https://productionresultssa0.blob.core.windows.net/fixture",
  };
  const verified = {
    runID: "123",
    runAttempt: 1,
    workflowID: "456",
    workflowPath: ".github/workflows/webdock.yml",
    repositoryID: binding.repositoryID,
    sha: "a".repeat(40),
    branch: "main",
    createdAt: new Date(Date.now() + 1000).toISOString(),
    artifact,
  };
  const actionsProvider = {
    ...fake,
    verifyActionsRun: async () => verified,
  } as unknown as ReturnType<typeof createGitHubProvider>;
  const actionsEvent = {
    deliveryID: key(),
    event: "workflow_run",
    installationID: binding.installationID,
    repositoryID: binding.repositoryID,
    branch: "main",
    sha: "a".repeat(40),
    runID: "123",
    runAttempt: 1,
    workflowPath: verified.workflowPath,
    conclusion: "success",
    action: "completed",
  };
  await receiveGitEvent(actionsEvent);
  assert.equal((await processGitEvents(actionsProvider)).queued, 1);
  assert.equal(
    (
      await database.query(
        "SELECT state FROM webdock_auth.git_connection WHERE id=$1",
        [connection.id],
      )
    ).rows[0].state,
    "active",
  );
  await receiveGitEvent({ ...actionsEvent, deliveryID: key() });
  assert.equal((await processGitEvents(actionsProvider)).queued, 0);
  assert.equal(await claimGitBuild(enrolled.credential), null);
  const importJob = await claimGitBuild(publisher.credential);
  assert.equal(importJob?.buildProvider, "github-actions");
  const importRow = (
    await database.query(
      "SELECT b.*,s.build_provider,s.workflow_path,s.artifact_prefix,s.branch,s.repository_id,c.installation_id FROM webdock_auth.git_build b JOIN webdock_auth.git_source s ON s.id=b.source_id JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE b.id=$1",
      [importJob!.buildID],
    )
  ).rows[0];
  assert.equal(importRow.actions_provenance.artifact.downloadURL, undefined);
  assert.deepEqual(
    await prepareGitActionsArtifact(importRow, actionsProvider),
    artifact,
  );
  await assert.rejects(
    prepareGitActionsArtifact(importRow, {
      ...actionsProvider,
      verifyActionsRun: async () => ({
        ...verified,
        artifact: { ...artifact, id: "999" },
      }),
    } as unknown as ReturnType<typeof createGitHubProvider>),
  );
  await assert.rejects(
    prepareGitActionsArtifact(importRow, {
      ...actionsProvider,
      resolveSource: async () => ({ sha: "b".repeat(40) }),
    } as unknown as ReturnType<typeof createGitHubProvider>),
  );
  await assert.rejects(
    completeGitBuild(
      publisher.credential,
      {
        buildID: importJob!.buildID,
        generation: importJob!.generation,
        status: "succeeded",
        logs: "",
        artifact: {
          kind: "oci",
          digest: "sha256:" + "f".repeat(64),
          storageKey: `${customer}/${importJob!.buildID}/${importJob!.generation}/${"f".repeat(64)}.tar`,
          sizeBytes: 123,
        },
      },
      {
        ...actionsProvider,
        resolveSource: async () => ({ sha: "b".repeat(40) }),
      } as unknown as ReturnType<typeof createGitHubProvider>,
    ),
  );
  const imported = await completeGitBuild(
    publisher.credential,
    {
      buildID: importJob!.buildID,
      generation: importJob!.generation,
      status: "succeeded",
      logs: "",
      artifact: {
        kind: "oci",
        digest: "sha256:" + "f".repeat(64),
        storageKey: `${customer}/${importJob!.buildID}/${importJob!.generation}/${"f".repeat(64)}.tar`,
        sizeBytes: 123,
      },
    },
    actionsProvider,
  );
  assert.equal(imported.release!.status, "awaiting-approval");
  await call({
    action: "git.releases.approve",
    projectID: project,
    releaseID: imported.release!.id,
    revision: 1,
    idempotencyKey: key(),
  });
  assert.equal(await claimGitRelease(enrolled.credential), null);
  const importedRelease = await claimGitRelease(publisher.credential);
  assert.equal(importedRelease?.releaseID, imported.release!.id);
  await completeGitRelease(publisher.credential, {
    releaseID: importedRelease!.releaseID,
    generation: importedRelease!.generation,
    status: "failed",
  });
  await assert.rejects(
    call({
      action: "git.source.configure",
      projectID: project,
      connectionID: connection.id,
      repositoryID: binding.repositoryID,
      branch: "main",
      rootDirectory: ".",
      recipe: "dockerfile",
      targetID: replacementApp,
      revision: 2,
      buildProvider: "github-actions",
      buildEnvironment: [{ name: "BUILD_SECRET", value: "no-export" }],
    }),
    /build variables/,
  );
  await receiveGitEvent({ ...actionsEvent, deliveryID: key(), runID: "124" });
  assert.equal(
    (
      await processGitEvents({
        ...actionsProvider,
        verifyActionsRun: async () => ({
          ...verified,
          runID: "124",
          createdAt: "2000-01-01T00:00:00Z",
        }),
      } as unknown as ReturnType<typeof createGitHubProvider>)
    ).queued,
    0,
  );
  // Delayed completion for the same SHA cannot make an older run the desired build.
  for (const older of [
    { runID: "122", createdAt: verified.createdAt },
    {
      runID: "125",
      createdAt: new Date(Date.parse(verified.createdAt) - 100).toISOString(),
    },
  ]) {
    await receiveGitEvent({
      ...actionsEvent,
      deliveryID: key(),
      runID: older.runID,
    });
    assert.equal(
      (
        await processGitEvents({
          ...actionsProvider,
          verifyActionsRun: async () => ({ ...verified, ...older }),
        } as unknown as ReturnType<typeof createGitHubProvider>)
      ).queued,
      0,
    );
    assert.equal(
      (
        await database.query(
          "SELECT latest_build_id FROM webdock_auth.git_source WHERE project_id=$1 AND target_id=$2",
          [project, activeTarget],
        )
      ).rows[0].latest_build_id,
      importJob!.buildID,
    );
  }
  for (const override of [
    { runAttempt: 2 },
    { workflowPath: ".github/workflows/other.yml" },
    { sha: "b".repeat(40) },
  ]) {
    await receiveGitEvent({ ...actionsEvent, ...override, deliveryID: key() });
    assert.equal((await processGitEvents(actionsProvider)).queued, 0);
  }
  // GitHub revocation prevents new writes, but must still allow historical observation.
  await database.query(
    "UPDATE webdock_auth.git_release SET status='needs-reconciliation' WHERE id=$1",
    [registryFailure.id],
  );
  const currentConnection = (
    await database.query(
      "SELECT revision FROM webdock_auth.git_connection WHERE id=$1",
      [connection.id],
    )
  ).rows[0];
  await call({
    action: "git.connections.disconnect",
    customerID: customer,
    connectionID: connection.id,
    revision: currentConnection.revision,
  });
  const recoveredAfterDisconnect = await reconcileGitRelease(
    actor,
    {
      action: "git.releases.reconcile",
      projectID: project,
      releaseID: registryFailure.id,
      revision: recoveredAfterChange.revision,
    },
    async (_db, original) => {
      assert.equal(original.target_id, app);
      return { status: "failed" };
    },
  );
  assert.equal(recoveredAfterDisconnect.status, "failed");
  const oldOperation = (
    await database.query(
      "INSERT INTO webdock_auth.hosting_operation(subject,customer_id,project_id,app_id,action,idempotency_key,payload_hash,status,target_revision) VALUES($1,$2,$3,$4,'apps.update',$5,'fixture','succeeded',1) RETURNING id",
      [actor.subject, customer, project, app, key()],
    )
  ).rows[0].id;
  await database.query(
    "UPDATE webdock_auth.hosting_app SET observed_revision=2,status='ready' WHERE id=$1",
    [app],
  );
  await database.query(
    "UPDATE webdock_auth.git_release SET status='needs-reconciliation',operation_id=$2 WHERE id=$1",
    [registryFailure.id, oldOperation],
  );
  const superseded = await reconcileGitRelease(actor, {
    action: "git.releases.reconcile",
    projectID: project,
    releaseID: registryFailure.id,
    revision: recoveredAfterDisconnect.revision,
  });
  assert.equal(superseded.status, "superseded");
  await database.query(
    "UPDATE webdock_auth.git_release SET status='needs-reconciliation',publication_target=NULL WHERE id=$1",
    [registryFailure.id],
  );
  await database.query(gitDeploymentSchemaSQL);
  await assert.rejects(
    reconcileGitRelease(
      actor,
      {
        action: "git.releases.reconcile",
        projectID: project,
        releaseID: registryFailure.id,
        revision: superseded.revision,
      },
      async () => {
        throw Error("Unproven target must never be observed");
      },
    ),
    /original publication target could not be verified/,
  );
  await database.query(
    "UPDATE webdock_auth.git_release SET status='failed' WHERE id=$1",
    [registryFailure.id],
  );
  // Leave no available work for subsequent test fixtures.
  await database.query(
    "UPDATE webdock_auth.git_connection SET state='disconnected' WHERE id=$1",
    [connection.id],
  );
});
