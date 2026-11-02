import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { randomBytes, randomUUID } from "node:crypto";
import pg from "pg";
import { withDatabaseScope } from "@webdock/database-runtime";
import type { HostingActor } from "@webdock/hosting-contracts";
const fixtureFile = process.env.WEBDOCK_RLS_FIXTURE_FILE;
test(
  "customer order acceptance creates and upgrades a dedicated backend under constrained RLS",
  { skip: !fixtureFile },
  async () => {
    const ownerURL = process.env.DATABASE_URL!,
      url = new URL(ownerURL);
    assert.ok(["localhost", "127.0.0.1"].includes(url.hostname));
    assert.equal(url.pathname, "/webdock_rls_test");
    const fixture = JSON.parse(readFileSync(fixtureFile!, "utf8"));
    Object.assign(process.env, {
      DATABASE_URL: fixture.roles.webdock_auth_runtime,
      AUTH_DATABASE_URL: fixture.roles.webdock_auth_bootstrap,
      DATABASE_WORKER_URL: fixture.roles.webdock_worker_runtime,
      DATABASE_RLS_SECRET: fixture.contextSecret,
      BETTER_AUTH_SECRET: "synthetic-only-rls-test-".repeat(3),
      WEBDOCK_DATABASE_PUBLIC_ENABLED: "true",
      WEBDOCK_DATABASE_DOMAIN: "rls.invalid",
    });
    const { executeHosting } =
      await import("../../src/modules/hosting/service");
    const { database, authDatabase, workerDatabase } =
      await import("../../src/db/runtime");
    const owner = new pg.Pool({ connectionString: ownerURL, max: 1 });
    const base = BigInt(
        "92" + BigInt("0x" + randomBytes(5).toString("hex")).toString(),
      ),
      id = (i: number) => (base + BigInt(i)).toString();
    const customer = id(0),
      subject = id(1),
      session = id(2),
      project = id(3),
      cluster = id(4),
      planID = id(5),
      upgradePlan = id(6),
      sharedPlan = id(7),
      sharedBackend = id(8),
      poolID = randomUUID();
    const actor: HostingActor = {
      subject,
      sessionID: session,
      source: "studio",
      scopes: ["hosting:read", "hosting:write"],
    };
    const scope = <T>(run: () => T) =>
      withDatabaseScope({ kind: "session", subject, sessionID: session }, run);
    let org: string | undefined;
    const sponsoredPlanIDs: string[] = [];
    const key = () => randomBytes(20).toString("hex");
    const plan = {
      kind: "dedicated",
      name: "RLS dedicated",
      monthlyPriceCents: 990,
      currency: "EUR",
      postgresMajor: 18,
      cpuMillicores: 1000,
      memoryBytes: 2147483648,
      volumeBytes: 10737418240,
      logicalStorageAllowanceBytes: null,
      connectionLimit: 50,
      terms: "Synthetic local rehearsal. Backups are not configured.",
    };
    try {
      await owner.query(
        'INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword","twoFactorEnabled") VALUES($1,$1,$2,\'user\',true,false,false)',
        [subject, subject + "@rls.invalid"],
      );
      await owner.query(
        'INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval \'1 hour\',now())',
        [session, subject],
      );
      await owner.query(
        "INSERT INTO webdock_admin.customers(id,name,status) VALUES($1,$1,'active')",
        [customer],
      );
      org = (
        await owner.query(
          "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
          [customer],
        )
      ).rows[0].organization_id;
      await owner.query(
        'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'owner\',now())',
        [org, subject],
      );
      await owner.query(
        "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES($1,$1,$2,'active')",
        [project, customer],
      );
      const capacity = {
        apps: 100,
        cpuMillicores: 32000,
        memoryBytes: 128 * 1024 ** 3,
        volumeBytes: 1024 * 1024 ** 3,
        ephemeralBytes: 100 * 1024 ** 3,
        replicasPerApp: 10,
        concurrentDeployments: 20,
      };
      await owner.query(
        "INSERT INTO webdock_auth.hosting_cluster(id,name,provider,country,region,location_evidence,verified,capacity) VALUES($1::text,$1::text,'local','DE','test','Synthetic RLS fixture',true,$2)",
        [cluster, capacity],
      );
      const observation = {
        capacity,
        capabilities: {
          databaseVersion: 1,
          databaseNodePorts: [],
          databasePools: [
            {
              node: "rls-node",
              poolID,
              ready: true,
              capacityBytes: 100 * 1024 ** 3,
              availableBytes: 100 * 1024 ** 3,
              requestedCpuMillicores: 0,
              requestedMemoryBytes: 0,
            },
          ],
        },
        nodes: [
          {
            id: "rls-node",
            cpuMillicores: 32000,
            memoryBytes: 128 * 1024 ** 3,
          },
        ],
      };
      await owner.query(
        "INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,generation,last_seen,observation) VALUES($1,$2,1,now(),$3)",
        [cluster, randomBytes(32).toString("hex"), observation],
      );
      await owner.query(
        "INSERT INTO webdock_auth.hosting_database_pool(cluster_id,node,pool_id,capacity_bytes) VALUES($1,'rls-node',$2,$3)",
        [cluster, poolID, 100 * 1024 ** 3],
      );
      await owner.query(
        "INSERT INTO webdock_auth.hosting_database_plan(id,plan,availability) VALUES($1,$2,'public'),($3,$4,'public'),($5,$6,'public')",
        [
          planID,
          plan,
          upgradePlan,
          { ...plan, cpuMillicores: 2000 },
          sharedPlan,
          {
            ...plan,
            kind: "shared",
            cpuMillicores: null,
            memoryBytes: null,
            volumeBytes: null,
            logicalStorageAllowanceBytes: 1024 ** 3,
          },
        ],
      );
      await owner.query(
        "INSERT INTO webdock_auth.hosting_database_backend(id,cluster_id,kind,node,pool_id,state,observed_revision,demand,max_databases,logical_capacity_bytes,host,direct_port,pooled_port) VALUES($1,$2,'shared-pool','rls-node',$3,'ready',1,$4,10,$5,'shared.rls.invalid',31998,31999)",
        [
          sharedBackend,
          cluster,
          poolID,
          {
            apps: 1,
            cpuMillicores: 1000,
            memoryBytes: 2 * 1024 ** 3,
            volumeBytes: 10 * 1024 ** 3,
            ephemeralBytes: 1024 ** 3,
            replicasPerApp: 1,
            concurrentDeployments: 0,
          },
          10 * 1024 ** 3,
        ],
      );
      process.env.WEBDOCK_DATABASE_CLUSTER_ID = cluster;
      await assert.rejects(
        scope(() =>
          database.query(
            "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
            [customer],
          ),
        ),
        /permission denied/,
      );
      await assert.rejects(
        scope(() =>
          database.query("DELETE FROM webdock_admin.customers WHERE id=$1", [
            customer,
          ]),
        ),
        /permission denied/,
      );
      await assert.rejects(
        scope(() =>
          database.query(
            "UPDATE webdock_admin.projects SET name='denied' WHERE id=$1",
            [project],
          ),
        ),
        /permission denied/,
      );
      assert.equal(
        (
          await scope(() =>
            database.query(
              "UPDATE webdock_admin.customers SET name='Updated synthetic profile' WHERE id=$1",
              [customer],
            ),
          )
        ).rowCount,
        1,
      );
      const offer: any = await executeHosting(actor, {
        action: "databases.offers.create",
        projectID: project,
        planRevisionID: planID,
        name: "Dedicated RLS test",
        idempotencyKey: key(),
      });
      const command = {
        action: "databases.acceptOrder",
        projectID: project,
        offerID: offer.id,
        idempotencyKey: key(),
      };
      const created: any = await executeHosting(actor, command);
      assert.ok(created.databaseID);
      assert.deepEqual(
        await executeHosting(actor, command),
        created,
        "replay remains idempotent",
      );
      const backend = (
        await owner.query(
          "SELECT backend_id FROM webdock_auth.hosting_database WHERE id=$1",
          [created.databaseID],
        )
      ).rows[0].backend_id;
      assert.equal(
        (
          await scope(() =>
            database.query(
              "SELECT id FROM webdock_auth.hosting_database_backend WHERE id=$1",
              [backend],
            ),
          )
        ).rowCount,
        1,
      );
      await assert.rejects(
        scope(() =>
          database.query(
            "UPDATE webdock_auth.hosting_database_backend SET revision=revision+1 WHERE id=$1",
            [sharedBackend],
          ),
        ),
        /row-level security/,
      );
      await withDatabaseScope(
        {
          kind: "system",
          purpose: "database-order",
          customerID: customer,
          clusterID: cluster,
        },
        async () => {
          await assert.rejects(
            database.query(
              "UPDATE webdock_auth.hosting_database_backend SET revision=revision+1 WHERE id=$1",
              [sharedBackend],
            ),
            /row-level security/,
          );
        },
      );
      await withDatabaseScope(
        {
          kind: "system",
          purpose: "database-order",
          customerID: id(99),
          clusterID: cluster,
        },
        async () => {
          await assert.rejects(
            database.query(
              "UPDATE webdock_auth.hosting_database_backend SET revision=revision+1 WHERE id=$1",
              [backend],
            ),
            /row-level security/,
          );
        },
      );
      await owner.query(
        "UPDATE webdock_auth.hosting_database_plan SET availability='disabled' WHERE id=$1",
        [planID],
      );
      assert.equal(
        (
          await scope(() =>
            database.query(
              "SELECT id FROM webdock_auth.hosting_database_plan WHERE id=$1",
              [planID],
            ),
          )
        ).rowCount,
        1,
        "accepted plan terms remain visible after withdrawal",
      );
      const view: any = await executeHosting(actor, {
        action: "databases.get",
        databaseID: created.databaseID,
      });
      assert.equal(view.state, "provisioning");
      // Simulate only observation metadata; no hosting agent or external provisioning runs.
      await owner.query(
        "UPDATE webdock_auth.hosting_database SET state='ready',observed_revision=revision WHERE id=$1",
        [created.databaseID],
      );
      await owner.query(
        "UPDATE webdock_auth.hosting_database_backend SET state='ready',observed_revision=revision,cnpg_uid='synthetic-only' WHERE id=$1",
        [backend],
      );
      await owner.query(
        "UPDATE webdock_auth.hosting_operation SET status='succeeded' WHERE customer_id=$1",
        [customer],
      );
      const upgrade: any = await executeHosting(actor, {
        action: "databases.offers.create",
        projectID: project,
        planRevisionID: upgradePlan,
        databaseID: created.databaseID,
        name: "Dedicated upgrade",
        idempotencyKey: key(),
      });
      const upgraded: any = await executeHosting(actor, {
        action: "databases.acceptOrder",
        projectID: project,
        offerID: upgrade.id,
        confirmInterruption: true,
        idempotencyKey: key(),
      });
      assert.equal(upgraded.databaseID, created.databaseID);
      assert.equal(
        (
          await owner.query(
            "SELECT profile FROM webdock_auth.hosting_database_backend WHERE id=$1",
            [backend],
          )
        ).rows[0].profile.cpuMillicores,
        2000,
      );
      const sharedOffer: any = await executeHosting(actor, {
        action: "databases.offers.create",
        projectID: project,
        planRevisionID: sharedPlan,
        name: "Shared RLS test",
        idempotencyKey: key(),
      });
      const shared: any = await executeHosting(actor, {
        action: "databases.acceptOrder",
        projectID: project,
        offerID: sharedOffer.id,
        idempotencyKey: key(),
      });
      assert.ok(shared.databaseID);
      assert.equal(
        (
          await owner.query(
            "SELECT backend_id FROM webdock_auth.hosting_database WHERE id=$1",
            [shared.databaseID],
          )
        ).rows[0].backend_id,
        sharedBackend,
      );
      await owner.query(
        'INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword","twoFactorEnabled") VALUES($1,$1,$2,\'operator\',true,false,true)',
        [id(80), id(80) + "@rls.invalid"],
      );
      await owner.query(
        'INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval \'1 hour\',now())',
        [id(81), id(80)],
      );
      const operatorActor = { ...actor, subject: id(80), sessionID: id(81) };
      for (const target of [customer, id(99)]) {
        const sponsored: any = await executeHosting(operatorActor, {
          action: "databases.plans.create",
          plan: { ...plan, monthlyPriceCents: 0, sponsoredCustomerID: target },
          idempotencyKey: key(),
        });
        sponsoredPlanIDs.push(sponsored.id);
        await assert.rejects(
          executeHosting(operatorActor, {
            action: "databases.plans.setAvailability",
            planRevisionID: sponsored.id,
            availability: "public",
            revision: 1,
            idempotencyKey: key(),
          }),
          /Database plan changed/,
        );
        await executeHosting(operatorActor, {
          action: "databases.plans.setAvailability",
          planRevisionID: sponsored.id,
          availability: "pilot",
          revision: 1,
          idempotencyKey: key(),
        });
        const request = {
          action: "databases.offers.create",
          projectID: project,
          planRevisionID: sponsored.id,
          name: "Sponsored database",
          idempotencyKey: key(),
        };
        if (target !== customer) {
          await assert.rejects(
            executeHosting(operatorActor, request),
            /sponsorship belongs to another customer/,
          );
          continue;
        }
        const sponsoredOffer: any = await executeHosting(
          operatorActor,
          request,
        );
        assert.equal(sponsoredOffer.snapshot.billing, "sponsored");
        const sponsoredOrder: any = await executeHosting(operatorActor, {
          action: "databases.acceptOrder",
          projectID: project,
          offerID: sponsoredOffer.id,
          idempotencyKey: key(),
        });
        assert.ok(sponsoredOrder.databaseID);

      }
    } finally {
      const cleanup = await owner.connect();
      try {
        await cleanup.query("BEGIN");
        // This local-only owner cleanup must remove immutable synthetic orders and their cyclic FKs.
        await cleanup.query("SET LOCAL session_replication_role='replica'");

        await cleanup.query(
          "UPDATE webdock_auth.hosting_database SET operation_id=NULL WHERE customer_id=$1",
          [customer],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_database_job WHERE operation_id IN (SELECT id FROM webdock_auth.hosting_operation WHERE customer_id=$1)",
          [customer],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_operation WHERE customer_id=$1",
          [customer],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_audit WHERE subject=$1",
          [subject],
        );
        await cleanup.query(
          "UPDATE webdock_auth.hosting_database_order SET database_id=NULL WHERE customer_id=$1",
          [customer],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_database WHERE customer_id=$1",
          [customer],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_database_order WHERE customer_id=$1",
          [customer],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_database_offer WHERE customer_id=$1",
          [customer],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_database_port WHERE cluster_id=$1",
          [cluster],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_database_backend WHERE cluster_id=$1",
          [cluster],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_database_plan WHERE id=ANY($1)",
          [[planID, upgradePlan, sharedPlan, ...sponsoredPlanIDs]],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_database_pool WHERE cluster_id=$1",
          [cluster],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_agent WHERE cluster_id=$1",
          [cluster],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_cluster WHERE id=$1",
          [cluster],
        );
        await cleanup.query("DELETE FROM webdock_admin.projects WHERE id=$1", [
          project,
        ]);
        await cleanup.query(
          "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=$1",
          [customer],
        );
        await cleanup.query(
          'DELETE FROM webdock_auth.member WHERE "userId"=$1',
          [subject],
        );
        if (org)
          await cleanup.query(
            "DELETE FROM webdock_auth.organization WHERE id=$1",
            [org],
          );
        await cleanup.query("DELETE FROM webdock_admin.customers WHERE id=$1", [
          customer,
        ]);
        await cleanup.query("DELETE FROM webdock_auth.session WHERE id=$1", [
          session,
        ]);
        await cleanup.query('DELETE FROM webdock_auth."user" WHERE id=$1', [
          subject,
        ]);
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_operation WHERE subject=$1",
          [id(80)],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_audit WHERE subject=$1",
          [id(80)],
        );
        await cleanup.query("DELETE FROM webdock_auth.session WHERE id=$1", [
          id(81),
        ]);
        await cleanup.query('DELETE FROM webdock_auth."user" WHERE id=$1', [
          id(80),
        ]);
        await cleanup.query("COMMIT");
      } catch (error) {
        await cleanup.query("ROLLBACK");
        throw error;
      } finally {
        cleanup.release();
      }
      await Promise.all([
        owner.end(),
        database.end(),
        authDatabase.end(),
        workerDatabase.end(),
      ]);
    }
  },
);
