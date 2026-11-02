import { configureFullInstallation } from "../../src/full-installation";
import { requireHostingCommand } from "../../src/installation";
configureFullInstallation();
import { createManagedAppsHandler } from "../../src/modules/hosting/plugin-apps";
import { createPluginApps } from "@webdock/core/plugin-apps";
import {
  saveInventory,
  claimByokOperation,
  completeByokOperation,
} from "../../src/modules/hosting/byok";
import { authorizeHosting } from "../../src/modules/hosting/authorization";
import { hostingRegistryLockSQL } from "../../src/modules/hosting/registry-access";
import test from "node:test";
import { subtest } from "../helpers/node-test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import { createIdentity } from "../../src/modules/auth/bootstrap";
import { hostingSchemaSQL } from "../../src/modules/hosting/schema";
import { executeHosting } from "../../src/modules/hosting/service";
import {
  consumeEnrollment,
  issueEnrollment,
  reportCluster,
  authenticateAgent,
} from "../../src/modules/hosting/clusters";
import { reserveHosting } from "../../src/modules/hosting/reservations";
import {
  claimOperation,
  completeOperation,
} from "../../src/modules/hosting/operations";
import {
  managePlans,
  getPlans,
  getTenantPlan,
} from "../../src/modules/plans/plans";
import {
  normalizeHostingAllowances,
  type HostingActor,
} from "@webdock/hosting-contracts";
const url = new URL(process.env.DATABASE_URL!);
if (
  !["localhost", "127.0.0.1"].includes(url.hostname) ||
  url.pathname != "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable local DB required");
const origin = process.env.BETTER_AUTH_URL!;
const key = () => randomBytes(18).toString("hex");
test.after(async () => {
  await auth.$context;
  await database.end();
});
async function identity(operator = false) {
  const password = randomBytes(24).toString("base64url");
  const user = await createIdentity({
    email: `hosting-${key()}@example.invalid`,
    name: "Hosting test",
    password,
    operator,
    mustChangePassword: false,
  });
  const r = await auth.handler(
    new Request(origin + "/api/auth/sign-in/email", {
      method: "POST",
      headers: {
        Origin: origin,
        "Content-Type": "application/json",
        "x-webdock-client-ip": `192.0.2.${1 + (randomBytes(1)[0] % 250)}`,
      },
      body: JSON.stringify({ email: user.email, password }),
    }),
  );
  assert.equal(r.status, 200);
  if (operator)
    await database.query(
      'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
      [user.id],
    );
  const headers = new Headers({
    Origin: origin,
    Cookie: r.headers
      .getSetCookie()
      .map((x) => x.split(";")[0])
      .join("; "),
  });
  const session = await auth.api.getSession({ headers });
  assert.ok(session);
  const actor: HostingActor = {
    subject: user.id,
    sessionID: session.session.id,
    source: "studio",
    scopes: ["hosting:read", "hosting:write"],
  };
  return { actor, headers };
}
test("hosting persists real authorization, quotas, enrollment and fenced operations", async (t) => {
  await database.query(hostingRegistryLockSQL);
  await database.query(hostingSchemaSQL);
  await database.query(hostingSchemaSQL);
  const root = await identity(true),
    a = await identity(),
    b = await identity();
  const customers = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Hosting test A'),(webdock_auth.next_snowflake(),'Hosting test B') RETURNING id",
    )
  ).rows.map((r) => r.id as string);
  const orgs = (
    await database.query(
      "SELECT customer_id,organization_id FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1)",
      [customers],
    )
  ).rows;
  for (const [i, u] of [a, b].entries())
    await database.query(
      'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
      [
        orgs.find((o) => o.customer_id === customers[i]).organization_id,
        u.actor.subject,
      ],
    );
  const project = (
    await database.query(
      "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES(webdock_auth.next_snowflake(),'Hosting fixture',$1,'active') RETURNING id",
      [customers[0]],
    )
  ).rows[0].id;
  const call = (actor: HostingActor, command: unknown) =>
    executeHosting(actor, command);
  let clusterID: string = "",
    secondClusterID: string = "",
    secondProjectID: string = "";
  try {
    await subtest(
      "workspace context is authorized, names real owners and distinguishes missing hosting",
      async () => {
        const context = (await call(a.actor, {
          action: "projects.get",
          projectID: project,
        })) as {
          name: string;
          customerID: string;
          customerName: string;
          configured: boolean;
        };
        assert.equal(context.name, "Hosting fixture");
        assert.equal(context.customerID, customers[0]);
        assert.equal(context.customerName, "Hosting test A");
        assert.equal(context.configured, false);
        await assert.rejects(
          call(b.actor, { action: "projects.get", projectID: project }),
          (error) =>
            error instanceof Error && "status" in error && error.status === 404,
        );
        await assert.rejects(
          call(a.actor, { action: "operations.overview" }),
          (error) =>
            error instanceof Error && "status" in error && error.status === 403,
        );
        const overview = (await call(root.actor, {
          action: "operations.overview",
          page: 1,
          limit: 10,
          search: "",
          sort: "name",
        })) as { docs: unknown[]; operations: unknown[]; readAt: string };
        assert.ok(Array.isArray(overview.docs));
        assert.ok(Array.isArray(overview.operations));
        assert.ok(!Number.isNaN(Date.parse(overview.readAt)));
        assert.ok(overview.docs.length <= 10);
      },
    );
    await subtest(
      "authorization rejects foreign tenants, customer writes and read-only tokens",
      async () => {
        await assert.rejects(
          call(b.actor, { action: "limits.get", customerID: customers[0] }),
        );
        await assert.rejects(
          call(a.actor, {
            action: "limits.set",
            customerID: customers[0],
            values: { apps: 1 },
            revision: 0,
            subscriptionRevision: 0,
          }),
        );
        await assert.rejects(
          call(
            { ...root.actor, scopes: ["hosting:read"] },
            {
              action: "clusters.register",
              name: "Denied",
              provider: "local",
              country: "DE",
              region: "local",
              locationEvidence: "Test fixture",
              idempotencyKey: key(),
            },
          ),
        );
      },
    );
    await subtest(
      "hosting authorization works without tenant-mapping UPDATE privilege",
      async () => {
        const client = await database.connect(),
          role = "hosting_perm_" + randomBytes(8).toString("hex");
        try {
          await client.query("BEGIN");
          await client.query(`CREATE ROLE ${role} NOLOGIN`);
          await client.query(
            `GRANT USAGE ON SCHEMA webdock_auth,webdock_admin TO ${role}`,
          );
          await client.query(
            `GRANT SELECT(id,name,customer_id,status,url) ON webdock_admin.projects TO ${role}`,
          );
          await client.query(
            `GRANT SELECT,UPDATE(name) ON webdock_admin.customers TO ${role}`,
          );
          await client.query(
            `GRANT SELECT ON webdock_auth.tenant_customer,webdock_auth.hosting_project,webdock_auth.studio_tenant_preview TO ${role}`,
          );
          await client.query(
            `GRANT SELECT,UPDATE ON webdock_auth."user",webdock_auth.session,webdock_auth.member TO ${role}`,
          );
          await client.query(
            `GRANT EXECUTE ON FUNCTION webdock_admin.lock_hosting_project(varchar) TO ${role}`,
          );
          await client.query(`SET LOCAL ROLE ${role}`);
          assert.equal(
            (
              await client.query(
                "SELECT has_table_privilege(current_user,'webdock_auth.tenant_customer','UPDATE') AS can_update",
              )
            ).rows[0].can_update,
            false,
          );
          assert.equal(
            (await authorizeHosting(root.actor, { projectID: project }, client))
              .customerID,
            customers[0],
          );
        } finally {
          await client.query("ROLLBACK");
          client.release();
        }
      },
    );
    await subtest(
      "existing plan snapshots retain new hosting fields on legacy edits",
      async () => {
        const name = "Hosting plan " + key();
        await managePlans(root.headers, {
          action: "create-plan",
          name,
          "hosting.apps": "4",
          "hosting.cpuMillicores": "1000",
          "hosting.memoryBytes": "2000000000",
          "hosting.volumeBytes": "1000000000",
          "hosting.ephemeralBytes": "1000000000",
          "hosting.replicasPerApp": "3",
          "hosting.concurrentDeployments": "2",
        });
        const plan = (await getPlans(root.headers)).find(
          (p) => p.name === name,
        )!;
        await managePlans(root.headers, {
          action: "assign",
          customerID: customers[0],
          planID: plan.id,
          revision: "0",
        });
        await managePlans(root.headers, {
          action: "extras",
          customerID: customers[0],
          revision: "1",
          "hosting.apps": "1",
        });
        await managePlans(root.headers, {
          action: "extras",
          customerID: customers[0],
          revision: "2",
          storageMB: "1",
        });
        assert.equal(
          (await getTenantPlan(a.headers, customers[0])).subscription?.effective
            .hosting?.apps,
          5,
        );
      },
    );
    await subtest(
      "registration is idempotent and enrollment is one-time without inventory secrets",
      async () => {
        const input = {
          action: "clusters.register",
          name: "Local test",
          provider: "local-test",
          country: "DE",
          region: "local",
          locationEvidence: "Disposable local fixture, no provider claim",
          dedicatedCustomerID: customers[0],
          idempotencyKey: key(),
        };
        clusterID = (await call(root.actor, input)).id;
        assert.equal((await call(root.actor, input)).id, clusterID);
        await assert.rejects(call(root.actor, { ...input, name: "Changed" }));
        secondClusterID = (
          await call(root.actor, {
            ...input,
            name: "Other",
            idempotencyKey: key(),
          })
        ).id;
        const reference = await call(root.actor, {
          action: "clusters.enrollment",
          clusterID,
        });
        assert.ok(reference.enrollmentID);
        assert.equal(reference.token, undefined);
        const issued = await issueEnrollment(
          root.actor,
          reference.enrollmentID,
        );
        const race = await Promise.allSettled([
          consumeEnrollment(clusterID, issued.token),
          consumeEnrollment(clusterID, issued.token),
        ]);
        assert.equal(race.filter((x) => x.status === "fulfilled").length, 1);
        const connected = (
          race.find((x) => x.status === "fulfilled") as PromiseFulfilledResult<
            Awaited<ReturnType<typeof executeHosting>>
          >
        ).value;
        const agent = await authenticateAgent(clusterID, connected.credential);
        const observation = {
          sequence: 1,
          generation: connected.generation,
          observedAt: new Date().toISOString(),
          version: "local-fixture",
          nodes: [
            {
              id: "node",
              role: "server",
              architecture: "amd64",
              cpuMillicores: 4000,
              memoryBytes: 8000000000,
            },
          ],
          capacity: normalizeHostingAllowances({
            apps: 10,
            cpuMillicores: 3000,
            memoryBytes: 7000000000,
            volumeBytes: 2000000000,
            ephemeralBytes: 2000000000,
            replicasPerApp: 10,
            concurrentDeployments: 4,
          }),
          capabilities: {
            networkIsolation: true,
            restrictedPods: true,
            storage: true,
            ingress: true,
          },
        };
        await reportCluster(agent, observation);
        await assert.rejects(reportCluster(agent, observation));
        await assert.rejects(
          authenticateAgent(secondClusterID, connected.credential),
        );
        const inventory = await call(root.actor, {
          action: "clusters.get",
          clusterID,
        });
        assert.equal(inventory.workloadReady, false);
        assert.ok(!JSON.stringify(inventory).includes(connected.credential));
        await call(root.actor, {
          action: "projects.create",
          projectID: project,
          provider: "k3s",
          clusterID,
          mode: "selfservice",
          ownImages: true,
          idempotencyKey: key(),
        });
        const limits = await call(a.actor, {
          action: "limits.get",
          customerID: customers[0],
        });
        assert.equal(limits.effective.cpuMillicores, 1000);
        const demand = normalizeHostingAllowances({
          apps: 1,
          cpuMillicores: 750,
          memoryBytes: 1000000,
          replicasPerApp: 1,
          concurrentDeployments: 1,
        });
        const reserve = () =>
          reserveHosting(a.actor, {
            projectID: project,
            demand,
            subscriptionRevision: 3,
            idempotencyKey: key(),
          });
        await assert.rejects(reserve(), /ready|verified/i);
        // Only this disposable test establishes simulated readiness; heartbeat cannot grant it.
        await database.query(
          "UPDATE webdock_auth.hosting_cluster SET verified=true WHERE id=$1",
          [clusterID],
        );
        const results = await Promise.allSettled([reserve(), reserve()]);
        assert.equal(results.filter((x) => x.status === "fulfilled").length, 1);
        const usage = await call(a.actor, {
          action: "usage.get",
          customerID: customers[0],
        });
        assert.equal(usage.reserved.cpuMillicores, 750);
        const operation = (
          results.find(
            (x) => x.status === "fulfilled",
          ) as PromiseFulfilledResult<
            Awaited<ReturnType<typeof executeHosting>>
          >
        ).value;
        await assert.rejects(
          call(b.actor, {
            action: "operations.get",
            operationID: operation.id,
          }),
        );
        const claimed = await claimOperation(agent);
        assert.equal(claimed?.id, operation.id);
        const leaseRemaining = Number(
          (
            await database.query(
              "SELECT EXTRACT(EPOCH FROM lease_until-now()) AS seconds FROM webdock_auth.hosting_operation WHERE id=$1",
              [operation.id],
            )
          ).rows[0].seconds,
        );
        // The 260-second executor transport needs headroom to return its proof.
        assert.ok(leaseRemaining > 290 && leaseRemaining <= 300);
        await assert.rejects(
          completeOperation(agent, {
            id: claimed!.id,
            generation: claimed!.generation + 1,
            outcome: "succeeded",
          }),
        );
        await database.query(
          "UPDATE webdock_auth.hosting_operation SET lease_until=now()-interval '1 second' WHERE id=$1",
          [operation.id],
        );
        await assert.rejects(
          completeOperation(agent, {
            id: claimed!.id,
            generation: claimed!.generation,
            outcome: "succeeded",
          }),
        );
        assert.equal(await claimOperation(agent), null);
        assert.equal(
          (
            await call(a.actor, {
              action: "usage.get",
              customerID: customers[0],
            })
          ).reserved.cpuMillicores,
          750,
        );
        assert.equal(
          (
            await call(root.actor, {
              action: "operations.get",
              operationID: operation.id,
            })
          ).status,
          "needs-reconciliation",
        );
        secondProjectID = (
          await database.query(
            "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES(webdock_auth.next_snowflake(),'Second hosting fixture',$1,'active') RETURNING id",
            [customers[0]],
          )
        ).rows[0].id;
        await call(root.actor, {
          action: "projects.create",
          projectID: secondProjectID,
          provider: "k3s",
          clusterID: secondClusterID,
          mode: "selfservice",
          ownImages: false,
          idempotencyKey: key(),
        });
        await database.query(
          "UPDATE webdock_auth.hosting_cluster SET verified=true WHERE id=$1",
          [secondClusterID],
        );
        await database.query(
          "INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_seen,observation) SELECT $1,$2,1,now(),observation FROM webdock_auth.hosting_agent WHERE cluster_id=$3",
          [secondClusterID, randomBytes(32).toString("hex"), clusterID],
        );
        await assert.rejects(
          reserveHosting(a.actor, {
            projectID: secondProjectID,
            demand: { ...demand, cpuMillicores: 300 },
            subscriptionRevision: 3,
            idempotencyKey: key(),
          }),
          /Customer hosting allowance/,
        );
        await assert.rejects(
          reportCluster(agent, {
            ...observation,
            sequence: 2,
            observedAt: new Date(Date.now() - 120000).toISOString(),
          }),
          /stale/,
        );
        await assert.rejects(
          reportCluster(agent, {
            ...observation,
            sequence: 2,
            generation: agent.generation + 1,
          }),
          /generation/,
        );
        await call(root.actor, {
          action: "clusters.revoke",
          clusterID,
          revision: 2,
        });
        await assert.rejects(
          authenticateAgent(clusterID, connected.credential),
        );
      },
    );
    await subtest(
      "lower caps block growth and preview cannot write",
      async () => {
        await call(root.actor, {
          action: "limits.set",
          customerID: customers[0],
          values: { cpuMillicores: 500 },
          revision: 0,
          subscriptionRevision: 3,
        });
        const usage = await call(a.actor, {
          action: "usage.get",
          customerID: customers[0],
        });
        assert.equal(usage.overallocated, true);
        await database.query(
          "INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id) VALUES($1,$2,$3,$4)",
          [
            root.actor.sessionID,
            root.actor.subject,
            customers[0],
            orgs[0].organization_id,
          ],
        );
        await assert.rejects(
          call(root.actor, {
            action: "limits.set",
            customerID: customers[0],
            values: { apps: 5 },
            revision: 1,
            subscriptionRevision: 3,
          }),
        );
        await assert.rejects(
          call(root.actor, { action: "limits.get", customerID: customers[1] }),
        );
        await database.query(
          "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1",
          [root.actor.sessionID],
        );
        await database.query(
          'DELETE FROM webdock_auth.member WHERE "userId"=$1',
          [a.actor.subject],
        );
        await assert.rejects(
          call(a.actor, { action: "limits.get", customerID: customers[0] }),
        );
      },
    );
    await subtest(
      "managed lifecycle reserves deltas and releases only observed resources",
      async () => {
        await database.query(
          "UPDATE webdock_auth.hosting_operation SET status='failed' WHERE project_id=$1 AND status='needs-reconciliation'",
          [project],
        );
        await database.query(
          "UPDATE webdock_auth.hosting_agent SET revoked=false,last_seen=now() WHERE cluster_id=$1",
          [clusterID],
        );
        await database.query(
          "UPDATE webdock_auth.hosting_cluster SET verified=true WHERE id=$1",
          [clusterID],
        );
        await call(root.actor, {
          action: "limits.set",
          customerID: customers[0],
          values: {},
          revision: 1,
          subscriptionRevision: 3,
        });
        const spec = {
          template: "healthcheck",
          image:
            "docker.io/rancher/mirrored-library-traefik@sha256:" +
            "a".repeat(64),
          args: [],
          port: 8080,
          healthPath: "/ping",
          cpuMillicores: 100,
          memoryBytes: 67108864,
          ephemeralBytes: 67108864,
          replicas: 1,
        };
        const environment = [
          { name: "TOKEN", value: "private-env-fixture" },
          { name: "EMPTY", value: "" },
        ];
        await assert.rejects(
          call(root.actor, {
            action: "apps.create",
            projectID: project,
            name: "Missing capability",
            spec,
            environment,
            subscriptionRevision: 3,
            idempotencyKey: key(),
          }),
          /Update the cluster agent/,
        );
        await database.query(
          "UPDATE webdock_auth.hosting_agent SET observation=jsonb_set(observation,'{capabilities,environment}','true') WHERE cluster_id=$1",
          [clusterID],
        );
        const {image: _image, template: _template, ...preparedSpec} = spec;
        const prepare = {action:'apps.prepare',projectID:project,name:'Managed fixture',spec:preparedSpec,subscriptionRevision:3,idempotencyKey:key()};
        const draft = await call(root.actor, prepare);
        const unusedDraft = await call(root.actor, {...prepare,name:'Discardable draft',idempotencyKey:key(),spec:{...preparedSpec,volumeBytes:67108864}});
        assert.equal((await call(root.actor, prepare)).appID, draft.appID);
        const draftView = await call(root.actor, {action:'apps.get',appID:draft.appID});
        assert.equal(draftView.lifecycle, 'draft');
        assert.equal(draftView.spec.image, undefined);
        assert.equal((await database.query("SELECT count(*)::int AS n FROM webdock_auth.hosting_operation WHERE app_id=$1 AND status='queued'", [draft.appID])).rows[0].n, 0);
        for (const action of ['apps.start','apps.restart','apps.scale','apps.update']) await assert.rejects(call(root.actor, {
          action, appID:draft.appID,revision:draft.revision,subscriptionRevision:3,idempotencyKey:key(),
          ...(action === 'apps.scale' ? {replicas:1} : action === 'apps.update' ? {spec} : {}),
        }), /first deployment/);
        const made = await call(root.actor, {
          environment,
          action: "apps.deploy",
          appID: draft.appID,
          revision: draft.revision,
          spec,
          subscriptionRevision: 3,
          idempotencyKey: key(),
        });
        const agentRow = (
          await database.query(
            "SELECT generation,credential_hash FROM webdock_auth.hosting_agent WHERE cluster_id=$1",
            [clusterID],
          )
        ).rows[0];
        const managedAgent = {
          clusterID,
          generation: agentRow.generation,
          credentialHash: agentRow.credential_hash,
        };
        async function finish(id: string, deleted = false) {
          const job = await claimOperation(managedAgent);
          assert.equal(job.id, id);
          assert.equal(job.desired.appID, made.appID);
          const proof = {
            appID: made.appID,
            revision: job.target_revision,
            namespace: "wd-" + project,
            uid: deleted ? null : "fixture-deployment-uid",
            ready: true,
            deleted,
            replicas: deleted ? 0 : job.desired.spec.replicas,
          };
          await assert.rejects(
            completeOperation(managedAgent, {
              id,
              generation: job.generation,
              outcome: "succeeded",
              proof: { ...proof, appID: "999" },
            }),
          );
          await completeOperation(managedAgent, {
            id,
            generation: job.generation,
            outcome: "succeeded",
            proof,
          });
          return job;
        }
        const firstAttempt = await claimOperation(managedAgent);
        const deletion = await call(root.actor, {action:'apps.deletion',appID:unusedDraft.appID});
        assert.deepEqual(deletion.plan.resources, []);
        await call(root.actor, {action:'apps.delete',appID:unusedDraft.appID,confirmName:'Discardable draft',planHash:deletion.planHash,idempotencyKey:key()});
        assert.equal((await call(root.actor, {action:'apps.get',appID:unusedDraft.appID})).status, 'deleted');
        assert.equal((await database.query("SELECT count(*)::int AS n FROM webdock_auth.hosting_operation WHERE app_id=$1", [unusedDraft.appID])).rows[0].n, 0);

        assert.equal(firstAttempt.id, made.id);
        assert.equal(
          firstAttempt.desired.leaseUntil,
          firstAttempt.lease_until.toISOString(),
        );
        assert.deepEqual(firstAttempt.desired.environment, {
          TOKEN: "private-env-fixture",
          EMPTY: "",
        });
        assert.equal(firstAttempt.desired.environmentEncrypted, undefined);
        const persisted = (
          await database.query(
            "SELECT to_jsonb(a) AS app FROM webdock_auth.hosting_app a WHERE id=$1",
            [made.appID],
          )
        ).rows;
        assert.ok(!JSON.stringify(persisted).includes("private-env-fixture"));
        const persistedOps = (
          await database.query(
            "SELECT desired FROM webdock_auth.hosting_operation WHERE app_id=$1",
            [made.appID],
          )
        ).rows;
        assert.ok(
          !JSON.stringify(persistedOps).includes("private-env-fixture"),
        );
        await completeOperation(managedAgent, {
          id: made.id,
          generation: firstAttempt.generation,
          outcome: "failed",
          error: "rollout_timeout",
        });
        const diagnostic = await call(root.actor, {
          action: "apps.logs",
          appID: made.appID,
          idempotencyKey: key(),
        });
        const diagnosticClaim = await claimOperation(managedAgent);
        assert.equal(diagnosticClaim.id, diagnostic.id);
        await completeOperation(managedAgent, {
          id: diagnostic.id,
          generation: diagnosticClaim.generation,
          outcome: "succeeded",
          proof: {
            appID: made.appID,
            revision: 1,
            namespace: "wd-" + project,
            uid: "fixture-deployment-uid",
            ready: true,
            deleted: false,
            replicas: 1,
            logsBase64: Buffer.from(
              "diagnostic output private-env-fixture",
            ).toString("base64"),
            logsTruncated: false,
          },
        });
        assert.equal(
          (await call(root.actor, { action: "apps.get", appID: made.appID }))
            .status,
          "failed",
        );
        assert.match(
          (await call(root.actor, { action: "apps.get", appID: made.appID }))
            .lastError,
          /did not become ready/,
        );
        await call(root.actor, {
          action: "apps.reconcile",
          appID: made.appID,
          idempotencyKey: key(),
        });
        await finish(made.id);
        const originalKey = (
          await database.query(
            "SELECT idempotency_key FROM webdock_auth.hosting_operation WHERE id=$1",
            [made.id],
          )
        ).rows[0].idempotency_key;
        const repeated = await call(root.actor, {
          environment,
          action: "apps.deploy",
          appID: draft.appID,
          revision: draft.revision,
          spec,
          subscriptionRevision: 3,
          idempotencyKey: originalKey,
        });
        assert.equal(repeated.id, made.id);
        assert.equal(repeated.appID, made.appID);
        let app = await call(root.actor, {
          action: "apps.get",
          appID: made.appID,
        });
        assert.equal(app.status, "ready");
        assert.deepEqual(app.environmentNames, ["EMPTY", "TOKEN"]);
        const overview = (await call(root.actor, {
          action: "operations.overview",
          page: 1,
          limit: 10,
          search: "Managed fixture",
          sort: "name",
        })) as import("@webdock/hosting-contracts").HostingOverview;
        assert.equal(
          overview.docs.find((item) => item.id === made.appID)?.customerName,
          "Hosting test A",
        );
        assert.ok(!JSON.stringify(overview).includes("private-env-fixture"));
        assert.ok(!("spec" in overview.docs[0]));
        assert.ok(!("logs" in overview.docs[0]));

        assert.ok(!JSON.stringify(app).includes("private-env-fixture"));
        const logResult = (
          await database.query(
            "SELECT result FROM webdock_auth.hosting_operation WHERE id=$1",
            [diagnostic.id],
          )
        ).rows[0].result;
        assert.ok(!JSON.stringify(logResult).includes("private-env-fixture"));
        await assert.rejects(
          call(b.actor, {
            action: "apps.update",
            appID: app.id,
            spec,
            environment: [{ name: "TOKEN", value: "foreign" }],
            revision: app.revision,
            subscriptionRevision: 3,
            idempotencyKey: key(),
          }),
        );
        const envUpdate = await call(root.actor, {
          action: "apps.update",
          appID: app.id,
          spec,
          environment: [
            { name: "TOKEN", value: "rotated-fixture" },
            { name: "EMPTY", value: null },
          ],
          revision: app.revision,
          subscriptionRevision: 3,
          idempotencyKey: key(),
        });
        assert.deepEqual((await finish(envUpdate.id)).desired.environment, {
          TOKEN: "rotated-fixture",
        });
        app = await call(root.actor, { action: "apps.get", appID: app.id });
        const envRollback = await call(root.actor, {
          action: "apps.rollback",
          appID: app.id,
          revision: app.revision,
          subscriptionRevision: 3,
          idempotencyKey: key(),
        });
        assert.deepEqual((await finish(envRollback.id)).desired.environment, {
          TOKEN: "private-env-fixture",
          EMPTY: "",
        });
        app = await call(root.actor, { action: "apps.get", appID: app.id });
        const testOrg = orgs.find(
          (o) => o.customer_id === customers[0],
        ).organization_id;
        await database.query(
          "INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id) VALUES($1,$2,$3,$4)",
          [root.actor.sessionID, root.actor.subject, customers[0], testOrg],
        );
        await assert.rejects(
          call(root.actor, { action: "operations.overview" }),
          /read-only/,
        );
        assert.equal(
          (
            await call(root.actor, {
              action: "projects.get",
              projectID: project,
            })
          ).operator,
          false,
        );
        assert.equal(
          (await call(root.actor, { action: "apps.get", appID: made.appID }))
            .canRequestLogs,
          false,
        );
        await assert.rejects(
          call(root.actor, {
            action: "apps.logs",
            appID: made.appID,
            idempotencyKey: key(),
          }),
          /read-only/,
        );
        await database.query(
          "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1",
          [root.actor.sessionID],
        );

        await assert.rejects(
          call(root.actor, {
            action: "apps.scale",
            appID: app.id,
            replicas: 3,
            revision: app.revision,
            subscriptionRevision: 3,
            idempotencyKey: key(),
          }),
          /allowance/,
        );
        let operation = await call(root.actor, {
          action: "apps.scale",
          appID: app.id,
          replicas: 2,
          revision: app.revision,
          subscriptionRevision: 3,
          idempotencyKey: key(),
        });
        await finish(operation.id);
        assert.equal(
          (
            await call(root.actor, {
              action: "usage.get",
              customerID: customers[0],
            })
          ).allocated.cpuMillicores,
          200,
        );
        app = await call(root.actor, { action: "apps.get", appID: made.appID });
        const managedScope = {pluginID:"managed-apps",customerID:customers[0],projectID:project,operationID:key(),subject:root.actor.subject,sessionID:root.actor.sessionID};
        const managedBinding = {appID:app.id,kind:"tabularium" as const,origin:"https://registry.example"};
        const managed = createPluginApps({run:(_request,_capability,fn)=>fn(managedScope,[managedBinding]),handler:()=>createManagedAppsHandler(executeHosting,requireHostingCommand)});
        const request = new Request("https://host.example/v1/plugin/apps");
        const safe = await managed.request(request,{operation:"get",input:{app_id:app.id}});
        assert.equal(JSON.stringify(safe).includes("private-env-fixture"),false);
        assert.equal("logs" in safe,false);
        const stop = {operation:"command",input:{app_id:app.id,operation:"stop",revision:app.revision,subscription_revision:3}};
        const stopped = await managed.request(request,stop);
        assert.ok("operation_id" in stopped);
        assert.deepEqual(await managed.request(request,stop),stopped,"same invocation retries the native operation");
        managedScope.operationID=key();
        await assert.rejects(managed.request(request,stop),error=>!!error && typeof error==="object" && "code" in error && error.code==="conflict");
        managedScope.projectID=secondProjectID;
        await assert.rejects(managed.request(request,stop),error=>!!error && typeof error==="object" && "code" in error && error.code==="permission_denied");
        managedScope.projectID=project;
        managedScope.subject=b.actor.subject;managedScope.sessionID=b.actor.sessionID;
        await assert.rejects(managed.request(request,{operation:"get",input:{app_id:app.id}}));
        await finish(stopped.operation_id);
        assert.equal(
          (await call(root.actor, { action: "apps.get", appID: made.appID }))
            .status,
          "stopped",
        );
        const logOperation = await call(root.actor, {
          action: "apps.logs",
          appID: made.appID,
          idempotencyKey: key(),
        });
        const logClaim = await claimOperation(managedAgent);
        assert.equal(logClaim.id, logOperation.id);
        await completeOperation(managedAgent, {
          id: logClaim.id,
          generation: logClaim.generation,
          outcome: "failed",
          error: "execution_failed",
        });
        assert.equal(
          (
            await call(root.actor, {
              action: "operations.get",
              operationID: logOperation.id,
            })
          ).status,
          "failed",
          "log failure must not block application mutations",
        );
        assert.equal(
          (await call(root.actor, { action: "apps.get", appID: made.appID }))
            .status,
          "stopped",
        );
        const preview = await call(root.actor, {
          action: "apps.deletion",
          appID: made.appID,
        });
        await assert.rejects(
          call(root.actor, {
            action: "apps.delete",
            appID: made.appID,
            confirmName: "wrong",
            planHash: preview.planHash,
            idempotencyKey: key(),
          }),
        );
        operation = await call(root.actor, {
          action: "apps.delete",
          appID: made.appID,
          confirmName: preview.plan.name,
          planHash: preview.planHash,
          idempotencyKey: key(),
        });
        await finish(operation.id, true);
        assert.equal(
          (await call(root.actor, { action: "apps.get", appID: made.appID }))
            .status,
          "deleted",
        );
        assert.equal(
          (
            await call(root.actor, {
              action: "usage.get",
              customerID: customers[0],
            })
          ).allocated.apps,
          0,
        );
        await database.query(
          'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
          [testOrg, a.actor.subject],
        );
        await call(root.actor, {
          action: "projects.update",
          projectID: project,
          mode: "selfservice",
          ownImages: false,
          revision: 1,
        });
        await assert.rejects(
          call(a.actor, {
            action: "apps.create",
            projectID: project,
            name: "Forbidden custom",
            spec: { ...spec, template: "custom" },
            subscriptionRevision: 3,
            idempotencyKey: key(),
          }),
          /Custom images/,
        );
        {
          // Persistent bytes survive stop and deletion; operator purge releases them.
          const storedSpec = { ...spec, volumeBytes: 134217728 };
          const create = {
            action: "apps.create",
            projectID: project,
            name: "Persistent fixture",
            spec: storedSpec,
            subscriptionRevision: 3,
            idempotencyKey: key(),
          };
          await assert.rejects(call(root.actor, create), /storage/i);
          await database.query(
            "UPDATE webdock_auth.hosting_agent SET observation=jsonb_set(jsonb_set(jsonb_set(observation,'{capabilities,storage}','true'),'{capabilities,storageVersion}','1'),'{capacity,volumeBytes}','1000000000'),last_seen=now() WHERE cluster_id=$1",
            [clusterID],
          );
          await database.query(
            "UPDATE webdock_auth.hosting_cluster SET capacity=jsonb_set(capacity,'{volumeBytes}','1000000000') WHERE id=$1",
            [clusterID],
          );
          const stored = await call(a.actor, create);
          async function complete(
            op: { id: string },
            deleted = false,
            purged = false,
          ) {
            const job = await claimOperation(managedAgent);
            assert.equal(job.id, op.id);
            assert.equal(job.desired.storageVersion, 1);
            const proof = {
              appID: stored.appID,
              revision: job.target_revision,
              namespace: "wd-" + project,
              uid: deleted ? null : "storage-uid",
              ready: true,
              deleted,
              replicas: deleted ? 0 : job.desired.spec.replicas,
              storage: {
                volumeBytes: purged ? 0 : 134217728,
                retained: deleted && !purged,
              },
            };
            const withoutStorage = { ...proof };
            Reflect.deleteProperty(withoutStorage, "storage");
            await assert.rejects(
              completeOperation(managedAgent, {
                id: job.id,
                generation: job.generation,
                outcome: "succeeded",
                proof: withoutStorage,
              }),
            );
            await completeOperation(managedAgent, {
              id: job.id,
              generation: job.generation,
              outcome: "succeeded",
              proof,
            });
            return job;
          }
          await complete(stored);
          const log = await call(a.actor, {
            action: "apps.logs",
            appID: stored.appID,
            idempotencyKey: key(),
          });
          const logJob = await claimOperation(managedAgent);
          assert.equal(logJob.id, log.id);
          assert.equal(logJob.desired.storageVersion, 1);
          await completeOperation(managedAgent, {
            id: logJob.id,
            generation: logJob.generation,
            outcome: "succeeded",
            proof: {
              appID: stored.appID,
              revision: logJob.target_revision,
              namespace: "wd-" + project,
              uid: "storage-uid",
              ready: true,
              deleted: false,
              replicas: 1,
              logs: "storage app log",
            },
          });
          const get = () =>
            call(a.actor, { action: "apps.get", appID: stored.appID });
          const app = await get();
          await assert.rejects(
            call(a.actor, {
              action: "apps.scale",
              appID: stored.appID,
              replicas: 2,
              revision: app.revision,
              subscriptionRevision: 3,
              idempotencyKey: key(),
            }),
          );
          await assert.rejects(
            call(a.actor, {
              action: "apps.update",
              appID: stored.appID,
              spec: { ...storedSpec, volumeBytes: 0 },
              revision: app.revision,
              subscriptionRevision: 3,
              idempotencyKey: key(),
            }),
            /fixed/i,
          );
          await assert.rejects(
            call(root.actor, {
              action: "apps.storageDeletion",
              appID: stored.appID,
            }),
            /delete/i,
          );
          await complete(
            await call(a.actor, {
              action: "apps.stop",
              appID: stored.appID,
              revision: app.revision,
              subscriptionRevision: 3,
              idempotencyKey: key(),
            }),
          );
          assert.equal(
            (
              await call(a.actor, {
                action: "usage.get",
                customerID: customers[0],
              })
            ).allocated.volumeBytes,
            134217728,
          );
          const preview = await call(root.actor, {
            action: "apps.deletion",
            appID: stored.appID,
          });
          assert.equal(preview.plan.persistentData, true);
          const removed = await call(root.actor, {
            action: "apps.delete",
            appID: stored.appID,
            confirmName: preview.plan.name,
            planHash: preview.planHash,
            idempotencyKey: key(),
          });
          const job = await complete(removed, true);
          assert.equal(job.desired.finalQuota.volumeBytes, 134217728);
          assert.equal((await get()).status, "deleted");
          assert.ok(
            (
              await call(a.actor, { action: "apps.list", projectID: project })
            ).docs.some((v: { id: string }) => v.id === stored.appID),
          );
          assert.equal(
            (
              await call(a.actor, {
                action: "usage.get",
                customerID: customers[0],
              })
            ).allocated.volumeBytes,
            134217728,
          );
          await assert.rejects(
            call(a.actor, {
              action: "apps.storageDeletion",
              appID: stored.appID,
            }),
            /operator/,
          );
          const purge = await call(root.actor, {
            action: "apps.storageDeletion",
            appID: stored.appID,
          });
          await assert.rejects(
            call(a.actor, {
              action: "apps.purgeStorage",
              appID: stored.appID,
              confirmName: purge.plan.name,
              planHash: purge.planHash,
              idempotencyKey: key(),
            }),
            /operator/,
          );
          await assert.rejects(
            call(root.actor, {
              action: "apps.purgeStorage",
              appID: stored.appID,
              confirmName: purge.plan.name,
              planHash: "0".repeat(64),
              idempotencyKey: key(),
            }),
          );
          await database.query(
            "UPDATE webdock_auth.hosting_agent SET observation=jsonb_set(observation,'{capabilities,storage}','false') WHERE cluster_id=$1",
            [clusterID],
          );
          await complete(
            await call(root.actor, {
              action: "apps.purgeStorage",
              appID: stored.appID,
              confirmName: purge.plan.name,
              planHash: purge.planHash,
              idempotencyKey: key(),
            }),
            true,
            true,
          );
          assert.equal(
            (
              await call(a.actor, {
                action: "usage.get",
                customerID: customers[0],
              })
            ).allocated.volumeBytes,
            0,
          );
          assert.equal((await get()).spec.volumeBytes, 0);
          assert.equal(
            (
              await call(root.actor, {
                action: "apps.storageDeletion",
                appID: stored.appID,
              })
            ).removed,
            true,
          );
        }
        const own = await call(a.actor, {
          action: "apps.create",
          projectID: project,
          name: "Customer selfservice fixture",
          spec,
          subscriptionRevision: 3,
          idempotencyKey: key(),
        });
        assert.equal(own.status, "queued");
        await assert.rejects(
          call(b.actor, { action: "apps.get", appID: own.appID }),
        );
        await assert.rejects(
          call(a.actor, { action: "apps.deletion", appID: own.appID }),
          /operator/,
        );
      },
    );
  } finally {
    for (const table of [
      "hosting_reservation",
      "hosting_operation",
      "hosting_limit",
    ])
      await database.query(
        `DELETE FROM webdock_auth.${table} WHERE customer_id=ANY($1)`,
        [customers],
      );
    await database.query(
      "DELETE FROM webdock_auth.hosting_app WHERE project_id IN (SELECT project_id FROM webdock_auth.hosting_project WHERE customer_id=ANY($1))",
      [customers],
    );
    await database.query(
      "DELETE FROM webdock_auth.hosting_project WHERE customer_id=ANY($1)",
      [customers],
    );
    for (const table of ["hosting_enrollment", "hosting_agent"])
      await database.query(
        `DELETE FROM webdock_auth.${table} WHERE cluster_id=ANY($1)`,
        [[clusterID, secondClusterID].filter(Boolean)],
      );
    await database.query(
      "DELETE FROM webdock_auth.hosting_operation WHERE subject=ANY($1)",
      [[root.actor.subject, a.actor.subject, b.actor.subject]],
    );
    await database.query(
      "DELETE FROM webdock_auth.hosting_cluster WHERE id=ANY($1)",
      [[clusterID, secondClusterID].filter(Boolean)],
    );
    await database.query(
      "DELETE FROM webdock_auth.hosting_audit WHERE subject=ANY($1)",
      [[root.actor.subject, a.actor.subject, b.actor.subject]],
    );
    await database.query(
      "DELETE FROM webdock_admin.projects WHERE id=ANY($1)",
      [[project, secondProjectID].filter(Boolean)],
    );
    for (const table of [
      "plan_offer",
      "subscription_history",
      "tenant_subscription",
    ])
      await database.query(
        `DELETE FROM webdock_auth.${table} WHERE customer_id=ANY($1)`,
        [customers],
      );
    await database.query(
      "DELETE FROM webdock_auth.plan_template WHERE created_by=$1",
      [root.actor.subject],
    );
    await database.query(
      'DELETE FROM webdock_auth.member WHERE "userId"=ANY($1)',
      [[root.actor.subject, a.actor.subject, b.actor.subject]],
    );
    await database.query(
      "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1)",
      [customers],
    );
    await database.query(
      "DELETE FROM webdock_admin.customers WHERE id=ANY($1)",
      [customers],
    );
  }
});

test("BYOK tenant isolation, disabled policy, enrollment and finite cluster count", async () => {
  await database.query(hostingSchemaSQL);
  const root = await identity(true),
    owner = await identity(),
    stranger = await identity();
  const customer = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'BYOK fixture') RETURNING id",
    )
  ).rows[0].id;
  const org = (
    await database.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customer],
    )
  ).rows[0].organization_id;
  await database.query(
    'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
    [org, owner.actor.subject],
  );
  const call = (actor: HostingActor, cmd: unknown) =>
    executeHosting(actor, cmd);
  const registration = {
    action: "byok.register",
    customerID: customer,
    name: "Customer cluster",
    provider: "k3s",
    region: "EU",
    locationEvidence: "Owner confirmed EU",
    idempotencyKey: key(),
  };
  await assert.rejects(call(owner.actor, registration));
  const policy = await call(root.actor, {
    action: "byok.policy.set",
    customerID: customer,
    revision: 0,
    kubernetes: true,
    vercel: true,
    maxClusters: 1,
    manageExisting: true,
    namespaces: ["apps"],
  });
  assert.equal(policy.kubernetes, true);
  await assert.rejects(
    call(owner.actor, {
      action: "byok.policy.set",
      customerID: customer,
      revision: policy.revision,
      kubernetes: true,
      vercel: true,
      maxClusters: 100,
      manageExisting: true,
      namespaces: ["apps"],
    }),
  );
  const c = await call(owner.actor, registration);
  assert.equal(c.dedicatedCustomerID, customer);
  await assert.rejects(
    call(stranger.actor, { action: "byok.cluster", clusterID: c.id }),
  );
  assert.equal(
    (await call(owner.actor, { action: "byok.list", customerID: customer }))
      .clusters.length,
    1,
  );
  await assert.rejects(
    call(owner.actor, { ...registration, idempotencyKey: key() }),
  );
  const enrollment = await call(owner.actor, {
    action: "byok.enrollment",
    clusterID: c.id,
  });
  const token = await issueEnrollment(owner.actor, enrollment.enrollmentID);
  assert.equal(token.clusterID, c.id);
  await call(root.actor, {
    action: "byok.policy.set",
    customerID: customer,
    revision: policy.revision,
    kubernetes: false,
    vercel: false,
    maxClusters: 1,
    manageExisting: false,
    namespaces: ["apps"],
  });
  await assert.rejects(consumeEnrollment(c.id, token.token));
});

test("BYOK Vercel callback binds the tenant session and selected projects gate provider calls", async () => {
  await database.query(hostingSchemaSQL);
  const root = await identity(true),
    owner = await identity(),
    other = await identity();
  const customer = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Vercel BYOK fixture') RETURNING id",
    )
  ).rows[0].id;
  const org = (
    await database.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customer],
    )
  ).rows[0].organization_id;
  await database.query(
    'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
    [org, owner.actor.subject],
  );
  const runtime = {
    clientID: "integration_fixture",
    clientSecret: "fixture-secret",
    slug: "webdock-fixture",
    platformTeam: "team_platformfixture",
    origin: "https://studio.example.invalid",
  };
  const call = (actor: HostingActor, cmd: unknown) =>
    executeHosting(actor, cmd, runtime);
  await call(root.actor, {
    action: "byok.policy.set",
    customerID: customer,
    revision: 0,
    kubernetes: false,
    vercel: true,
    maxClusters: 0,
    manageExisting: false,
    namespaces: [],
  });
  const before = { ...process.env };
  Object.assign(process.env, {
    WEBDOCK_STUDIO_ORIGIN: "https://studio.example.invalid",
  });
  const teamID = "team_" + key(),
    configurationID = "icfg_" + key();
  let requests = 0;
  const original = globalThis.fetch;
  try {
    const start = await call(owner.actor, {
      action: "byok.vercel.begin",
      customerID: customer,
    });
    const state = new URL(start.url).searchParams.get("state");
    globalThis.fetch = async (input) => {
      requests++;
      const u = String(input);
      return Response.json(
        u.includes("access_token")
          ? {
              access_token: "fixture-private-token",
              team_id: teamID,
              installation_id: configurationID,
              token_type: "Bearer",
            }
          : u.includes("/configuration/")
            ? {
                id: configurationID,
                teamId: teamID,
                integrationId: "integration_fixture",
                scopes: [
                  "read:integration-configuration",
                  "read:project",
                  "read:deployment",
                  "read:domain",
                ],
              }
            : {
                projects: [
                  {
                    id: "prj_fixture",
                    name: "Existing website",
                    accountId: teamID,
                  },
                ],
                pagination: { next: null },
              },
      );
    };
    const finish = {
      action: "byok.vercel.finish",
      state,
      code: "fixture-code",
      teamID,
      configurationID,
    };
    await assert.rejects(call(other.actor, finish));
    assert.equal(requests, 0);
    assert.equal((await call(owner.actor, finish)).customerID, customer);
    await assert.rejects(call(owner.actor, finish));
    const connection = (
      await database.query(
        "SELECT encrypted_token,projects FROM webdock_auth.hosting_vercel WHERE customer_id=$1",
        [customer],
      )
    ).rows[0];
    assert.ok(!connection.encrypted_token.includes("fixture-private-token"));
    assert.deepEqual(connection.projects, []);
    const previousRequests = requests;
    await assert.rejects(
      call(owner.actor, {
        action: "byok.vercel.resources",
        customerID: customer,
        projectID: "prj_foreign",
      }),
    );
    assert.equal(requests, previousRequests);
    await call(owner.actor, {
      action: "byok.vercel.select",
      customerID: customer,
      revision: 1,
      projects: ["prj_fixture"],
    });
    await assert.rejects(
      call(other.actor, {
        action: "byok.vercel.projects",
        customerID: customer,
      }),
    );
  } finally {
    globalThis.fetch = original;
    for (const k of Object.keys(process.env))
      if (!(k in before)) delete process.env[k];
    Object.assign(process.env, before);
  }
});
test("BYOK inventory is bounded, namespace filtered, identity pinned and permission changes fence queued jobs", async () => {
  await database.query(hostingSchemaSQL);
  const root = await identity(true),
    owner = await identity();
  const customer = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Inventory fixture') RETURNING id",
    )
  ).rows[0].id;
  const org = (
    await database.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customer],
    )
  ).rows[0].organization_id;
  await database.query(
    'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
    [org, owner.actor.subject],
  );
  const call = (actor: HostingActor, cmd: unknown) =>
    executeHosting(actor, cmd);
  const policy = {
    action: "byok.policy.set",
    customerID: customer,
    revision: 0,
    kubernetes: true,
    vercel: false,
    maxClusters: 1,
    manageExisting: true,
    namespaces: ["apps"],
  };
  await call(root.actor, policy);
  const cluster = await call(owner.actor, {
    action: "byok.register",
    customerID: customer,
    name: "Inventory fixture",
    provider: "kubernetes",
    region: "EU",
    locationEvidence: "Local disposable test",
    idempotencyKey: key(),
  });
  const enrollment = await call(owner.actor, {
    action: "byok.enrollment",
    clusterID: cluster.id,
  });
  const token = await issueEnrollment(owner.actor, enrollment.enrollmentID);
  const connection = await consumeEnrollment(cluster.id, token.token);
  const agent = await authenticateAgent(cluster.id, connection.credential);
  await reportCluster(agent, {
    sequence: 1,
    generation: connection.generation,
    observedAt: new Date().toISOString(),
    version: "v1.35.1",
    nodes: [
      {
        id: "test",
        role: "server",
        architecture: "amd64",
        cpuMillicores: 2000,
        memoryBytes: 2e9,
      },
    ],
    capacity: normalizeHostingAllowances(undefined),
    capabilities: {
      networkIsolation: false,
      restrictedPods: false,
      storage: false,
      ingress: false,
    },
  });
  const resource = {
    uid: "existing-deployment",
    resourceVersion: "100",
    kind: "Deployment",
    name: "existing",
    namespace: "apps",
    status: "ready",
    replicas: 1,
    ready: 1,
  };
  const inventory = {
    clusterUID: "cluster-identity",
    observedAt: new Date().toISOString(),
    complete: true,
    unavailable: ["metrics"],
    resources: [resource, { ...resource, uid: "other", namespace: "foreign" }],
  };
  await saveInventory(agent, inventory);
  assert.equal(
    (
      await call(owner.actor, {
        action: "byok.inventory",
        clusterID: cluster.id,
      })
    ).resources.length,
    1,
  );
  await assert.rejects(
    saveInventory(agent, { ...inventory, clusterUID: "changed" }),
  );
  await assert.rejects(
    saveInventory(agent, {
      ...inventory,
      resources: [{ ...resource, secret: "never store" }],
    }),
  );
  const op = await call(owner.actor, {
    action: "byok.workload",
    clusterID: cluster.id,
    uid: resource.uid,
    resourceVersion: resource.resourceVersion,
    operation: "scale",
    replicas: 2,
    idempotencyKey: key(),
  });
  await call(root.actor, { ...policy, revision: 1, manageExisting: false });
  assert.equal(await claimByokOperation(agent), null);
  assert.equal(
    (
      await call(owner.actor, {
        action: "operations.get",
        operationID: op.operationID,
      })
    ).status,
    "failed",
  );
  await call(root.actor, { ...policy, revision: 2 });
  const op2 = await call(owner.actor, {
    action: "byok.workload",
    clusterID: cluster.id,
    uid: resource.uid,
    resourceVersion: resource.resourceVersion,
    operation: "scale",
    replicas: 2,
    idempotencyKey: key(),
  });
  const job = await claimByokOperation(agent);
  assert.equal(job.id, op2.operationID);
  await assert.rejects(
    completeByokOperation(agent, {
      id: job.id,
      generation: job.generation,
      outcome: "succeeded",
      proof: { uid: "other", action: "scale", observed: true },
    }),
  );
  assert.equal(
    (
      await completeByokOperation(agent, {
        id: job.id,
        generation: job.generation,
        outcome: "succeeded",
        proof: { uid: resource.uid, action: "scale", observed: true },
      })
    ).status,
    "succeeded",
  );
  const expired = await call(owner.actor, {
    action: "byok.workload",
    clusterID: cluster.id,
    uid: resource.uid,
    resourceVersion: resource.resourceVersion,
    operation: "restart",
    idempotencyKey: key(),
  });
  await database.query(
    "UPDATE webdock_auth.hosting_operation SET created_at=now()-interval '4 minutes' WHERE id=$1",
    [expired.operationID],
  );
  assert.equal(await claimByokOperation(agent), null);
  assert.equal(
    (
      await call(owner.actor, {
        action: "operations.get",
        operationID: expired.operationID,
      })
    ).status,
    "failed",
  );
  const failing = await call(owner.actor, {
    action: "byok.workload",
    clusterID: cluster.id,
    uid: resource.uid,
    resourceVersion: resource.resourceVersion,
    operation: "restart",
    idempotencyKey: key(),
  });
  const failedJob = await claimByokOperation(agent);
  await completeByokOperation(agent, {
    id: failedJob.id,
    generation: failedJob.generation,
    outcome: "failed",
  });
  await assert.rejects(
    call(owner.actor, {
      action: "byok.reconcile",
      operationID: failing.operationID,
      resourceVersion: resource.resourceVersion,
    }),
  );
  assert.equal(
    (
      await call(root.actor, {
        action: "byok.reconcile",
        operationID: failing.operationID,
        resourceVersion: resource.resourceVersion,
      })
    ).reviewed,
    true,
  );
});

test("turboSMTP BYOK keeps credentials private and enforces tenant, revision and domain limits", async () => {
  await database.query(hostingSchemaSQL);
  const root = await identity(true),
    owner = await identity(),
    other = await identity();
  const customers = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Own SMTP A'),(webdock_auth.next_snowflake(),'Own SMTP B') RETURNING id",
    )
  ).rows.map((r) => r.id);
  for (const [i, user] of [owner, other].entries()) {
    const org = (
      await database.query(
        "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
        [customers[i]],
      )
    ).rows[0].organization_id;
    await database.query(
      'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
      [org, user.actor.subject],
    );
  }
  const call = (actor: HostingActor, cmd: unknown) =>
    executeHosting(actor, cmd);
  const credential = "test-consumer-" + key();
  const secret = "private-secret-" + key();
  const connect = {
    action: "byok.mail.connect",
    customerID: customers[0],
    label: "Own account",
    consumerKey: credential,
    consumerSecret: secret,
    confirmAccountAccess: true,
    revision: 0,
  };
  await assert.rejects(call(owner.actor, connect));
  for (const customerID of customers)
    await call(root.actor, {
      action: "byok.policy.set",
      customerID,
      revision: 0,
      kubernetes: false,
      vercel: false,
      turbosmtp: true,
      maxMailDomains: 1,
      maxClusters: 0,
      manageExisting: false,
      namespaces: [],
    });
  let requests = 0;
  const original = globalThis.fetch;
  let domains: {
    id: string;
    domain: string;
    spf_verified: boolean;
    dkim_verified: boolean;
    dmarc_verified: boolean;
  }[] = [];
  globalThis.fetch = async (input, init) => {
    requests++;
    assert.match(
      String(input),
      /^https:\/\/pro\.api\.serversmtp\.com\/api\/v2\/sender-domains/,
    );
    assert.equal(new Headers(init?.headers).get("consumerKey"), credential);
    if (init?.method === "POST") {
      const body = JSON.parse(String(init.body));
      domains = [
        {
          id: "domain1",
          domain: body.domain,
          spf_verified: false,
          dkim_verified: false,
          dmarc_verified: false,
        },
      ];
      return Response.json(domains[0]);
    }
    return Response.json({ count: domains.length, results: domains });
  };
  try {
    await assert.rejects(call({ ...owner.actor, source: "oauth" }, connect));
    assert.equal(requests, 0);
    const saved = await call(owner.actor, connect);
    assert.equal(saved.connected, true);
    assert.ok(!JSON.stringify(saved).includes(secret));
    assert.ok(!JSON.stringify(saved).includes(credential));
    await assert.rejects(
      call(other.actor, { action: "byok.mail.get", customerID: customers[0] }),
    );
    await assert.rejects(
      call(other.actor, { ...connect, customerID: customers[1] }),
    );
    const stored = (
      await database.query(
        "SELECT encrypted_secret FROM webdock_auth.hosting_turbosmtp WHERE customer_id=$1",
        [customers[0]],
      )
    ).rows[0];
    assert.ok(!stored.encrypted_secret.includes(secret));
    const registered = await call(owner.actor, {
      action: "byok.mail.domain",
      customerID: customers[0],
      revision: saved.revision,
      domain: "example.invalid",
    });
    assert.equal(registered.domains.length, 1);
    await assert.rejects(
      call(owner.actor, {
        action: "byok.mail.domain",
        customerID: customers[0],
        revision: registered.revision,
        domain: "second.invalid",
      }),
    );
    await assert.rejects(
      call(owner.actor, {
        action: "byok.mail.disconnect",
        customerID: customers[0],
        revision: saved.revision,
      }),
    );
    await call(owner.actor, {
      action: "byok.mail.disconnect",
      customerID: customers[0],
      revision: registered.revision,
    });
    assert.equal(
      (
        await call(owner.actor, {
          action: "byok.mail.get",
          customerID: customers[0],
        })
      ).connected,
      false,
    );
  } finally {
    globalThis.fetch = original;
  }
});

test("turboSMTP uncertain registration is fenced and resolved by observation, not blind retry", async () => {
  await database.query(hostingSchemaSQL);
  const root = await identity(true),
    owner = await identity();
  const customerID = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Own SMTP recovery') RETURNING id",
    )
  ).rows[0].id;
  const org = (
    await database.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customerID],
    )
  ).rows[0].organization_id;
  await database.query(
    'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
    [org, owner.actor.subject],
  );
  const call = (actor: HostingActor, cmd: unknown) =>
    executeHosting(actor, cmd);
  const policy = {
    action: "byok.policy.set",
    customerID,
    revision: 0,
    kubernetes: false,
    vercel: false,
    turbosmtp: true,
    maxMailDomains: 2,
    maxClusters: 0,
    manageExisting: false,
    namespaces: [],
  };
  await call(root.actor, policy);
  const legacy = { ...policy };
  Reflect.deleteProperty(legacy, "turbosmtp");
  Reflect.deleteProperty(legacy, "maxMailDomains");
  const kept = await call(root.actor, { ...legacy, revision: 1 });
  assert.equal(kept.turbosmtp, true);
  assert.equal(kept.maxMailDomains, 2);
  let domains: {
    id: string;
    domain: string;
    spf_verified: boolean;
    dkim_verified: boolean;
    dmarc_verified: boolean;
  }[] = [];
  let writes = 0;
  const original = globalThis.fetch;
  globalThis.fetch = async (_url, init) => {
    if (init?.method === "POST") {
      writes++;
      domains = [
        {
          id: "recovered",
          domain: "example.invalid",
          spf_verified: false,
          dkim_verified: false,
          dmarc_verified: false,
        },
      ];
      throw Error("response lost after provider write");
    }
    return Response.json({ count: domains.length, results: domains });
  };
  try {
    const connected = await call(owner.actor, {
      action: "byok.mail.connect",
      customerID,
      label: "Own account",
      consumerKey: "consumer-" + key(),
      consumerSecret: "secret-" + key(),
      confirmAccountAccess: true,
      revision: 0,
    });
    await assert.rejects(
      call(owner.actor, {
        action: "byok.mail.domain",
        customerID,
        revision: connected.revision,
        domain: "example.invalid",
      }),
    );
    const uncertain = await call(owner.actor, {
      action: "byok.mail.get",
      customerID,
    });
    assert.equal(uncertain.state, "needs_review");
    await assert.rejects(
      call(owner.actor, {
        action: "byok.mail.domain",
        customerID,
        revision: uncertain.revision,
        domain: "example.invalid",
      }),
    );
    assert.equal(writes, 1);
    await assert.rejects(
      call(owner.actor, {
        action: "byok.mail.disconnect",
        customerID,
        revision: uncertain.revision,
      }),
    );
    const checked = await call(owner.actor, {
      action: "byok.mail.refresh",
      customerID,
      revision: uncertain.revision,
    });
    assert.equal(checked.state, "ready");
    assert.equal(checked.pendingDomain, null);
    assert.equal(checked.domains[0].id, "recovered");
    const cipher = (
      await database.query(
        "SELECT encrypted_secret FROM webdock_auth.hosting_turbosmtp WHERE customer_id=$1",
        [customerID],
      )
    ).rows[0].encrypted_secret;
    globalThis.fetch = async () => {
      throw Error("provider unavailable");
    };
    await assert.rejects(
      call(owner.actor, {
        action: "byok.mail.connect",
        customerID,
        label: "Broken rotation",
        consumerKey: "other-" + key(),
        consumerSecret: "wrong",
        confirmAccountAccess: true,
        revision: checked.revision,
      }),
    );
    assert.equal(
      (
        await database.query(
          "SELECT encrypted_secret FROM webdock_auth.hosting_turbosmtp WHERE customer_id=$1",
          [customerID],
        )
      ).rows[0].encrypted_secret,
      cipher,
    );
  } finally {
    globalThis.fetch = original;
  }
});

test("shared custom images require explicit operator approval per project", async () => {
  await database.query(hostingSchemaSQL);
  const root = await identity(true),
    owner = await identity();
  const customerID = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Shared images fixture') RETURNING id",
    )
  ).rows[0].id;
  const org = (
    await database.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customerID],
    )
  ).rows[0].organization_id;
  await database.query(
    'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
    [org, owner.actor.subject],
  );
  const projects = (
    await database.query(
      "INSERT INTO webdock_admin.projects(id,name,customer_id,status) SELECT webdock_auth.next_snowflake(),'Image permission '||n,$1,'active' FROM generate_series(1,3) n RETURNING id",
      [customerID],
    )
  ).rows;
  const call = (actor: HostingActor, cmd: unknown) =>
    executeHosting(actor, cmd);
  const cluster = await call(root.actor, {
    action: "clusters.register",
    name: "Shared fixture",
    provider: "local",
    country: "DE",
    region: "local",
    locationEvidence: "Disposable fixture",
    idempotencyKey: key(),
  });
  const create = {
    action: "projects.create",
    projectID: projects[0].id,
    clusterID: cluster.id,
    provider: "k3s",
    mode: "selfservice",
    ownImages: true,
    idempotencyKey: key(),
  };
  await assert.rejects(call(root.actor, create));
  await call(root.actor, { ...create, confirmSharedImages: true });
  await assert.rejects(
    call(owner.actor, {
      ...create,
      projectID: projects[1].id,
      idempotencyKey: key(),
      confirmSharedImages: true,
    }),
  );
  await call(root.actor, {
    ...create,
    projectID: projects[1].id,
    ownImages: false,
    idempotencyKey: key(),
  });
  const update = {
    action: "projects.update",
    projectID: projects[1].id,
    mode: "selfservice",
    ownImages: true,
    revision: 1,
  };
  await assert.rejects(call(root.actor, update));
  await assert.rejects(
    call(owner.actor, { ...update, confirmSharedImages: true }),
  );
  await call(root.actor, { ...update, confirmSharedImages: true });
  await call(root.actor, { ...update, revision: 2, ownImages: false });
  const list = await call(root.actor, { action: "projects.list", customerID });
  assert.equal(
    list.docs.find(
      (p: { projectID: string; ownImages: boolean }) =>
        p.projectID === projects[0].id,
    ).ownImages,
    true,
  );
  assert.equal(
    list.docs.find(
      (p: { projectID: string; ownImages: boolean }) =>
        p.projectID === projects[1].id,
    ).ownImages,
    false,
  );
  await assert.rejects(
    call(root.actor, {
      ...create,
      projectID: projects[2].id,
      provider: "vercel",
      clusterID: undefined,
      confirmSharedImages: true,
      idempotencyKey: key(),
    }),
  );
  const other = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Foreign images fixture') RETURNING id",
    )
  ).rows[0].id;
  const dedicated = await call(root.actor, {
    action: "clusters.register",
    name: "Foreign dedicated",
    provider: "local",
    country: "DE",
    region: "local",
    locationEvidence: "Disposable fixture",
    dedicatedCustomerID: other,
    idempotencyKey: key(),
  });
  await assert.rejects(
    call(root.actor, {
      ...create,
      projectID: projects[2].id,
      clusterID: dedicated.id,
      confirmSharedImages: true,
      idempotencyKey: key(),
    }),
  );
});

test("Orama hosting search scans later batches without crossing tenant boundaries", async () => {
  await database.query(hostingSchemaSQL);
  const user = await identity();
  const customers = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Search tenant A'),(webdock_auth.next_snowflake(),'Search tenant B') RETURNING id",
    )
  ).rows.map((r) => r.id);
  const org = (
    await database.query(
      "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customers[0]],
    )
  ).rows[0].organization_id;
  await database.query(
    'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
    [org, user.actor.subject],
  );
  const projects = (
    await database.query(
      "INSERT INTO webdock_admin.projects(id,name,customer_id,status) SELECT webdock_auth.next_snowflake(),CASE WHEN n=301 THEN 'ZZ Orama garden' ELSE 'Other '||lpad(n::text,4,'0') END,$1,'active' FROM generate_series(1,301) n RETURNING id",
      [customers[0]],
    )
  ).rows;
  await database.query(
    "INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,mode) SELECT id,$1,'vercel','selfservice' FROM webdock_admin.projects WHERE id=ANY($2::varchar[])",
    [customers[0], projects.map((r) => r.id)],
  );
  const foreign = (
    await database.query(
      "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES(webdock_auth.next_snowflake(),'Orama private foreign',$1,'active') RETURNING id",
      [customers[1]],
    )
  ).rows[0];
  await database.query(
    "INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,mode) VALUES($1,$2,'vercel','selfservice')",
    [foreign.id, customers[1]],
  );
  const result = await executeHosting(user.actor, {
    action: "projects.list",
    customerID: customers[0],
    search: "orma",
    page: 1,
    limit: 20,
  });
  assert.equal(result.totalDocs, 1);
  assert.equal(result.docs[0].name, "ZZ Orama garden");
  await assert.rejects(
    executeHosting(user.actor, {
      action: "projects.list",
      customerID: customers[1],
      search: "orama",
      page: 1,
      limit: 20,
    }),
  );
});

test("operator relocation preserves identity and quota, rejects writers and stale restore evidence", async () => {
  const root = await identity(true),
    tenant = await identity();
  const customer = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'Relocation fixture') RETURNING id",
    )
  ).rows[0].id;
  const project = (
    await database.query(
      "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES(webdock_auth.next_snowflake(),'Relocation fixture',$1,'active') RETURNING id",
      [customer],
    )
  ).rows[0].id;
  const cap = normalizeHostingAllowances({
    apps: 4,
    cpuMillicores: 1000,
    memoryBytes: 2e9,
    volumeBytes: 1e9,
    ephemeralBytes: 1e9,
    replicasPerApp: 2,
    concurrentDeployments: 1,
  });
  const clusters = [];
  for (let n = 0; n < 2; n++) {
    const c = (await executeHosting(root.actor, {
      action: "clusters.register",
      name: "Relocate " + n,
      provider: "local",
      country: "DE",
      region: "local",
      locationEvidence: "Local disposable fixture",
      idempotencyKey: key(),
    })) as any;
    clusters.push(c.id);
    await database.query(
      "UPDATE webdock_auth.hosting_cluster SET verified=true,capacity=$2 WHERE id=$1",
      [c.id, JSON.stringify(cap)],
    );
    await database.query(
      "INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_seen,observation) VALUES($1,$2,1,now(),$3)",
      [
        c.id,
        key(),
        JSON.stringify({
          nodes: [{ id: "local" }],
          capacity: cap,
          capabilities: { storage: true, storageVersion: 1, environment: true },
        }),
      ],
    );
  }
  await executeHosting(root.actor, {
    action: "projects.create",
    projectID: project,
    provider: "k3s",
    clusterID: clusters[0],
    mode: "managed",
    ownImages: true,
    confirmSharedImages: true,
    idempotencyKey: key(),
  });
  const spec = {
    template: "custom",
    image: "example.invalid/test@sha256:" + "a".repeat(64),
    args: [],
    port: 8080,
    healthPath: "/health",
    cpuMillicores: 100,
    memoryBytes: 128e6,
    ephemeralBytes: 128e6,
    volumeBytes: 134217728,
    replicas: 0,
  };
  const app = (
    await database.query(
      "INSERT INTO webdock_auth.hosting_app(project_id,name,spec,observed_spec,revision,observed_revision,status,observed_uid) VALUES($1,'Relocate me',$2,$2,2,2,'stopped','old-uid') RETURNING id",
      [project, JSON.stringify(spec)],
    )
  ).rows[0].id;
  const op = (
    await database.query(
      "INSERT INTO webdock_auth.hosting_operation(subject,customer_id,project_id,cluster_id,app_id,action,idempotency_key,payload_hash,status,target_revision,desired,result) VALUES($1,$2,$3,$4,$5,'apps.stop',$6,$6,'succeeded',2,$7,$8) RETURNING id",
      [
        root.actor.subject,
        customer,
        project,
        clusters[0],
        app,
        key(),
        JSON.stringify({ action: "apply", spec }),
        JSON.stringify({
          proof: {
            appID: app,
            namespace: "wd-" + project,
            uid: "old-uid",
            revision: 2,
            ready: true,
            replicas: 0,
            deleted: false,
            storage: { volumeBytes: 134217728, retained: false },
          },
        }),
      ],
    )
  ).rows[0].id;
  await database.query(
    "INSERT INTO webdock_auth.hosting_reservation(operation_id,customer_id,project_id,cluster_id,app_id,demand,status) VALUES($1,$2,$3,$4,$5,$6,'active')",
    [
      op,
      customer,
      project,
      clusters[0],
      app,
      JSON.stringify({
        ...cap,
        apps: 1,
        cpuMillicores: 0,
        memoryBytes: 0,
        ephemeralBytes: 0,
        volumeBytes: 134217728,
        replicasPerApp: 0,
      }),
    ],
  );
  const command = {
    action: "projects.relocate",
    projectID: project,
    sourceClusterID: clusters[0],
    targetClusterID: clusters[1],
    revision: 1,
    idempotencyKey: key(),
    confirmSharedImages: true,
    checkedAt: new Date().toISOString(),
    evidence: [
      {
        appID: app,
        revision: 2,
        sourceUID: "old-uid",
        sourcePodsZero: true,
        targetPodsZero: true,
        backupSHA256: "b".repeat(64),
        restoredSHA256: "b".repeat(64),
      },
    ],
  };
  try {
    await assert.rejects(executeHosting(tenant.actor, command));
    await assert.rejects(
      executeHosting(root.actor, {
        ...command,
        checkedAt: "2000-01-01T00:00:00.000Z",
      }),
      /fresh/i,
    );
    await assert.rejects(
      executeHosting(root.actor, {
        ...command,
        evidence: [{ ...command.evidence[0], restoredSHA256: "c".repeat(64) }],
      }),
      /restore/i,
    );
    await database.query(
      "UPDATE webdock_auth.hosting_app SET status='ready',spec=jsonb_set(spec,'{replicas}','1') WHERE id=$1",
      [app],
    );
    await assert.rejects(executeHosting(root.actor, command), /stopped/i);
    await database.query(
      "UPDATE webdock_auth.hosting_app SET status='stopped',spec=$2 WHERE id=$1",
      [app, JSON.stringify(spec)],
    );
    await database.query(
      "UPDATE webdock_auth.hosting_agent SET last_seen=now()-interval '5 minutes' WHERE cluster_id=$1",
      [clusters[1]],
    );
    await assert.rejects(executeHosting(root.actor, command), /fresh/i);
    await database.query(
      "UPDATE webdock_auth.hosting_agent SET last_seen=now() WHERE cluster_id=$1",
      [clusters[1]],
    );
    await database.query(
      "UPDATE webdock_auth.hosting_cluster SET capacity=$2 WHERE id=$1",
      [clusters[1], JSON.stringify({ ...cap, volumeBytes: 0 })],
    );
    await assert.rejects(executeHosting(root.actor, command), /capacity/i);
    await database.query(
      "UPDATE webdock_auth.hosting_cluster SET capacity=$2 WHERE id=$1",
      [clusters[1], JSON.stringify(cap)],
    );
    const planName = "Relocation plan " + key();
    await managePlans(root.headers, {
      action: "create-plan",
      name: planName,
      "hosting.apps": "4",
      "hosting.cpuMillicores": "1000",
      "hosting.memoryBytes": "2000000000",
      "hosting.volumeBytes": "1000000000",
      "hosting.ephemeralBytes": "1000000000",
      "hosting.replicasPerApp": "2",
      "hosting.concurrentDeployments": "1",
    });
    const plan = (await getPlans(root.headers)).find(
      (p) => p.name === planName,
    )!;
    await managePlans(root.headers, {
      action: "assign",
      customerID: customer,
      planID: plan.id,
      revision: "0",
    });
    const blocker = await database.connect();
    const waitForBlocked = async (n: number) => {
      for (let attempt = 0; attempt < 100; attempt++) {
        const count = (
          await database.query(
            "SELECT count(*)::int AS n FROM pg_stat_activity WHERE datname=current_database() AND wait_event='advisory'",
          )
        ).rows[0].n;
        if (count >= n) return;
        await new Promise((resolve) => setTimeout(resolve, 10));
      }
      throw Error("Expected requests waiting at the shared plan lock");
    };
    let moving: Promise<any>, creating: Promise<any>;
    await blocker.query("BEGIN");
    await blocker.query(
      "SELECT pg_advisory_xact_lock(hashtextextended($1,0))",
      ["plan:" + customer],
    );
    try {
      moving = executeHosting(root.actor, command);
      await waitForBlocked(1);
      creating = executeHosting(root.actor, {
        action: "apps.create",
        projectID: project,
        name: "Queued behind relocation",
        spec: { ...spec, volumeBytes: 0 },
        subscriptionRevision: 1,
        idempotencyKey: key(),
      });
      await waitForBlocked(2);
    } finally {
      await blocker.query("COMMIT");
      blocker.release();
    }
    const moved = (await moving!) as any,
      created = (await creating!) as any;
    const queued = (
      await database.query(
        "SELECT cluster_id,desired FROM webdock_auth.hosting_operation WHERE id=$1",
        [created.id],
      )
    ).rows[0];
    assert.equal(
      queued.cluster_id,
      clusters[1],
      "A request queued behind relocation must use current placement",
    );
    assert.equal(queued.desired.clusterID, clusters[1]);
    assert.equal(moved.clusterID, clusters[1]);
    assert.deepEqual(await executeHosting(root.actor, command), moved);
    const saved = (
      await database.query(
        "SELECT * FROM webdock_auth.hosting_app WHERE id=$1",
        [app],
      )
    ).rows[0];
    assert.equal(saved.id, app);
    assert.equal(saved.spec.replicas, 0);
    assert.equal(saved.observed_uid, null);
    assert.equal(saved.revision, 3);
    const reservation = (
      await database.query(
        "SELECT * FROM webdock_auth.hosting_reservation WHERE app_id=$1",
        [app],
      )
    ).rows[0];
    assert.equal(reservation.cluster_id, clusters[1]);
    assert.equal(reservation.demand.volumeBytes, 134217728);
    assert.equal(reservation.status, "active");
    const source = (
      await database.query(
        "SELECT capacity FROM webdock_auth.hosting_cluster WHERE id=$1",
        [clusters[0]],
      )
    ).rows[0];
    assert.equal(
      source.capacity.volumeBytes,
      1e9 - 134217728,
      "Retained source data must still consume source capacity",
    );
  } finally {
    for (const t of [
      "hosting_reservation",
      "hosting_operation",
      "hosting_app",
      "hosting_project",
    ])
      await database.query(
        `DELETE FROM webdock_auth.${t} WHERE project_id=$1`,
        [project],
      );
    for (const t of ["hosting_enrollment", "hosting_agent"])
      await database.query(
        `DELETE FROM webdock_auth.${t} WHERE cluster_id=ANY($1)`,
        [clusters],
      );
    await database.query(
      "DELETE FROM webdock_auth.hosting_operation WHERE subject=ANY($1)",
      [[root.actor.subject, tenant.actor.subject]],
    );
    await database.query(
      "DELETE FROM webdock_auth.hosting_audit WHERE subject=ANY($1)",
      [[root.actor.subject, tenant.actor.subject]],
    );
    await database.query(
      "DELETE FROM webdock_auth.hosting_cluster WHERE id=ANY($1)",
      [clusters],
    );
    await database.query("DELETE FROM webdock_admin.projects WHERE id=$1", [
      project,
    ]);
    for (const t of [
      "plan_offer",
      "subscription_history",
      "tenant_subscription",
    ])
      await database.query(
        `DELETE FROM webdock_auth.${t} WHERE customer_id=$1`,
        [customer],
      );
    await database.query(
      "DELETE FROM webdock_auth.plan_template WHERE created_by=$1",
      [root.actor.subject],
    );
    await database.query(
      "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1",
      [customer],
    );
    await database.query("DELETE FROM webdock_admin.customers WHERE id=$1", [
      customer,
    ]);
  }
});
