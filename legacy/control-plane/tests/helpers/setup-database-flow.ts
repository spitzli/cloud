import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import {
  healthcheckImage,
  type HostingActor,
} from "@webdock/hosting-contracts";
import { database } from "../../src/db/runtime";
import { executeHosting } from "../../src/modules/hosting/service";
import { runSetupBatch } from "../../src/modules/hosting/setup-worker";
import {
  claimOperation,
  completeOperation,
} from "../../src/modules/hosting/operations";

/** Runs inside the existing disposable database-order fixture; no external provider. */
export async function verifySetupDatabaseFlow(
  actor: HostingActor,
  projectID: string,
  databaseID: string,
  planRevisionID: string,
  agent: { clusterID: string; generation: number; credentialHash: string },
) {
  const old = process.env.WEBDOCK_SETUP_ENABLED;
  process.env.WEBDOCK_SETUP_ENABLED = "true";
  const observation = (
    await database.query(
      "SELECT observation FROM webdock_auth.hosting_agent WHERE cluster_id=$1",
      [agent.clusterID],
    )
  ).rows[0].observation;
  const customerID = (
    await database.query(
      "SELECT customer_id FROM webdock_admin.projects WHERE id=$1",
      [projectID],
    )
  ).rows[0].customer_id;
  await database.query(
    "INSERT INTO webdock_auth.tenant_subscription(customer_id,name,description,base,extras,revision,updated_by) VALUES($1,'Setup fixture','Local fixture',$2,'{}',1,$3)",
    [
      customerID,
      JSON.stringify({
        hosting: {
          apps: 10,
          cpuMillicores: 4000,
          memoryBytes: 4294967296,
          ephemeralBytes: 4294967296,
          volumeBytes: 5368709120,
          replicasPerApp: 10,
          concurrentDeployments: 10,
        },
      }),
      actor.subject,
    ],
  );
  const setupIDs: string[] = [];
  let appID: string | undefined;
  try {
    await database.query(
      "UPDATE webdock_auth.hosting_agent SET last_seen=now(),observation=jsonb_set(observation,'{capabilities}',(observation->'capabilities') || $2::jsonb) WHERE cluster_id=$1",
      [
        agent.clusterID,
        JSON.stringify({
          networkIsolation: true,
          restrictedPods: true,
          environment: true,
          serviceBindingsVersion: 1,
        }),
      ],
    );
    const configuration = {
      name: "Setup database flow",
      source: { kind: "image", reference: healthcheckImage },
      spec: {
        args: [],
        port: 8080,
        healthPath: "/ping",
        cpuMillicores: 100,
        memoryBytes: 67108864,
        ephemeralBytes: 67108864,
        replicas: 1,
      },
      database: {
        mode: "new",
        name: "Explicit order",
        planRevisionID,
        variable: "DATABASE_URL",
      },
      mail: null,
      autoPublish: false,
    };
    const save = async (config: unknown) => {
      const view = await executeHosting(actor, {
        action: "setup.save",
        projectID,
        revision: 0,
        configuration: config,
        idempotencyKey: randomUUID(),
      });
      setupIDs.push(view.id);
      return view;
    };
    const orderCount = async () =>
      Number(
        (
          await database.query(
            "SELECT count(*) AS n FROM webdock_auth.hosting_database_order WHERE project_id=$1",
            [projectID],
          )
        ).rows[0].n,
      );
    const before = await orderCount();
    const paid = await save(configuration);
    const quote = await executeHosting(actor, {
      action: "setup.preview",
      projectID,
      setupID: paid.id,
      revision: paid.revision,
    });
    assert.equal(quote.preview.offers.length, 1);
    await assert.rejects(
      executeHosting(actor, {
        action: "setup.confirm",
        projectID,
        setupID: paid.id,
        revision: paid.revision,
        previewID: quote.preview.id,
        acceptedOfferIDs: [],
        idempotencyKey: randomUUID(),
      }),
    );
    assert.equal(
      await orderCount(),
      before,
      "preview and missing consent must never order",
    );
    const collision = await executeHosting(actor, {
      action: "setup.save",
      projectID,
      revision: 0,
      configuration,
      environment: [{ name: "DATABASE_URL", value: "manual-conflict" }],
      idempotencyKey: randomUUID(),
    });
    setupIDs.push(collision.id);
    const collisionPreview = await executeHosting(actor, {
      action: "setup.preview",
      projectID,
      setupID: collision.id,
      revision: collision.revision,
    });
    assert.ok(
      collisionPreview.preview.blockers.includes(
        "Choose distinct service variables and remove conflicting manual variables before deploying.",
      ),
    );
    const draft = await save({
      ...configuration,
      name: "Setup existing database",
      database: { mode: "existing", databaseID, variable: "DATABASE_URL" },
    });
    const preview = await executeHosting(actor, {
      action: "setup.preview",
      projectID,
      setupID: draft.id,
      revision: draft.revision,
    });
    assert.deepEqual(preview.preview.blockers, []);
    assert.deepEqual(preview.preview.offers, []);
    const confirmation = {
      action: "setup.confirm",
      projectID,
      setupID: draft.id,
      revision: draft.revision,
      previewID: preview.preview.id,
      acceptedOfferIDs: [],
      idempotencyKey: randomUUID(),
    };
    await executeHosting(actor, confirmation);
    await executeHosting(actor, confirmation);
    await runSetupBatch({ limit: 15 });
    const read = () =>
      executeHosting(actor, {
        action: "setup.get",
        projectID,
        setupID: draft.id,
      });
    const running = await read();
    appID = running.resources.appID;
    assert.equal(running.state, "running", running.reason);
    assert.equal(running.resources.databaseID, databaseID);
    assert.equal(running.resources.bindingIDs.length, 1);
    const packet = await claimOperation(agent);
    assert.ok(packet);
    assert.equal(packet.desired.appID, appID);
    assert.equal(packet.desired.serviceBindings.length, 1);
    const connection = new URL(packet.desired.environment.DATABASE_URL);
    assert.equal(connection.searchParams.get("sslmode"), "verify-full");
    assert.equal(JSON.stringify(running).includes(connection.password), false);
    await completeOperation(agent, {
      id: packet.id,
      generation: packet.generation,
      outcome: "succeeded",
      proof: {
        appID: appID!,
        revision: packet.target_revision,
        namespace: "wd-" + projectID,
        uid: "setup-fixture",
        ready: true,
        deleted: false,
        replicas: 1,
        serviceBindings: packet.desired.serviceBindings.map(
          (b: { id: string; revision: number }) => ({
            id: b.id,
            revision: b.revision,
          }),
        ),
      },
    });
    await database.query(
      "UPDATE webdock_auth.hosting_setup SET next_attempt_at=now() WHERE id=$1",
      [draft.id],
    );
    await runSetupBatch({ limit: 15 });
    const ready = await read();
    assert.equal(ready.state, "ready", ready.reason);
    assert.equal(ready.resources.appID, appID);
    assert.equal(
      await orderCount(),
      before,
      "existing database must not create another order",
    );
    await database.query(
      "UPDATE webdock_auth.hosting_database SET state='paused' WHERE id=$1",
      [databaseID],
    );
    try {
      const app = await executeHosting(actor, { action: "apps.get", appID });
      const stopped = await executeHosting(actor, {
        action: "apps.stop",
        appID,
        revision: app.revision,
        subscriptionRevision: app.subscriptionRevision,
        idempotencyKey: randomUUID(),
      });
      assert.equal(stopped.status, "queued");
    } finally {
      await database.query(
        "UPDATE webdock_auth.hosting_database SET state='ready' WHERE id=$1",
        [databaseID],
      );
    }
    const bindings = await executeHosting(actor, {
      action: "services.list",
      projectID,
      appID,
    });
    assert.ok(["connected", "pending"].includes(bindings[0].state));
  } finally {
    await database.query(
      "DELETE FROM webdock_auth.hosting_setup WHERE id=ANY($1)",
      [setupIDs],
    );
    if (appID) {
      await database.query(
        "DELETE FROM webdock_auth.hosting_service_binding WHERE app_id=$1",
        [appID],
      );
      await database.query(
        "DELETE FROM webdock_auth.hosting_reservation WHERE app_id=$1",
        [appID],
      );
      await database.query(
        "DELETE FROM webdock_auth.hosting_operation WHERE app_id=$1",
        [appID],
      );
      await database.query("DELETE FROM webdock_auth.hosting_app WHERE id=$1", [
        appID,
      ]);
    }
    await database.query(
      "UPDATE webdock_auth.hosting_agent SET observation=$2 WHERE cluster_id=$1",
      [agent.clusterID, observation],
    );
    await database.query(
      "DELETE FROM webdock_auth.tenant_subscription WHERE customer_id=$1",
      [customerID],
    );
    if (old === undefined) delete process.env.WEBDOCK_SETUP_ENABLED;
    else process.env.WEBDOCK_SETUP_ENABLED = old;
  }
}
