import { verifySetupDatabaseFlow } from "../helpers/setup-database-flow";
import { serviceBindingSchemaSQL } from "../../src/modules/hosting/service-binding-schema";
import { reportDatabaseObservations } from "../../src/modules/hosting/database-observations";
import { readFileSync } from "node:fs";
import { displayDatabaseCredentials } from "../../src/modules/hosting/database-credentials";
import {
  claimDatabaseOperation,
  checkpointDatabaseOperation,
  completeDatabaseOperation,
} from "../../src/modules/hosting/database-agent";
import { claimOperation } from "../../src/modules/hosting/operations";
import { nativeClusterDemand } from "@webdock/mail-core/capacity";
import test from "node:test";
import { subtest } from "../helpers/node-test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { auth } from "../../src/modules/auth/auth";
import { database } from "../../src/db/runtime";
import { createIdentity } from "../../src/modules/auth/bootstrap";
import { executeHosting } from "../../src/modules/hosting/service";
import {
  databaseRuntimeGrants,
  databaseSchemaSQL,
} from "../../src/modules/hosting/database-schema";
import type { HostingActor } from "@webdock/hosting-contracts";
const url = new URL(process.env.DATABASE_URL!);
if (
  url.hostname !== "127.0.0.1" ||
  url.pathname !== "/webdock_api_test" ||
  process.env.AUTH_TEST_MAIL !== "true"
)
  throw Error("Disposable local DB required");
const key = () => randomBytes(18).toString("hex");
async function identity(operator = false) {
  const password = key(),
    u = await createIdentity({
      email: `pg-${key()}@example.invalid`,
      name: "DB fixture",
      password,
      operator,
      mustChangePassword: false,
    });
  const response = await auth.api.signInEmail({
    body: { email: u.email, password },
    asResponse: true,
  });
  assert.equal(response.status, 200);
  if (operator)
    await database.query(
      'UPDATE webdock_auth."user" SET "twoFactorEnabled"=true WHERE id=$1',
      [u.id],
    );
  const session = await auth.api.getSession({
    headers: new Headers({
      Cookie: response.headers
        .getSetCookie()
        .map((c) => c.split(";")[0])
        .join("; "),
    }),
  });
  assert.ok(session);
  return {
    subject: u.id,
    sessionID: session.session.id,
    source: "studio",
    scopes: ["hosting:read", "hosting:write"],
  } satisfies HostingActor;
}
test.after(async () => {
  await auth.$context;
  await database.end();
});
test("database migration, operator catalog and tenant ordering use real transactions", async (t) => {
  await database.query(databaseSchemaSQL);
  await database.query(serviceBindingSchemaSQL);
  await database.query(databaseSchemaSQL);
  await subtest(
    "runtime grants permit provisioning but preserve order history and schema ownership",
    async () => {
      const role = "wd_test_runtime_" + key();
      const connection = await database.connect();
      try {
        await connection.query("BEGIN");
        await connection.query(`CREATE ROLE "${role}" NOLOGIN`);
        await connection.query(databaseRuntimeGrants(role));
        const rights = (
          await connection.query(
            `SELECT has_table_privilege($1,'webdock_auth.hosting_database','INSERT,UPDATE,SELECT') AS provision,
    has_table_privilege($1,'webdock_auth.hosting_database_order','DELETE') AS erase_order,
    has_table_privilege($1,'webdock_auth.hosting_database_port','DELETE') AS release_port,
    has_schema_privilege($1,'webdock_auth','CREATE') AS ddl`,
            [role],
          )
        ).rows[0];
        assert.deepEqual(rights, {
          provision: true,
          erase_order: false,
          release_port: true,
          ddl: false,
        });
        assert.throws(() => databaseRuntimeGrants("bad; DROP SCHEMA public"));
      } finally {
        await connection.query("ROLLBACK");
        connection.release();
      }
    },
  );
  const root = await identity(true),
    a = await identity(),
    b = await identity();
  const customer = (
    await database.query(
      "INSERT INTO webdock_admin.customers(id,name) VALUES(webdock_auth.next_snowflake(),'DB fixture') RETURNING id",
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
    [org, a.subject],
  );
  const project = (
    await database.query(
      "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES(webdock_auth.next_snowflake(),'DB fixture',$1,'active') RETURNING id",
      [customer],
    )
  ).rows[0].id;
  const plan = {
    kind: "shared",
    name: "Shared fixture",
    monthlyPriceCents: 590,
    currency: "EUR",
    postgresMajor: 18,
    cpuMillicores: null,
    memoryBytes: null,
    volumeBytes: null,
    logicalStorageAllowanceBytes: 1073741824,
    connectionLimit: 10,
    terms: "Shared resources and measured storage. Backups are not configured.",
  };
  let planID = "";
  await subtest(
    "tenant cannot publish tariffs and operator publication never provisions",
    async () => {
      await assert.rejects(
        executeHosting(a, {
          action: "databases.plans.create",
          plan,
          idempotencyKey: key(),
        }),
      );
      const created = await executeHosting(root, {
        action: "databases.plans.create",
        plan,
        idempotencyKey: key(),
      });
      planID = created.id;
      assert.equal(created.availability, "disabled");
      assert.deepEqual(
        await executeHosting(a, {
          action: "databases.list",
          projectID: project,
        }),
        [],
      );
    },
  );
  await subtest(
    "foreign tenant and preview cannot accept an offer",
    async () => {
      await assert.rejects(
        executeHosting(b, {
          action: "databases.offers.create",
          projectID: project,
          planRevisionID: planID,
          name: "DB",
          idempotencyKey: key(),
        }),
      );
      await database.query(
        "INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id,expires_at) VALUES($1,$2,$3,$4,now()+interval '10 minutes')",
        [root.sessionID, root.subject, customer, org],
      );
      await assert.rejects(
        executeHosting(root, {
          action: "databases.plans.setAvailability",
          planRevisionID: planID,
          availability: "pilot",
          revision: 1,
          idempotencyKey: key(),
        }),
      );
      await database.query(
        "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1",
        [root.sessionID],
      );
    },
  );
  await subtest(
    "no accepted order means no database; disabled plans cannot be ordered",
    async () => {
      await assert.rejects(
        executeHosting(a, {
          action: "databases.create",
          projectID: project,
          name: "DB",
          planRevisionID: planID,
          orderID: "1",
          kind: "shared",
          idempotencyKey: key(),
        }),
      );
      await assert.rejects(
        executeHosting(a, {
          action: "databases.offers.create",
          projectID: project,
          planRevisionID: planID,
          name: "DB",
          idempotencyKey: key(),
        }),
      );
      assert.equal(
        (
          await database.query(
            "SELECT count(*)::int AS n FROM webdock_auth.hosting_database WHERE project_id=$1",
            [project],
          )
        ).rows[0].n,
        0,
      );
    },
  );
  await subtest(
    "concurrent order retries provision once and last shared slot cannot be oversold",
    async () => {
      const old = process.env.WEBDOCK_DATABASE_PUBLIC_ENABLED;
      process.env.WEBDOCK_DATABASE_PUBLIC_ENABLED = "true";
      try {
        const cluster = (
          await database.query(
            "INSERT INTO webdock_auth.hosting_cluster(name,provider,country,region,location_evidence,verified,capacity) VALUES('DB cluster','local','DE','test','Disposable fixture',true,$1) RETURNING id",
            [
              JSON.stringify({
                apps: 20,
                cpuMillicores: 8000,
                memoryBytes: 16000000000,
                volumeBytes: 100000000000,
                ephemeralBytes: 8000000000,
                replicasPerApp: 4,
                concurrentDeployments: 2,
              }),
            ],
          )
        ).rows[0].id;
        const observation = {
          capabilities: { databaseVersion: 1 },
          nodes: [
            { id: "test-node", cpuMillicores: 10000, memoryBytes: 20000000000 },
          ],
        };
        const credentialHash = randomBytes(32).toString("hex");
        await database.query(
          "INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_seen,observation) VALUES($1,$2,1,now(),$3)",
          [cluster, credentialHash, JSON.stringify(observation)],
        );
        const backend = (
          await database.query(
            "INSERT INTO webdock_auth.hosting_database_backend(cluster_id,kind,node,pool_id,state,observed_revision,demand,max_databases,logical_capacity_bytes,host,direct_port,pooled_port) VALUES($1,'shared-pool','test-node','bd0ba8e4-e801-4c16-bf1b-060129437ccb','ready',1,$2,1,1073741824,'test.example.invalid',31001,31002) RETURNING id",
            [
              cluster,
              JSON.stringify({
                apps: 1,
                cpuMillicores: 1000,
                memoryBytes: 2684354560,
                volumeBytes: 32212254720,
                ephemeralBytes: 1073741824,
                replicasPerApp: 1,
                concurrentDeployments: 0,
              }),
            ],
          )
        ).rows[0].id;
        process.env.WEBDOCK_DATABASE_CLUSTER_ID = cluster;
        await executeHosting(root, {
          action: "databases.plans.setAvailability",
          planRevisionID: planID,
          availability: "public",
          revision: 1,
          idempotencyKey: key(),
        });
        const offer = await executeHosting(a, {
          action: "databases.offers.create",
          projectID: project,
          planRevisionID: planID,
          name: "Ordered DB",
          idempotencyKey: key(),
        });
        const command = {
          action: "databases.acceptOrder",
          projectID: project,
          offerID: offer.id,
          idempotencyKey: key(),
        };
        const [one, two] = await Promise.all([
          executeHosting(a, command),
          executeHosting(a, command),
        ]);
        assert.equal(one.databaseID, two.databaseID);
        const differentRetry = await executeHosting(a, {
          ...command,
          idempotencyKey: key(),
        });
        assert.equal(differentRetry.databaseID, one.databaseID);
        const view = await executeHosting(a, {
          action: "databases.get",
          databaseID: one.databaseID,
        });
        assert.equal(view.state, "provisioning");
        assert.equal(view.backup.status, "not-configured");
        assert.equal(view.memoryBytes, null);
        assert.equal(
          (
            await database.query(
              "SELECT billing_started_at FROM webdock_auth.hosting_database_order WHERE id=$1",
              [one.orderID],
            )
          ).rows[0].billing_started_at,
          null,
        );
        assert.equal(
          (await nativeClusterDemand(database, cluster)).memoryBytes,
          2684354560,
        );
        const agent = { clusterID: cluster, generation: 1, credentialHash };
        assert.equal(await claimOperation(agent), null);
        const packet = await claimDatabaseOperation(agent);
        assert.ok(packet?.database);
        assert.equal(packet.database.id, one.databaseID);
        const credentials = packet.database.credentials;
        assert.equal(packet.phase, "apply");
        assert.equal(packet.database.credentialRevision, 1);
        const lease = {
          operationID: packet.operationID,
          generation: packet.generation,
          leaseToken: packet.leaseToken,
        };
        await assert.rejects(
          checkpointDatabaseOperation(
            { ...agent, clusterID: "1" },
            { ...lease, phase: "observing", cnpgUID: "fixture-cnpg" },
          ),
        );
        await checkpointDatabaseOperation(agent, {
          ...lease,
          phase: "applied",
          cnpgUID: "fixture-cnpg",
          caCertificate: readFileSync(
            new URL("./fixtures/database-ca.crt", import.meta.url),
            "utf8",
          ),
        });
        await checkpointDatabaseOperation(agent, {
          ...lease,
          phase: "configuring",
          cnpgUID: "fixture-cnpg",
        });
        await checkpointDatabaseOperation(agent, {
          ...lease,
          phase: "observing",
          cnpgUID: "fixture-cnpg",
        });
        const observationPacket = await claimDatabaseOperation(agent);
        assert.ok(observationPacket);
        assert.equal(observationPacket.phase, "observe");
        await assert.rejects(
          completeDatabaseOperation(agent, {
            ...lease,
            outcome: "succeeded",
            proof: {
              installationID: backend,
              databaseID: one.databaseID,
              cnpgUID: "wrong",
              revision: 1,
              state: "ready",
              endpointReady: true,
            },
          }),
        );
        await completeDatabaseOperation(agent, {
          ...lease,
          outcome: "succeeded",
          proof: {
            installationID: backend,
            databaseID: one.databaseID,
            cnpgUID: "fixture-cnpg",
            revision: 1,
            state: "ready",
            endpointReady: true,
          },
        });
        const ready = await executeHosting(a, {
          action: "databases.get",
          databaseID: one.databaseID,
        });
        assert.equal(ready.state, "ready");
        assert.equal(
          JSON.stringify(ready).includes(credentials.applicationPassword),
          false,
        );
        const billing = (
          await database.query(
            "SELECT billing_started_at FROM webdock_auth.hosting_database_order WHERE id=$1",
            [one.orderID],
          )
        ).rows[0].billing_started_at;
        assert.ok(billing);
        await completeDatabaseOperation(agent, {
          ...lease,
          outcome: "succeeded",
          proof: {
            installationID: backend,
            databaseID: one.databaseID,
            cnpgUID: "fixture-cnpg",
            revision: 1,
            state: "ready",
            endpointReady: true,
          },
        });
        assert.equal(
          (
            await database.query(
              "SELECT billing_started_at FROM webdock_auth.hosting_database_order WHERE id=$1",
              [one.orderID],
            )
          ).rows[0].billing_started_at.toISOString(),
          billing.toISOString(),
        );
        assert.equal(
          (await displayDatabaseCredentials(a, one.databaseID)).application
            .password,
          credentials.applicationPassword,
        );
        await assert.rejects(
          displayDatabaseCredentials({ ...a, source: "oauth" }, one.databaseID),
        );
        await assert.rejects(displayDatabaseCredentials(b, one.databaseID));
        await database.query(
          "INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,mode,cluster_id,namespace) VALUES($1,$2,'k3s','selfservice',$3,$4)",
          [project, customer, cluster, "wd-" + project],
        );
        await database.query(
          "UPDATE webdock_auth.hosting_agent SET observation=jsonb_set(observation,'{capabilities,serviceBindingsVersion}','1') WHERE cluster_id=$1",
          [cluster],
        );
        const limits = await executeHosting(root, {
          action: "limits.get",
          customerID: customer,
        });
        const draft = await executeHosting(root, {
          action: "apps.prepare",
          projectID: project,
          name: "Bound application",
          spec: {
            args: [],
            port: 8080,
            healthPath: "/",
            cpuMillicores: 100,
            memoryBytes: 67108864,
            ephemeralBytes: 67108864,
            replicas: 1,
          },
          subscriptionRevision: limits.subscriptionRevision,
          idempotencyKey: key(),
        });
        const bind = {
          action: "services.bind",
          projectID: project,
          appID: draft.appID,
          service: {
            kind: "postgres",
            databaseID: one.databaseID,
            variable: "DATABASE_URL",
          },
          revision: 0,
          idempotencyKey: key(),
        };
        const bound = await executeHosting(root, bind);
        assert.equal((await executeHosting(root, bind)).id, bound.id);
        assert.equal(bound.state, "pending");
        assert.equal(
          JSON.stringify(bound).includes(credentials.applicationPassword),
          false,
        );
        const appView = await executeHosting(root, {
          action: "apps.get",
          appID: draft.appID,
        });
        assert.deepEqual(appView.environmentNames, ["DATABASE_URL"]);
        await database.query(
          "UPDATE webdock_auth.hosting_database SET credential_revision=credential_revision+1 WHERE id=$1",
          [one.databaseID],
        );
        const stale = await executeHosting(root, {
          action: "services.list",
          projectID: project,
          appID: draft.appID,
        });
        assert.equal(stale[0].state, "blocked");
        await database.query(
          "UPDATE webdock_auth.hosting_database SET credential_revision=credential_revision-1 WHERE id=$1",
          [one.databaseID],
        );
        await assert.rejects(executeHosting(b, bind));
        await assert.rejects(
          executeHosting(
            { ...root, scopes: ["hosting:read"] },
            { ...bind, idempotencyKey: key() },
          ),
        );
        await assert.rejects(
          executeHosting(root, {
            action: "services.disconnect",
            projectID: project,
            bindingID: bound.id,
            revision: 0,
            idempotencyKey: key(),
          }),
        );
        await executeHosting(root, {
          action: "services.disconnect",
          projectID: project,
          bindingID: bound.id,
          revision: bound.revision,
          idempotencyKey: key(),
        });
        assert.deepEqual(
          (
            await executeHosting(root, {
              action: "apps.get",
              appID: draft.appID,
            })
          ).environmentNames,
          [],
        );
        await verifySetupDatabaseFlow(
          root,
          project,
          one.databaseID,
          planID,
          agent,
        );
        // Dedicated orders reserve a physical installation; upgrades reserve their delta before execution.
        await database.query(
          "UPDATE webdock_auth.hosting_agent SET observation=$2 WHERE cluster_id=$1",
          [
            cluster,
            JSON.stringify({
              ...observation,
              capacity: { volumeBytes: 0 },
              capabilities: {
                databaseVersion: 1,
                databasePools: [
                  {
                    node: "test-node",
                    poolID: "bd0ba8e4-e801-4c16-bf1b-060129437ccb",
                    ready: true,
                    capacityBytes: 100000000000,
                    availableBytes: 100000000000,
                    requestedCpuMillicores: 1000,
                    requestedMemoryBytes: 2147483648,
                  },
                ],
                databaseNodePorts: [],
              },
            }),
          ],
        );
        await executeHosting(root, {
          action: "databases.pools.register",
          clusterID: cluster,
          node: "test-node",
          poolID: "bd0ba8e4-e801-4c16-bf1b-060129437ccb",
          revision: 1,
          idempotencyKey: key(),
        });
        process.env.WEBDOCK_DATABASE_DOMAIN = "example.invalid";
        const dedicatedPlan = {
          ...plan,
          kind: "dedicated",
          name: "Dedicated fixture",
          monthlyPriceCents: 1490,
          cpuMillicores: 500,
          memoryBytes: 1073741824,
          volumeBytes: 10737418240,
          logicalStorageAllowanceBytes: null,
        };
        const dedicatedCatalog = await executeHosting(root, {
          action: "databases.plans.create",
          plan: dedicatedPlan,
          idempotencyKey: key(),
        });
        await executeHosting(root, {
          action: "databases.plans.setAvailability",
          planRevisionID: dedicatedCatalog.id,
          revision: 1,
          availability: "public",
          idempotencyKey: key(),
        });
        const dedicatedOffer = await executeHosting(a, {
          action: "databases.offers.create",
          projectID: project,
          planRevisionID: dedicatedCatalog.id,
          name: "Dedicated",
          idempotencyKey: key(),
        });
        const dedicated = await executeHosting(a, {
          action: "databases.acceptOrder",
          offerID: dedicatedOffer.id,
          projectID: project,
          idempotencyKey: key(),
        });
        const finish = async (state: "ready" | "paused" = "ready") => {
          const job = await claimDatabaseOperation(agent);
          assert.ok(job?.database);
          const identity = {
            operationID: job.operationID,
            generation: job.generation,
            leaseToken: job.leaseToken,
          };
          const uid = job.backend.cnpgUID ?? "dedicated-cnpg";
          await checkpointDatabaseOperation(agent, {
            ...identity,
            phase: "applied",
            cnpgUID: uid,
            caCertificate: readFileSync(
              new URL("./fixtures/database-ca.crt", import.meta.url),
              "utf8",
            ),
          });
          for (const phase of ["configuring", "observing"])
            await checkpointDatabaseOperation(agent, {
              ...identity,
              phase,
              cnpgUID: uid,
            });
          await completeDatabaseOperation(agent, {
            ...identity,
            outcome: "succeeded",
            proof: {
              installationID: job.backend.id,
              databaseID: job.database.id,
              cnpgUID: uid,
              revision: job.revision,
              state,
              endpointReady: true,
            },
          });
        };
        await finish();
        const upgradeCatalog = await executeHosting(root, {
          action: "databases.plans.create",
          plan: {
            ...dedicatedPlan,
            name: "Larger",
            monthlyPriceCents: 2490,
            cpuMillicores: 1000,
            memoryBytes: 2147483648,
          },
          idempotencyKey: key(),
        });
        await executeHosting(root, {
          action: "databases.plans.setAvailability",
          planRevisionID: upgradeCatalog.id,
          revision: 1,
          availability: "public",
          idempotencyKey: key(),
        });
        const upgrade = await executeHosting(a, {
          action: "databases.offers.create",
          projectID: project,
          planRevisionID: upgradeCatalog.id,
          databaseID: dedicated.databaseID,
          name: "Dedicated",
          idempotencyKey: key(),
        });
        await assert.rejects(
          executeHosting(a, {
            action: "databases.acceptOrder",
            offerID: upgrade.id,
            projectID: project,
            idempotencyKey: key(),
          }),
        );
        const acceptedUpgrade = await executeHosting(a, {
          action: "databases.acceptOrder",
          offerID: upgrade.id,
          projectID: project,
          confirmInterruption: true,
          idempotencyKey: key(),
        });
        assert.equal(acceptedUpgrade.databaseID, dedicated.databaseID);
        assert.equal(
          (
            await executeHosting(a, {
              action: "databases.get",
              databaseID: dedicated.databaseID,
            })
          ).state,
          "updating",
        );
        await finish();
        assert.equal(
          (
            await executeHosting(a, {
              action: "databases.get",
              databaseID: dedicated.databaseID,
            })
          ).memoryBytes,
          2147483648,
        );
        assert.equal(
          (await nativeClusterDemand(database, cluster)).memoryBytes,
          5368709120,
        );
        assert.equal(
          (
            await executeHosting(a, {
              action: "databases.acceptOrder",
              offerID: dedicatedOffer.id,
              projectID: project,
              idempotencyKey: key(),
            })
          ).databaseID,
          dedicated.databaseID,
        );
        await assert.rejects(
          database.query(
            "UPDATE webdock_auth.hosting_database_plan SET plan=$2 WHERE id=$1",
            [
              dedicatedCatalog.id,
              JSON.stringify({ ...dedicatedPlan, monthlyPriceCents: 1 }),
            ],
          ),
        );
        await database.query(
          "UPDATE webdock_auth.hosting_database_backend SET max_databases=2,logical_capacity_bytes=2147483648 WHERE id=$1",
          [backend],
        );
        const movingOffer = await executeHosting(a, {
          action: "databases.offers.create",
          projectID: project,
          planRevisionID: planID,
          name: "Moving DB",
          idempotencyKey: key(),
        });
        const moving = await executeHosting(a, {
          action: "databases.acceptOrder",
          offerID: movingOffer.id,
          projectID: project,
          idempotencyKey: key(),
        });
        await finish();
        await database.query(
          "UPDATE webdock_auth.hosting_database SET usage=$2 WHERE id=$1",
          [
            moving.databaseID,
            JSON.stringify({
              logicalBytes: 1048576,
              filesystemBytes: null,
              observedAt: new Date().toISOString(),
            }),
          ],
        );
        const moveOffer = await executeHosting(a, {
          action: "databases.offers.create",
          projectID: project,
          planRevisionID: dedicatedCatalog.id,
          databaseID: moving.databaseID,
          name: "Moving DB",
          idempotencyKey: key(),
        });
        await executeHosting(a, {
          action: "databases.acceptOrder",
          offerID: moveOffer.id,
          projectID: project,
          confirmInterruption: true,
          idempotencyKey: key(),
        });
        const preparation = await claimDatabaseOperation(agent);
        assert.ok(preparation);
        assert.equal(preparation.action, "prepareMigration");
        assert.equal(preparation.database, null);
        const prepLease = {
          operationID: preparation.operationID,
          generation: preparation.generation,
          leaseToken: preparation.leaseToken,
        };
        for (const phase of ["applied", "configuring", "observing"])
          await checkpointDatabaseOperation(agent, {
            ...prepLease,
            phase,
            cnpgUID: "migration-target",
          });
        await completeDatabaseOperation(agent, {
          ...prepLease,
          outcome: "succeeded",
          proof: {
            installationID: preparation.backend.id,
            databaseID: null,
            cnpgUID: "migration-target",
            revision: preparation.revision,
            state: "ready",
            endpointReady: true,
          },
        });
        assert.equal(
          (
            await executeHosting(a, {
              action: "databases.get",
              databaseID: moving.databaseID,
            })
          ).kind,
          "shared",
        );
        const transfer = await claimDatabaseOperation(agent);
        assert.ok(transfer?.database && transfer.migration);
        assert.equal(transfer.action, "migrate");
        assert.equal(transfer.operationID, preparation.operationID);
        const transferLease = {
          operationID: transfer.operationID,
          generation: transfer.generation,
          leaseToken: transfer.leaseToken,
        };
        await assert.rejects(
          checkpointDatabaseOperation(agent, {
            ...prepLease,
            phase: "applied",
            cnpgUID: "migration-target",
          }),
        );
        for (const phase of ["applied", "configuring"])
          await checkpointDatabaseOperation(agent, {
            ...transferLease,
            phase,
            cnpgUID: "migration-target",
          });
        await checkpointDatabaseOperation(agent, {
          ...transferLease,
          phase: "observing",
          cnpgUID: "migration-target",
          migrationJobUID: "logical-copy-job",
          copySHA256: "a".repeat(64),
        });
        await completeDatabaseOperation(agent, {
          ...transferLease,
          outcome: "succeeded",
          proof: {
            installationID: transfer.backend.id,
            databaseID: moving.databaseID,
            cnpgUID: "migration-target",
            revision: transfer.revision,
            state: "ready",
            endpointReady: true,
            copySHA256: "a".repeat(64),
          },
        });
        assert.equal(
          (
            await executeHosting(a, {
              action: "databases.get",
              databaseID: moving.databaseID,
            })
          ).kind,
          "dedicated",
        );
        assert.equal(
          (
            await database.query(
              "SELECT state FROM webdock_auth.hosting_database_backend WHERE id=$1",
              [backend],
            )
          ).rows[0].state,
          "ready",
        );
        assert.equal(
          (
            await database.query(
              "SELECT state FROM webdock_auth.hosting_database_migration WHERE database_id=$1",
              [moving.databaseID],
            )
          ).rows[0].state,
          "retained",
        );
        const phantom = (
          await database.query(
            "INSERT INTO webdock_auth.hosting_database_backend(cluster_id,kind,node,pool_id,state,demand,max_databases,logical_capacity_bytes,host,direct_port,pooled_port) VALUES($1,'dedicated','test-node','bd0ba8e4-e801-4c16-bf1b-060129437ccb','paused',$2,1,0,'held.example.invalid',31011,31012) RETURNING id",
            [
              cluster,
              JSON.stringify({
                apps: 1,
                cpuMillicores: 100,
                memoryBytes: 1073741824,
                volumeBytes: 10737418240,
                ephemeralBytes: 0,
                replicasPerApp: 1,
                concurrentDeployments: 0,
              }),
            ],
          )
        ).rows[0].id;
        const tightObservation = {
          ...observation,
          nodes: [
            { id: "test-node", cpuMillicores: 10000, memoryBytes: 12884901888 },
          ],
          capabilities: {
            databaseVersion: 1,
            databasePools: [
              {
                node: "test-node",
                poolID: "bd0ba8e4-e801-4c16-bf1b-060129437ccb",
                ready: true,
                capacityBytes: 100000000000,
                availableBytes: 100000000000,
                requestedCpuMillicores: 1000,
                requestedMemoryBytes: 1073741824,
                observedDatabases: [],
              },
            ],
            databaseNodePorts: [],
          },
        };
        await database.query(
          "UPDATE webdock_auth.hosting_agent SET observation=$2 WHERE cluster_id=$1",
          [cluster, JSON.stringify(tightObservation)],
        );
        const noRoom = await executeHosting(a, {
          action: "databases.offers.create",
          projectID: project,
          planRevisionID: dedicatedCatalog.id,
          name: "Must not fit",
          idempotencyKey: key(),
        });
        await assert.rejects(
          executeHosting(a, {
            action: "databases.acceptOrder",
            offerID: noRoom.id,
            projectID: project,
            idempotencyKey: key(),
          }),
          /node|capacity/i,
        );
        await database.query(
          "UPDATE webdock_auth.hosting_database_backend SET state='purged' WHERE id=$1",
          [phantom],
        );
        await reportDatabaseObservations(agent, {
          observedAt: new Date().toISOString(),
          installations: [
            {
              installationID: backend,
              cnpgUID: "fixture-cnpg",
              ready: true,
              filesystemBytes: 2097152,
              databases: [{ id: one.databaseID, logicalBytes: 1048576 }],
            },
          ],
        });
        const measured = await executeHosting(a, {
          action: "databases.get",
          databaseID: one.databaseID,
        });
        assert.equal(measured.usage.logicalBytes, 1048576);
        assert.equal(measured.usage.filesystemBytes, null);
        await assert.rejects(
          reportDatabaseObservations(agent, {
            observedAt: new Date(Date.now() + 60000).toISOString(),
            installations: [],
          }),
        );
        const paused = await executeHosting(a, {
          action: "databases.pause",
          databaseID: one.databaseID,
          revision: 1,
          idempotencyKey: key(),
        });
        assert.equal(paused.state, "pausing");
        assert.equal(paused.revision, 2);
        await assert.rejects(
          executeHosting(a, {
            action: "databases.resume",
            databaseID: one.databaseID,
            revision: 1,
            idempotencyKey: key(),
          }),
        );
        assert.equal(
          (await nativeClusterDemand(database, cluster)).memoryBytes,
          6979321856,
        );
        const pausePacket = await claimDatabaseOperation(agent);
        assert.ok(pausePacket);
        assert.equal(pausePacket.action, "pause");
        const pauseLease = {
          operationID: pausePacket.operationID,
          generation: pausePacket.generation,
          leaseToken: pausePacket.leaseToken,
        };
        for (const phase of ["applied", "configuring", "observing"])
          await checkpointDatabaseOperation(agent, {
            ...pauseLease,
            phase,
            cnpgUID: "fixture-cnpg",
          });
        await completeDatabaseOperation(agent, {
          ...pauseLease,
          outcome: "succeeded",
          proof: {
            installationID: backend,
            databaseID: one.databaseID,
            cnpgUID: "fixture-cnpg",
            revision: 2,
            state: "paused",
            endpointReady: true,
          },
        });
        assert.equal(
          (
            await database.query(
              "SELECT state FROM webdock_auth.hosting_database_backend WHERE id=$1",
              [backend],
            )
          ).rows[0].state,
          "ready",
        );
        const cancel = await executeHosting(a, {
          action: "databases.requestCancellation",
          databaseID: one.databaseID,
          revision: 2,
          idempotencyKey: key(),
        });
        assert.ok(cancel.cancellationRequestedAt);
        await assert.rejects(
          executeHosting(a, {
            action: "databases.deletionPreview",
            databaseID: one.databaseID,
          }),
        );
        const preview = await executeHosting(root, {
          action: "databases.deletionPreview",
          databaseID: one.databaseID,
        });
        await assert.rejects(
          executeHosting(root, {
            action: "databases.delete",
            databaseID: one.databaseID,
            revision: 2,
            confirmName: "wrong",
            planHash: preview.planHash,
            idempotencyKey: key(),
          }),
        );
        await assert.rejects(
          executeHosting(a, {
            action: "databases.backup",
            databaseID: one.databaseID,
            revision: 2,
            idempotencyKey: key(),
          }),
          /not configured/,
        );
        const stored = (
          await database.query(
            "SELECT credentials_encrypted FROM webdock_auth.hosting_database WHERE id=$1",
            [one.databaseID],
          )
        ).rows[0].credentials_encrypted;
        assert.ok(stored);
        assert.equal(JSON.stringify(view).includes(stored), false);
        const second = await executeHosting(a, {
          action: "databases.offers.create",
          projectID: project,
          planRevisionID: planID,
          name: "Over capacity",
          idempotencyKey: key(),
        });
        await assert.rejects(
          executeHosting(a, {
            ...command,
            offerID: second.id,
            idempotencyKey: key(),
          }),
        );
        assert.equal(
          (
            await database.query(
              "SELECT count(*)::int AS n FROM webdock_auth.hosting_database WHERE backend_id=$1",
              [backend],
            )
          ).rows[0].n,
          1,
        );
        await assert.rejects(
          executeHosting(b, {
            action: "databases.get",
            databaseID: one.databaseID,
          }),
        );
        await assert.rejects(
          executeHosting(a, {
            action: "databases.restart",
            databaseID: one.databaseID,
            revision: 1,
            idempotencyKey: key(),
          }),
        );
        const history = await executeHosting(root, {
          action: "databases.migrations.list",
          databaseID: moving.databaseID,
        });
        const retainedID = history[0].id;
        await assert.rejects(
          executeHosting(a, {
            action: "databases.migrationPurgePreview",
            migrationID: retainedID,
          }),
        );
        let cleanup = await executeHosting(root, {
          action: "databases.migrationPurgePreview",
          migrationID: retainedID,
        });
        await assert.rejects(
          executeHosting(root, {
            action: "databases.purgeMigrationSource",
            migrationID: retainedID,
            revision: cleanup.revision,
            confirmName: "Moving DB",
            planHash: cleanup.planHash,
            idempotencyKey: key(),
          }),
        );
        await database.query(
          "UPDATE webdock_auth.hosting_database_migration SET retained_until=now()-interval '1 second' WHERE id=$1",
          [retainedID],
        );
        cleanup = await executeHosting(root, {
          action: "databases.migrationPurgePreview",
          migrationID: retainedID,
        });
        await executeHosting(root, {
          action: "databases.purgeMigrationSource",
          migrationID: retainedID,
          revision: cleanup.revision,
          confirmName: "Moving DB",
          planHash: cleanup.planHash,
          idempotencyKey: key(),
        });
        const cleanJob = await claimDatabaseOperation(agent);
        assert.ok(cleanJob?.database);
        assert.equal(cleanJob.action, "purge");
        assert.equal(cleanJob.backend.id, backend);
        const cleanLease = {
          operationID: cleanJob.operationID,
          generation: cleanJob.generation,
          leaseToken: cleanJob.leaseToken,
        };
        for (const phase of ["applied", "configuring", "observing"])
          await checkpointDatabaseOperation(agent, {
            ...cleanLease,
            phase,
            cnpgUID: "fixture-cnpg",
          });
        await completeDatabaseOperation(agent, {
          ...cleanLease,
          outcome: "succeeded",
          proof: {
            installationID: backend,
            databaseID: moving.databaseID,
            cnpgUID: "fixture-cnpg",
            revision: cleanJob.revision,
            state: "purged",
            endpointReady: true,
          },
        });
        assert.equal(
          (
            await executeHosting(a, {
              action: "databases.get",
              databaseID: moving.databaseID,
            })
          ).state,
          "ready",
        );
        assert.equal(
          (
            await database.query(
              "SELECT state FROM webdock_auth.hosting_database_backend WHERE id=$1",
              [backend],
            )
          ).rows[0].state,
          "ready",
        );
        assert.equal(
          (
            await database.query(
              "SELECT state FROM webdock_auth.hosting_database_migration WHERE id=$1",
              [retainedID],
            )
          ).rows[0].state,
          "purged",
        );
        const beforeExpiry = await executeHosting(a, {
          action: "databases.get",
          databaseID: moving.databaseID,
        });
        await executeHosting(a, {
          action: "databases.pause",
          databaseID: moving.databaseID,
          revision: beforeExpiry.revision,
          idempotencyKey: key(),
        });
        const expiring = await claimDatabaseOperation(agent);
        assert.ok(expiring);
        await database.query(
          "UPDATE webdock_auth.hosting_operation SET lease_until=now()-interval '1 second' WHERE id=$1",
          [expiring.operationID],
        );
        assert.equal(await claimDatabaseOperation(agent), null);
        assert.equal(
          (
            await executeHosting(a, {
              action: "databases.get",
              databaseID: moving.databaseID,
            })
          ).state,
          "needs-reconciliation",
          "expired worker must expose operator recovery instead of leaving a permanent pausing state",
        );
      } finally {
        if (old === undefined)
          delete process.env.WEBDOCK_DATABASE_PUBLIC_ENABLED;
        else process.env.WEBDOCK_DATABASE_PUBLIC_ENABLED = old;
        delete process.env.WEBDOCK_DATABASE_CLUSTER_ID;
        delete process.env.WEBDOCK_DATABASE_DOMAIN;
      }
    },
  );
  await subtest(
    "reusing idempotency key with changed price is rejected",
    async () => {
      const idempotencyKey = key();
      const first = await executeHosting(root, {
        action: "databases.plans.create",
        plan,
        idempotencyKey,
      });
      assert.deepEqual(
        await executeHosting(root, {
          action: "databases.plans.create",
          plan,
          idempotencyKey,
        }),
        first,
      );
      await assert.rejects(
        executeHosting(root, {
          action: "databases.plans.create",
          plan: { ...plan, monthlyPriceCents: 600 },
          idempotencyKey,
        }),
      );
    },
  );
});
