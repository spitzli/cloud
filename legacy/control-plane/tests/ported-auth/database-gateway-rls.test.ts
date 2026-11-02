import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { randomBytes } from "node:crypto";
import pg from "pg";
import { withDatabaseScope } from "@webdock/database-runtime";
import type { HostingActor } from "@webdock/hosting-contracts";
const fixtureFile = process.env.WEBDOCK_RLS_FIXTURE_FILE;
test(
  "strict database roles preserve launch exchange, runtime profile, inspection and revocation",
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
      BETTER_AUTH_SECRET: "synthetic-gateway-rls-".repeat(3),
      WEBDOCK_DATABASE_GATEWAY_SECRET: "synthetic-gateway-secret-".repeat(3),
      WEBDOCK_DATABASE_GATEWAY_ORIGIN: "https://gateway.rls.invalid",
    });
    const { database, authDatabase, workerDatabase } =
      await import("../../src/db/runtime");
    const { executeDatabase, exchangeLaunch, inspectSession } =
      await import("../../src/modules/databases/service");
    const { databaseGateway } =
      await import("../../src/modules/databases/http");
    const { transaction } = await import("../../src/modules/hosting/db");
    const { encryptMailSecret } =
      await import("../../src/modules/platform/platform");
    const owner = new pg.Pool({ connectionString: ownerURL, max: 1 });
    const base = BigInt(
        "94" + BigInt("0x" + randomBytes(5).toString("hex")).toString(),
      ),
      id = (i: number) => (base + BigInt(i)).toString();
    const customer = id(0),
      subject = id(1),
      session = id(2),
      project = id(3),
      binding = id(4),
      grant = id(5),
      origin = "https://runtime-" + subject + ".rls.invalid",
      connection = "platform-own-site",
      proxySecret = "synthetic-proxy-secret-".repeat(3);
    const actor: HostingActor = {
      subject,
      sessionID: session,
      source: "studio",
      scopes: ["hosting:read", "hosting:write"],
    };
    const user = <T>(run: () => T) =>
      withDatabaseScope({ kind: "session", subject, sessionID: session }, run);
    const broker = <T>(run: () => T) =>
      withDatabaseScope({ kind: "system", purpose: "database-gateway" }, run);
    const open = () =>
      user(() =>
        transaction((db) =>
          executeDatabase(db, actor, { action: "open", bindingID: binding }),
        ),
      ) as Promise<{ code: string }>;
    const http = (
      body: unknown,
      credential = process.env.WEBDOCK_DATABASE_GATEWAY_SECRET,
    ) =>
      databaseGateway(
        new Request("https://auth.rls.invalid/api/databases/gateway", {
          method: "POST",
          headers: {
            Authorization: "Bearer " + credential,
            "Content-Type": "application/json",
          },
          body: JSON.stringify(body),
        }),
      );
    let org: string | undefined;
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
        'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'member\',now())',
        [org, subject],
      );
      await owner.query(
        "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES($1,$1,$2,'active')",
        [project, customer],
      );
      await owner.query(
        "INSERT INTO webdock_auth.database_binding(id,project_id,name,environment) VALUES($1,$2,'Gateway fixture','production')",
        [binding, project],
      );
      await owner.query(
        "INSERT INTO webdock_auth.database_grant(id,binding_id,subject,profile,runtime_origin,connection_id,secret_sealed) VALUES($1,$2,$3,'schema',$4,$5,$6)",
        [
          grant,
          binding,
          subject,
          origin,
          connection,
          encryptMailSecret(proxySecret, `database:${binding}:${subject}`),
        ],
      );
      await owner.query(
        "INSERT INTO webdock_auth.database_runtime_owner(runtime_origin,binding_id,subject,profile) VALUES($1,$2,$3,'schema')",
        [origin, binding, subject],
      );
      const cluster = id(10),
        backend = id(11),
        plan = id(12),
        offer = id(13),
        order = id(14),
        managed = id(15);
      await owner.query(
        "INSERT INTO webdock_auth.hosting_cluster(id,name,provider,country,region,location_evidence,verified,capacity) VALUES($1::text,$1::text,'local','DE','test','Synthetic viewer fixture',true,'{}')",
        [cluster],
      );
      await owner.query(
        "INSERT INTO webdock_auth.hosting_database_backend(id,cluster_id,kind,node,pool_id,state,demand,max_databases,logical_capacity_bytes,host,direct_port,pooled_port,endpoint_ready,ca_certificate) VALUES($1,$2,'dedicated','local','00000000-0000-0000-0000-000000000001','ready','{}',1,1024,'managed.rls.invalid',30432,30433,true,'synthetic-ca')",
        [backend, cluster],
      );
      await owner.query(
        "INSERT INTO webdock_auth.hosting_database_plan(id,plan) VALUES($1,'{}')",
        [plan],
      );
      await owner.query(
        "INSERT INTO webdock_auth.hosting_database_offer(id,customer_id,project_id,plan_revision_id,name,snapshot) VALUES($1,$2,$3,$4,'Viewer fixture','{}')",
        [offer, customer, project, plan],
      );
      await owner.query(
        "INSERT INTO webdock_auth.hosting_database_order(id,offer_id,customer_id,project_id,plan_revision_id,accepted_by,snapshot) VALUES($1,$2,$3,$4,$5,$6,'{}')",
        [order, offer, customer, project, plan, subject],
      );
      await owner.query(
        "INSERT INTO webdock_auth.hosting_database(id,customer_id,project_id,order_id,plan_revision_id,backend_id,kind,name,database_name,state,credentials_encrypted,confirmed_credentials_encrypted,confirmed_credential_revision) VALUES($1,$2,$3,$4,$5,$6,'dedicated','Viewer fixture',$7,'ready',$8,$8,1)",
        [
          managed,
          customer,
          project,
          order,
          plan,
          backend,
          "db_" + managed,
          encryptMailSecret(
            { migrationPassword: "managed-secret" },
            `database:${managed}:1`,
          ),
        ],
      );
      const managedOpen = () =>
        user(() =>
          transaction((db) =>
            executeDatabase(db, actor, {
              action: "openManaged",
              databaseID: managed,
            }),
          ),
        ) as Promise<{ code: string }>;
      await assert.rejects(managedOpen(), /managed by your operator/);
      await owner.query(
        `UPDATE webdock_auth.member SET role='admin' WHERE "userId"=$1`,
        [subject],
      );
      const directory = (await user(() =>
        transaction((db) =>
          executeDatabase(db, actor, { action: "list", customerID: customer }),
        ),
      )) as { managed: { id: string; canOpen: boolean }[] };
      assert.deepEqual(
        directory.managed.map((d) => [d.id, d.canOpen]),
        [[managed, true]],
      );
      const managedLaunch = await managedOpen();
      await assert.rejects(
        user(() =>
          database.query(
            "UPDATE webdock_auth.managed_database_launch SET consumed_at=NULL WHERE database_id=$1",
            [managed],
          ),
        ),
        /permission denied/,
      );
      await assert.rejects(
        user(() =>
          database.query(
            "INSERT INTO webdock_auth.managed_database_session SELECT * FROM webdock_auth.managed_database_session",
          ),
        ),
        /permission denied/,
      );
      const managedReply = await http({
        action: "exchange",
        code: managedLaunch.code,
      });
      assert.equal(
        managedReply.status,
        200,
        JSON.stringify(await managedReply.clone().json()),
      );
      const managedSession = (await managedReply.json()).data;
      const managedInspect = await http({
        action: "inspect",
        token: managedSession.token,
      });
      assert.equal(
        managedInspect.status,
        200,
        JSON.stringify(await managedInspect.clone().json()),
      );
      const managedAccess = (await managedInspect.json()).data;
      assert.equal(managedAccess.kind, "managed");
      assert.equal(managedAccess.database.username, "wd_migrate_" + managed);
      assert.equal(managedAccess.database.password, "managed-secret");
      assert.equal(
        (await http({ action: "exchange", code: managedLaunch.code })).status,
        404,
      );
      await assert.rejects(
        broker(() =>
          database.query(
            "UPDATE webdock_auth.hosting_database SET name='denied' WHERE id=$1",
            [managed],
          ),
        ),
        /row-level security/,
      );
      await assert.rejects(
        broker(() =>
          database.query(
            "UPDATE webdock_auth.hosting_database_backend SET host='denied.invalid' WHERE id=$1",
            [backend],
          ),
        ),
        /row-level security/,
      );
      await owner.query(
        `UPDATE webdock_auth.member SET role='member' WHERE "userId"=$1`,
        [subject],
      );
      assert.notEqual(
        (await http({ action: "inspect", token: managedSession.token })).status,
        200,
      );
      await owner.query(
        `UPDATE webdock_auth.member SET role='admin' WHERE "userId"=$1`,
        [subject],
      );
      await owner.query(
        "UPDATE webdock_auth.hosting_database SET credential_revision=2 WHERE id=$1",
        [managed],
      );
      assert.equal(
        (await http({ action: "inspect", token: managedSession.token })).status,
        404,
      );
      await owner.query(
        "UPDATE webdock_auth.hosting_database SET credential_revision=1 WHERE id=$1",
        [managed],
      );
      assert.equal(
        (await http({ action: "logout", token: managedSession.token })).status,
        200,
      );
      assert.equal(
        (await http({ action: "inspect", token: managedSession.token })).status,
        404,
      );
      const launch = await open();
      const exchanged = await broker(() =>
        transaction((db) => exchangeLaunch(db, launch.code)),
      );
      const inspected = await broker(() =>
        transaction((db) => inspectSession(db, exchanged.token)),
      );
      assert.equal(inspected.subject, subject);
      assert.equal(inspected.profile, "schema");
      assert.equal(inspected.connectionID, connection);
      assert.ok("runtimeOrigin" in inspected);
      if (!("runtimeOrigin" in inspected))
        throw Error("Expected legacy runtime");
      assert.equal(inspected.runtimeOrigin, origin);
      assert.equal(inspected.proxySecret, proxySecret);
      assert.equal(inspected.scopeID, exchanged.scopeID);
      await assert.rejects(
        broker(() => transaction((db) => exchangeLaunch(db, launch.code))),
        /unavailable/,
        "launch can be consumed once",
      );
      const second = await open();
      assert.equal(
        (await http({ action: "exchange", code: second.code }, "wrong")).status,
        401,
      );
      const reply = await http({ action: "exchange", code: second.code });
      assert.equal(reply.status, 200);
      const { data: gatewaySession } = await reply.json();
      assert.equal(
        (await http({ action: "inspect", token: gatewaySession.token })).status,
        200,
      );
      await owner.query(
        "UPDATE webdock_auth.database_grant SET enabled=false,revision=revision+1 WHERE id=$1",
        [grant],
      );
      assert.equal(
        (await http({ action: "inspect", token: gatewaySession.token })).status,
        404,
        "revoked grant invalidates existing gateway session",
      );
      await owner.query(
        "UPDATE webdock_auth.database_grant SET enabled=true WHERE id=$1",
        [grant],
      );
      const third = await open(),
        thirdReply = await http({ action: "exchange", code: third.code });
      assert.equal(thirdReply.status, 200);
      const thirdSession = (await thirdReply.json()).data;
      await owner.query(
        "UPDATE webdock_auth.session SET \"expiresAt\"=now()-interval '1 minute' WHERE id=$1",
        [session],
      );
      assert.notEqual(
        (await http({ action: "inspect", token: thirdSession.token })).status,
        200,
        "expired authentication session invalidates gateway session",
      );
      assert.equal(
        (await http({ action: "logout", token: thirdSession.token })).status,
        200,
      );
      assert.equal(
        (
          await broker(() =>
            database.query(
              "SELECT id FROM webdock_auth.database_session WHERE id=$1",
              [thirdSession.scopeID],
            ),
          )
        ).rowCount,
        0,
      );
    } finally {
      const cleanup = await owner.connect();
      try {
        await cleanup.query("BEGIN");
        await cleanup.query("SET LOCAL session_replication_role='replica'");
        await cleanup.query(
          "DELETE FROM webdock_auth.database_session WHERE grant_id=$1",
          [grant],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.database_launch WHERE grant_id=$1",
          [grant],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.database_audit WHERE binding_id=$1",
          [binding],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.database_runtime_owner WHERE binding_id=$1",
          [binding],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.database_grant WHERE id=$1",
          [grant],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.database_binding WHERE id=$1",
          [binding],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.managed_database_session WHERE database_id=$1",
          [id(15)],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.managed_database_launch WHERE database_id=$1",
          [id(15)],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_audit WHERE target_id=$1",
          [id(15)],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_database WHERE id=$1",
          [id(15)],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_database_order WHERE id=$1",
          [id(14)],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_database_offer WHERE id=$1",
          [id(13)],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_database_plan WHERE id=$1",
          [id(12)],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_database_backend WHERE id=$1",
          [id(11)],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.hosting_cluster WHERE id=$1",
          [id(10)],
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
        await cleanup.query("COMMIT");
      } catch (error) {
        await cleanup.query("ROLLBACK");
        throw error;
      } finally {
        cleanup.release();
        await Promise.all([
          owner.end(),
          database.end(),
          authDatabase.end(),
          workerDatabase.end(),
        ]);
      }
    }
  },
);
