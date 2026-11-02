import test from "node:test";
import assert from "node:assert/strict";
import { randomUUID, generateKeyPairSync } from "node:crypto";
import { Pool } from "pg";
import { database } from "../../src/db/runtime";
import { createSetupFixture } from "../helpers/setup-fixture";
import { executeHosting } from "../../src/modules/hosting/service";
import { executeSetupStep } from "../../src/modules/hosting/setup-execution";
import { healthcheckImage, setupSteps } from "@webdock/hosting-contracts";
const url = new URL(process.env.DATABASE_URL!);
if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_api_test")
  throw Error("Disposable fixture required");
test.after(() => database.end());
test("Git setup recovers terminal release with a fresh build of the pinned commit and then observes readiness", async () => {
  const db = new Pool({ connectionString: url.href }),
    f = await createSetupFixture(db);
  const originalFetch = globalThis.fetch;
  const previous = {
    id: process.env.WEBDOCK_GITHUB_APP_ID,
    key: process.env.WEBDOCK_GITHUB_APP_PRIVATE_KEY,
  };
  let connectionID: string | undefined;
  try {
    const insert = async (sql: string, values: unknown[] = []) =>
      (await db.query(sql + " RETURNING id", values)).rows[0].id as string;
    connectionID = await insert(
      "INSERT INTO webdock_auth.git_connection(customer_id,installation_id,account_id,account_login,permissions) VALUES($1,$2,'7','fixture','{}')",
      [f.customerID, f.customerID],
    );
    await db.query(
      "INSERT INTO webdock_auth.git_repository(connection_id,customer_id,repository_id,metadata) VALUES($1,$2,'42','{}')",
      [connectionID, f.customerID],
    );
    const spec = {
      args: [],
      port: 8080,
      healthPath: "/ping",
      cpuMillicores: 100,
      memoryBytes: 67108864,
      ephemeralBytes: 67108864,
      replicas: 1,
    };
    const app = await executeHosting(f.actor, {
      action: "apps.prepare",
      projectID: f.projectID,
      name: "Git retry",
      spec,
      subscriptionRevision: 1,
      idempotencyKey: randomUUID(),
    });
    const sourceID = await insert(
      "INSERT INTO webdock_auth.git_source(customer_id,project_id,connection_id,repository_id,branch,root_directory,recipe,target_id) VALUES($1,$2,$3,'42','main','apps/web','dockerfile',$4)",
      [f.customerID, f.projectID, connectionID, app.appID],
    );
    const sha = "a".repeat(40);
    const buildID = await insert(
      "INSERT INTO webdock_auth.git_build(customer_id,project_id,source_id,source_sha,source_revision,connection_generation,environment_revision,status,subject,idempotency_key) VALUES($1,$2,$3,$4,1,1,1,'succeeded',$5,$6)",
      [f.customerID, f.projectID, sourceID, sha, f.actor.subject, randomUUID()],
    );
    const artifactID = await insert(
      "INSERT INTO webdock_auth.git_artifact(build_id,customer_id,project_id,kind,digest,storage_key,size_bytes) VALUES($1,$2,$3,'oci',$4,$5,1)",
      [
        buildID,
        f.customerID,
        f.projectID,
        "sha256:" + "b".repeat(64),
        randomUUID(),
      ],
    );
    const releaseID = await insert(
      "INSERT INTO webdock_auth.git_release(customer_id,project_id,artifact_id,source_id,status,source_revision,connection_generation,environment_revision,subject) VALUES($1,$2,$3,$4,'failed',1,1,1,$5)",
      [f.customerID, f.projectID, artifactID, sourceID, f.actor.subject],
    );
    const configuration = {
      name: "Git retry",
      source: {
        kind: "git",
        connectionID,
        repositoryID: "42",
        branch: "main",
        rootDirectory: "apps/web",
        dockerfile: "Dockerfile",
        buildProvider: "isolated",
        workflowPath: ".github/workflows/webdock.yml",
        artifactPrefix: "webdock",
      },
      spec,
      database: { mode: "none" },
      mail: null,
      autoPublish: false,
    };
    const saved = await executeHosting(f.actor, {
      action: "setup.save",
      projectID: f.projectID,
      revision: 0,
      configuration,
      idempotencyKey: randomUUID(),
    });
    const preview = {
      id: randomUUID(),
      revision: 1,
      sourceSHA: sha,
      image: null,
      expiresAt: new Date(Date.now() + 600000).toISOString(),
      offers: [],
      blockers: [],
      subscriptionRevision: 1,
    };
    await db.query(
      "UPDATE webdock_auth.hosting_setup SET state='blocked',actor=$2,resources=$3,steps=$4,preview=$5,confirmed_preview_id=$6 WHERE id=$1",
      [
        saved.id,
        JSON.stringify(f.actor),
        JSON.stringify({ appID: app.appID, sourceID, buildID }),
        JSON.stringify(
          setupSteps.map((name) => ({
            name,
            state:
              name === "release"
                ? "blocked"
                : name === "readiness"
                  ? "pending"
                  : "succeeded",
            reason: null,
            operationID: null,
          })),
        ),
        JSON.stringify(preview),
        preview.id,
      ],
    );
    const resumed = await executeHosting(f.actor, {
      action: "setup.resume",
      projectID: f.projectID,
      setupID: saved.id,
      revision: 1,
      idempotencyKey: randomUUID(),
    });
    assert.equal(resumed.resources.buildID, undefined);
    assert.equal(resumed.preview.sourceSHA, sha);
    process.env.WEBDOCK_GITHUB_APP_ID = "123";
    process.env.WEBDOCK_GITHUB_APP_PRIVATE_KEY = generateKeyPairSync("rsa", {
      modulusLength: 2048,
    })
      .privateKey.export({ type: "pkcs8", format: "pem" })
      .toString();
    const repo = {
      id: 42,
      owner: { id: 7, login: "owner" },
      name: "repo",
      full_name: "owner/repo",
      default_branch: "main",
      permissions: { pull: true },
    };
    globalThis.fetch = (async (input: RequestInfo | URL) => {
      const path = new URL(String(input)).pathname;
      const responses: Record<string, unknown> = {
        ["/app/installations/" + f.customerID]: {
          id: f.customerID,
          app_id: 123,
          account: { id: 7, login: "owner" },
          suspended_at: null,
          permissions: { contents: "read", checks: "write", metadata: "read" },
        },
        ["/app/installations/" + f.customerID + "/access_tokens"]: {
          token: "fixture-token",
          expires_at: new Date(Date.now() + 3600000).toISOString(),
          permissions: { contents: "read", metadata: "read" },
          repositories: [repo],
        },
        "/repositories/42": repo,
        ["/repos/owner/repo/commits/" + sha]: { sha },
        "/repos/owner/repo/branches/main": {
          name: "main",
          commit: { sha: "c".repeat(40) },
        },
      };
      return Response.json(responses[path] ?? {}, {
        status: path in responses ? 200 : 404,
      });
    }) as typeof fetch;
    const row = (
      await db.query("SELECT * FROM webdock_auth.hosting_setup WHERE id=$1", [
        saved.id,
      ])
    ).rows[0];
    const building = await executeSetupStep(row);
    assert.ok(building.resources.buildID);
    assert.notEqual(building.resources.buildID, buildID);
    const fresh = (
      await db.query("SELECT * FROM webdock_auth.git_build WHERE id=$1", [
        building.resources.buildID,
      ])
    ).rows[0];
    assert.equal(fresh.source_sha, sha);
    assert.equal(fresh.source_id, sourceID);
    assert.equal(fresh.idempotency_key, "setup-" + saved.id + "-build-1");
    // Fixture publisher completes the new artifact; setup must follow the new build, never the failed release.
    await db.query(
      "UPDATE webdock_auth.git_build SET status='succeeded' WHERE id=$1",
      [fresh.id],
    );
    const freshArtifact = await insert(
      "INSERT INTO webdock_auth.git_artifact(build_id,customer_id,project_id,kind,digest,storage_key,size_bytes) VALUES($1,$2,$3,'oci',$4,$5,1)",
      [
        fresh.id,
        f.customerID,
        f.projectID,
        "sha256:" + "d".repeat(64),
        randomUUID(),
      ],
    );
    const freshRelease = await insert(
      "INSERT INTO webdock_auth.git_release(customer_id,project_id,artifact_id,source_id,status,source_revision,connection_generation,environment_revision,subject) VALUES($1,$2,$3,$4,'ready',1,1,1,$5)",
      [f.customerID, f.projectID, freshArtifact, sourceID, f.actor.subject],
    );
    await db.query(
      "UPDATE webdock_auth.hosting_app SET lifecycle='deployed',status='ready',observed_revision=revision,spec=$2 WHERE id=$1",
      [
        app.appID,
        JSON.stringify({
          ...spec,
          template: "custom",
          image: healthcheckImage,
        }),
      ],
    );
    let progress = building;
    for (let i = 0; i < 4 && !progress.ready; i++)
      progress = await executeSetupStep({
        ...row,
        resources: progress.resources,
        steps: progress.steps,
      });
    assert.equal(progress.ready, true);
    assert.equal(progress.resources.releaseID, freshRelease);
    assert.equal(progress.resources.appID, app.appID);
    assert.equal(
      (
        await db.query(
          "SELECT status FROM webdock_auth.git_release WHERE id=$1",
          [releaseID],
        )
      ).rows[0].status,
      "failed",
    );
  } finally {
    globalThis.fetch = originalFetch;
    for (const [name, value] of [
      ["WEBDOCK_GITHUB_APP_ID", previous.id],
      ["WEBDOCK_GITHUB_APP_PRIVATE_KEY", previous.key],
    ]) {
      if (value === undefined) delete process.env[name!];
      else process.env[name!] = value;
    }
    for (const table of [
      "git_check_outbox",
      "git_release",
      "git_artifact",
      "git_build",
      "git_source",
    ])
      await db.query(
        "DELETE FROM webdock_auth." +
          table +
          " WHERE " +
          (table === "git_check_outbox"
            ? "build_id IN (SELECT id FROM webdock_auth.git_build WHERE project_id=$1)"
            : "project_id=$1"),
        [f.projectID],
      );
    if (connectionID) {
      await db.query(
        "DELETE FROM webdock_auth.git_repository WHERE connection_id=$1",
        [connectionID],
      );
      await db.query("DELETE FROM webdock_auth.git_connection WHERE id=$1", [
        connectionID,
      ]);
    }
    await f.cleanup();
    await db.end();
  }
});
