import { bindingRuntime } from "../../src/modules/hosting/binding-runtime";
import test from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import {
  withMailProductFixture,
  assertMailTestDatabase,
} from "./helpers/mail-product";
import { executeHosting } from "../../src/modules/hosting/service";
import { hostingSchemaSQL } from "../../src/modules/hosting/schema";
import { openEnvironment } from "../../src/modules/hosting/environment";
import { authorizeMailKey } from "../../src/modules/mail/product/api-keys";
import { database } from "../../src/db/runtime";
assertMailTestDatabase(process.env.DATABASE_URL!);
test.after(() => database.end());
const key = () => randomBytes(16).toString("hex");
test("native mail bindings recover the same private key after interruption and revoke only their own access", async () => {
  await withMailProductFixture(async (f) => {
    await f.db.query(hostingSchemaSQL);
    let project = "",
      cluster = "";
    const oldOrigin = process.env.MAIL_API_ORIGIN,
      oldEgress = process.env.WEBDOCK_APPLICATION_MAIL_EGRESS_CIDRS;
    try {
      project = (
        await f.db.query(
          "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES(webdock_auth.next_snowflake(),'Mail application',$1,'active') RETURNING id",
          [f.customers[0]],
        )
      ).rows[0].id;
      cluster = (
        await f.db.query(
          "INSERT INTO webdock_auth.hosting_cluster(name,provider,country,region,location_evidence) VALUES('Mail fixture','local','DE','test','Disposable fixture only') RETURNING id",
        )
      ).rows[0].id;
      await f.db.query(
        "INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,mode,cluster_id,namespace) VALUES($1,$2,'k3s','selfservice',$3,$4)",
        [project, f.customers[0], cluster, "wd-" + project],
      );
      await f.db.query(
        "INSERT INTO webdock_auth.hosting_agent(cluster_id,credential_hash,observation,generation) VALUES($1,$2,$3,1)",
        [
          cluster,
          key(),
          JSON.stringify({
            capabilities: { serviceBindingsVersion: 1, environment: true },
          }),
        ],
      );
      const actor = {
        subject: f.admin.id,
        sessionID: f.admin.principal.sessionID,
        source: "studio" as const,
        scopes: ["hosting:read", "hosting:write"],
      };

      process.env.MAIL_API_ORIGIN = "https://mail.example.invalid";
      process.env.WEBDOCK_APPLICATION_MAIL_EGRESS_CIDRS = "192.0.2.10/32";
      const app = await executeHosting(actor, {
        action: "apps.prepare",
        projectID: project,
        name: "mail-app",
        spec: {
          args: [],
          port: 8080,
          healthPath: "/",
          cpuMillicores: 100,
          memoryBytes: 67108864,
          ephemeralBytes: 67108864,
          replicas: 1,
        },
        subscriptionRevision: 0,
        idempotencyKey: key(),
      });
      const before = (
        await f.db.query(
          "SELECT count(*)::int AS n FROM webdock_mail.instance WHERE customer_id=$1",
          [f.customers[0]],
        )
      ).rows[0].n;
      const command = {
        action: "services.bind",
        projectID: project,
        appID: app.appID,
        service: {
          kind: "mail",
          connectionID: f.actorA.connectionID,
          domainID: f.domainA.id,
          variable: "WEBDOCK_MAIL_API_KEY",
        },
        revision: 0,
        idempotencyKey: key(),
      };
      const bound = await executeHosting(actor, command);
      assert.equal((await executeHosting(actor, command)).id, bound.id);
      const rows = (
        await f.db.query(
          "SELECT permission,domain_id FROM webdock_auth.mail_api_key WHERE customer_id=$1",
          [f.customers[0]],
        )
      ).rows;
      assert.equal(rows.length, 1);
      assert.equal(rows[0].permission, "sending_access");
      assert.equal(rows[0].domain_id, f.domainA.id);
      const stored = (
        await f.db.query(
          "SELECT environment_encrypted FROM webdock_auth.hosting_app WHERE id=$1",
          [app.appID],
        )
      ).rows[0];
      const env = openEnvironment(app.appID, stored.environment_encrypted);
      assert.equal(
        JSON.stringify(bound).includes(env.WEBDOCK_MAIL_API_KEY),
        false,
      );
      assert.equal(env.WEBDOCK_MAIL_API_URL, "https://mail.example.invalid");
      const request = new Request("https://mail.example.invalid/emails", {
        headers: { authorization: "Bearer " + env.WEBDOCK_MAIL_API_KEY },
      });
      assert.equal(
        (await authorizeMailKey(request, "sending_access")).connectionID,
        f.actorA.connectionID,
      );
      await assert.rejects(authorizeMailKey(request, "full_access"));
      assert.equal(
        (
          await f.db.query(
            "SELECT count(*)::int AS n FROM webdock_mail.instance WHERE customer_id=$1",
            [f.customers[0]],
          )
        ).rows[0].n,
        before,
      );
      await f.db.query(
        "UPDATE webdock_auth.mail_api_key SET revoked_at=now() WHERE customer_id=$1",
        [f.customers[0]],
      );
      await assert.rejects(
        bindingRuntime(f.db, app.appID, env),
        /Mail connection needs attention/,
      );
      assert.equal(
        (await bindingRuntime(f.db, app.appID, env, false)).length,
        1,
        "shutdown retains bindings without requiring a healthy Mail key",
      );
      const rotation = {
        action: "services.rotate",
        projectID: project,
        bindingID: bound.id,
        revision: bound.revision,
        idempotencyKey: key(),
      };
      const rotated = await executeHosting(actor, rotation);
      assert.equal(
        (await executeHosting(actor, rotation)).revision,
        rotated.revision,
      );
      await assert.rejects(authorizeMailKey(request, "sending_access"));
      assert.equal(
        (
          await f.db.query(
            "SELECT count(*)::int AS n FROM webdock_auth.mail_api_key WHERE customer_id=$1",
            [f.customers[0]],
          )
        ).rows[0].n,
        2,
      );
      await f.db.query(
        "UPDATE webdock_auth.mail_connection SET state='paused' WHERE id=$1",
        [f.actorA.connectionID],
      );
      assert.equal(
        (
          await executeHosting(actor, {
            action: "services.list",
            projectID: project,
          })
        )[0].state,
        "blocked",
      );
      await f.db.query(
        "UPDATE webdock_auth.mail_connection SET state='ready' WHERE id=$1",
        [f.actorA.connectionID],
      );
      const secondApp = await executeHosting(actor, {
        action: "apps.prepare",
        projectID: project,
        name: "second-mail-app",
        spec: {
          args: [],
          port: 8080,
          healthPath: "/",
          cpuMillicores: 100,
          memoryBytes: 67108864,
          ephemeralBytes: 67108864,
          replicas: 1,
        },
        subscriptionRevision: 0,
        idempotencyKey: key(),
      });
      await executeHosting(actor, {
        ...command,
        appID: secondApp.appID,
        idempotencyKey: key(),
      });
      const secondEnv = openEnvironment(
        secondApp.appID,
        (
          await f.db.query(
            "SELECT environment_encrypted FROM webdock_auth.hosting_app WHERE id=$1",
            [secondApp.appID],
          )
        ).rows[0].environment_encrypted,
      );
      const countBeforeCollision = (
        await f.db.query(
          "SELECT count(*)::int AS n FROM webdock_auth.mail_api_key WHERE customer_id=$1",
          [f.customers[0]],
        )
      ).rows[0].n;
      const conflictingApp = await executeHosting(actor, {
        action: "apps.prepare",
        projectID: project,
        name: "conflicting-mail-app",
        spec: {
          args: [],
          port: 8080,
          healthPath: "/",
          cpuMillicores: 100,
          memoryBytes: 67108864,
          ephemeralBytes: 67108864,
          replicas: 1,
        },
        environment: [
          { name: "WEBDOCK_MAIL_API_URL", value: "manually-configured" },
        ],
        subscriptionRevision: 0,
        idempotencyKey: key(),
      });
      await assert.rejects(
        executeHosting(actor, {
          ...command,
          appID: conflictingApp.appID,
          idempotencyKey: key(),
        }),
        /environment variable/,
      );
      assert.equal(
        (
          await f.db.query(
            "SELECT count(*)::int AS n FROM webdock_auth.mail_api_key WHERE customer_id=$1",
            [f.customers[0]],
          )
        ).rows[0].n,
        countBeforeCollision,
      );
      await executeHosting(actor, {
        action: "services.disconnect",
        projectID: project,
        bindingID: bound.id,
        revision: rotated.revision,
        idempotencyKey: key(),
      });
      assert.equal(
        (
          await authorizeMailKey(
            new Request("https://mail.example.invalid/emails", {
              headers: {
                authorization: "Bearer " + secondEnv.WEBDOCK_MAIL_API_KEY,
              },
            }),
            "sending_access",
          )
        ).connectionID,
        f.actorA.connectionID,
      );

      await assert.rejects(authorizeMailKey(request, "sending_access"));
      assert.deepEqual(
        (await executeHosting(actor, { action: "apps.get", appID: app.appID }))
          .environmentNames,
        [],
      );
    } finally {
      if (oldOrigin === undefined) delete process.env.MAIL_API_ORIGIN;
      else process.env.MAIL_API_ORIGIN = oldOrigin;
      if (oldEgress === undefined)
        delete process.env.WEBDOCK_APPLICATION_MAIL_EGRESS_CIDRS;
      else process.env.WEBDOCK_APPLICATION_MAIL_EGRESS_CIDRS = oldEgress;
      await f.db.query(
        "DELETE FROM webdock_auth.hosting_service_binding WHERE project_id=$1",
        [project],
      );
      await f.db.query(
        "DELETE FROM webdock_auth.hosting_operation WHERE project_id=$1",
        [project],
      );
      await f.db.query(
        "DELETE FROM webdock_auth.hosting_app WHERE project_id=$1",
        [project],
      );
      await f.db.query(
        "DELETE FROM webdock_auth.hosting_project WHERE project_id=$1",
        [project],
      );
      await f.db.query(
        "DELETE FROM webdock_auth.hosting_agent WHERE cluster_id=$1",
        [cluster],
      );
      await f.db.query("DELETE FROM webdock_auth.hosting_cluster WHERE id=$1", [
        cluster,
      ]);
      await f.db.query("DELETE FROM webdock_admin.projects WHERE id=$1", [
        project,
      ]);
    }
  });
});
