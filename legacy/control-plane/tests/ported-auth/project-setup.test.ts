import test from "node:test";
import assert from "node:assert/strict";
import { Pool } from "pg";
import { randomUUID } from "node:crypto";
import { createSetupFixture } from "../helpers/setup-fixture";
import { executeHosting } from "../../src/modules/hosting/service";
import { hostingSchemaSQL } from "../../src/modules/hosting/schema";
import { runSetupBatch } from "../../src/modules/hosting/setup-worker";
import {
  claimOperation,
  completeOperation,
} from "../../src/modules/hosting/operations";
import { database } from "../../src/db/runtime";
const url = new URL(process.env.DATABASE_URL!);
if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_api_test")
  throw Error("Disposable fixture required");
test.after(() => database.end());
test("confirmed setup survives worker restarts without duplicate applications and waits for observed readiness", async () => {
  const db = new Pool({ connectionString: url.href });
  await db.query(hostingSchemaSQL);
  const f = await createSetupFixture(db);
  const enabled = process.env.WEBDOCK_SETUP_ENABLED;
  process.env.WEBDOCK_SETUP_ENABLED = "true";
  try {
    const command = {
      action: "setup.save",
      projectID: f.projectID,
      revision: 0,
      configuration: {
        name: "portal",
        source: { kind: "template", template: "healthcheck" },
        spec: {
          args: [],
          port: 8080,
          healthPath: "/ping",
          cpuMillicores: 100,
          memoryBytes: 67108864,
          ephemeralBytes: 67108864,
          replicas: 1,
        },
        database: { mode: "none" },
        mail: null,
        autoPublish: false,
      },
      environment: [{ name: "PRIVATE_TOKEN", value: "never-public" }],
      idempotencyKey: randomUUID(),
    };
    const saved = await executeHosting(f.actor, command);
    assert.equal((await executeHosting(f.actor, command)).id, saved.id);
    assert.equal(JSON.stringify(saved).includes("never-public"), false);
    const review = await executeHosting(f.actor, {
      action: "setup.preview",
      projectID: f.projectID,
      setupID: saved.id,
      revision: saved.revision,
    });
    assert.deepEqual(review.preview.blockers, []);
    await assert.rejects(
      executeHosting(f.actor, {
        action: "setup.confirm",
        projectID: f.projectID,
        setupID: saved.id,
        revision: saved.revision,
        previewID: randomUUID(),
        acceptedOfferIDs: [],
        idempotencyKey: randomUUID(),
      }),
      /changed/,
    );
    const confirm = {
      action: "setup.confirm",
      projectID: f.projectID,
      setupID: saved.id,
      revision: saved.revision,
      previewID: review.preview.id,
      acceptedOfferIDs: [],
      idempotencyKey: randomUUID(),
    };
    await executeHosting(f.actor, confirm);
    await executeHosting(f.actor, confirm);
    await runSetupBatch({ limit: 1 });
    const first = await executeHosting(f.actor, {
      action: "setup.get",
      projectID: f.projectID,
      setupID: saved.id,
    });
    assert.ok(first.resources.appID);
    await runSetupBatch({ limit: 10 });
    const waiting = await executeHosting(f.actor, {
      action: "setup.get",
      projectID: f.projectID,
      setupID: saved.id,
    });
    assert.equal(waiting.state, "running", waiting.reason);
    assert.equal(
      (
        await db.query(
          "SELECT count(*)::int AS n FROM webdock_auth.hosting_app WHERE project_id=$1",
          [f.projectID],
        )
      ).rows[0].n,
      1,
    );
    const op = await claimOperation(f.agent);
    assert.ok(op);
    await completeOperation(f.agent, {
      id: op.id,
      generation: op.generation,
      outcome: "failed",
      error: "rollout_timeout",
    });
    await db.query(
      "UPDATE webdock_auth.hosting_setup SET next_attempt_at=now() WHERE id=$1",
      [saved.id],
    );
    await runSetupBatch({ limit: 10 });
    const blocked = await executeHosting(f.actor, {
      action: "setup.get",
      projectID: f.projectID,
      setupID: saved.id,
    });
    assert.equal(blocked.state, "blocked");
    await assert.rejects(
      executeHosting(f.actor, {
        action: "setup.resume",
        projectID: f.projectID,
        setupID: saved.id,
        revision: saved.revision,
        idempotencyKey: randomUUID(),
      }),
      /operator/,
    );
    await db.query(
      'UPDATE webdock_auth."user" SET role=\'operator\',"twoFactorEnabled"=true WHERE id=$1',
      [f.actor.subject],
    );
    await executeHosting(f.actor, {
      action: "setup.resume",
      projectID: f.projectID,
      setupID: saved.id,
      revision: saved.revision,
      idempotencyKey: randomUUID(),
    });
    await runSetupBatch({ limit: 10 });
    const retry = await claimOperation(f.agent);
    assert.ok(retry);
    assert.equal(retry.id, op.id);
    await completeOperation(f.agent, {
      id: retry.id,
      generation: retry.generation,
      outcome: "succeeded",
      proof: {
        appID: waiting.resources.appID,
        revision: retry.target_revision,
        namespace: "wd-" + f.projectID,
        uid: "fixture-app",
        ready: true,
        deleted: false,
        replicas: 1,
      },
    });
    await db.query(
      "UPDATE webdock_auth.hosting_setup SET next_attempt_at=now() WHERE id=$1",
      [saved.id],
    );
    await runSetupBatch({ limit: 10 });
    const done = await executeHosting(f.actor, {
      action: "setup.get",
      projectID: f.projectID,
      setupID: saved.id,
    });
    assert.equal(done.state, "ready", done.reason);
    assert.equal(done.resources.appID, first.resources.appID);
    assert.equal(JSON.stringify(done).includes("never-public"), false);
  } finally {
    if (enabled === undefined) delete process.env.WEBDOCK_SETUP_ENABLED;
    else process.env.WEBDOCK_SETUP_ENABLED = enabled;
    await f.cleanup();
    await db.end();
  }
});

test("setup preflight rejects every managed-name collision and blocked drafts can repair only manual variables", async () => {
  const db = new Pool({ connectionString: url.href });
  await db.query(hostingSchemaSQL);
  const f = await createSetupFixture(db);
  try {
    const configuration = {
      name: "Repair draft",
      source: { kind: "template", template: "healthcheck" },
      spec: {
        args: [],
        port: 8080,
        healthPath: "/ping",
        cpuMillicores: 100,
        memoryBytes: 67108864,
        ephemeralBytes: 67108864,
        replicas: 1,
      },
      database: { mode: "none" },
      mail: null,
      autoPublish: false,
    };
    const message =
      "Choose distinct service variables and remove conflicting manual variables before deploying.";
    const cases = [
      {
        configuration: {
          ...configuration,
          mail: { connectionID: "1", domainID: "2", variable: "MAIL_KEY" },
        },
        environment: [{ name: "WEBDOCK_MAIL_API_URL", value: "manual" }],
      },
      {
        configuration: {
          ...configuration,
          database: { mode: "existing", databaseID: "1", variable: "SAME" },
          mail: { connectionID: "1", domainID: "2", variable: "SAME" },
        },
        environment: [],
      },
      {
        configuration: {
          ...configuration,
          mail: {
            connectionID: "1",
            domainID: "2",
            variable: "WEBDOCK_MAIL_API_URL",
          },
        },
        environment: [],
      },
    ];
    for (const input of cases) {
      const saved = await executeHosting(f.actor, {
        action: "setup.save",
        projectID: f.projectID,
        revision: 0,
        ...input,
        idempotencyKey: randomUUID(),
      });
      const preview = await executeHosting(f.actor, {
        action: "setup.preview",
        projectID: f.projectID,
        setupID: saved.id,
        revision: saved.revision,
      });
      assert.ok(preview.preview.blockers.includes(message));
    }
    const saved = await executeHosting(f.actor, {
      action: "setup.save",
      projectID: f.projectID,
      revision: 0,
      configuration,
      environment: [{ name: "CONFLICT", value: "private" }],
      idempotencyKey: randomUUID(),
    });
    const app = await executeHosting(f.actor, {
      action: "apps.prepare",
      projectID: f.projectID,
      name: "Repair draft",
      spec: configuration.spec,
      environment: [{ name: "CONFLICT", value: "private" }],
      subscriptionRevision: 1,
      idempotencyKey: randomUUID(),
    });
    await db.query(
      "UPDATE webdock_auth.hosting_setup SET state='blocked',resources=$2,steps=$3 WHERE id=$1",
      [
        saved.id,
        JSON.stringify({ appID: app.appID }),
        JSON.stringify([
          {
            name: "bindings",
            state: "blocked",
            reason: "conflict",
            operationID: null,
          },
        ]),
      ],
    );
    const command = {
      action: "setup.save",
      projectID: f.projectID,
      setupID: saved.id,
      revision: saved.revision,
      configuration,
      environment: [{ name: "CONFLICT", value: null }],
      idempotencyKey: randomUUID(),
    };
    await assert.rejects(
      executeHosting(f.actor, {
        ...command,
        configuration: { ...configuration, name: "Changed" },
        idempotencyKey: randomUUID(),
      }),
      /application settings/,
    );
    const fixed = await executeHosting(f.actor, command);
    assert.deepEqual(fixed.environmentNames, []);
    assert.equal(fixed.resources.appID, app.appID);
    assert.equal(fixed.state, "blocked");
    const actual = await executeHosting(f.actor, {
      action: "apps.get",
      appID: app.appID,
    });
    assert.deepEqual(actual.environmentNames, []);
    await db.query(
      "INSERT INTO webdock_auth.hosting_service_binding(customer_id,project_id,app_id,service,variable) VALUES($1,$2,$3,'{\"kind\":\"mail\"}','MAIL_KEY')",
      [f.customerID, f.projectID, app.appID],
    );
    await assert.rejects(
      executeHosting(f.actor, {
        ...command,
        revision: fixed.revision,
        environment: [{ name: "WEBDOCK_MAIL_API_URL", value: null }],
        idempotencyKey: randomUUID(),
      }),
      /Managed service variables/,
    );
  } finally {
    await f.cleanup();
    await db.end();
  }
});
