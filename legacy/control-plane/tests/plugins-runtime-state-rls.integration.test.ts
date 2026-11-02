import { test, expect } from "bun:test";
import { Pool } from "pg";
import { randomBytes, randomUUID } from "node:crypto";
import { readFile } from "node:fs/promises";
import { ScopedPool, withDatabaseScope } from "@webdock/database-runtime";
const enabled = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(enabled ? test : test.skip)(
  "plugin invocation RLS binds documents to live grant, actor, project and runtime",
  async () => {
    const url = new URL(process.env.API_TEST_OWNER_URL!);
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Isolated fixture required");
    const fixture = JSON.parse(await readFile(enabled!, "utf8"));
    const owner = new Pool({ connectionString: url.href, max: 1 });
    const api = new ScopedPool(
      { connectionString: fixture.roles.webdock_auth_runtime, max: 2 },
      { secret: fixture.contextSecret, audience: "webdock_auth_runtime" },
    );
    const worker = new ScopedPool(
      { connectionString: fixture.roles.webdock_worker_runtime, max: 1 },
      { secret: fixture.contextSecret, audience: "webdock_worker_runtime" },
    );
    const base = BigInt(
      "94" + BigInt("0x" + randomBytes(5).toString("hex")).toString(),
    );
    const id = (n: number) => (base + BigInt(n)).toString();
    const plugin = "notes-" + randomBytes(4).toString("hex"),
      invocation = randomUUID(),
      jobInvocation = randomUUID();
    const scope = <T>(n: number, run: () => T) =>
      withDatabaseScope(
        {
          kind: "session",
          subject: id(n + 1),
          sessionID: id(n + 2),
          pluginInvocationID: invocation,
        },
        run,
      );
    try {
      await owner.query(
        "INSERT INTO webdock_auth.plugin_installation(id,kind,state,desired_state,version,runtime,current_generation) VALUES($1,'external','enabled','enabled','1.0.0','ready',1)",
        [plugin],
      );
      await owner.query(
        'INSERT INTO webdock_auth.plugin_configuration(plugin_id,approved_capabilities,configuration,secret_refs) VALUES($1,\'["plugin.data.read","plugin.data.write"]\',\'{}\',\'{"mail":"secret-reference"}\')',
        [plugin],
      );
      await owner.query(
        "INSERT INTO webdock_auth.plugin_release(plugin_id,version,manifest_bytes,manifest_sha256,image,registry_url,signer_key_id) VALUES($1,'1.0.0','{}',$2,$3,'https://fixture.invalid','fixture')",
        [
          plugin,
          "a".repeat(64),
          "registry.fixture/notes@sha256:" + "a".repeat(64),
        ],
      );
      await owner.query(
        "INSERT INTO webdock_auth.plugin_runtime(plugin_id,generation,version,image,status,configuration_revision) VALUES($1,1,'1.0.0',$2,'ready',0)",
        [plugin, "registry.fixture/notes@sha256:" + "a".repeat(64)],
      );
      for (const n of [0, 10]) {
        await owner.query(
          'INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword") VALUES($1,\'Plugin fixture\',$2,\'user\',true,false)',
          [id(n + 1), id(n + 1) + "@example.invalid"],
        );
        await owner.query(
          'INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval \'1 hour\',now())',
          [id(n + 2), id(n + 1)],
        );
        await owner.query(
          "INSERT INTO webdock_admin.customers(id,name,status) VALUES($1,'Plugin fixture','active')",
          [id(n)],
        );
        const org = (
          await owner.query(
            "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
            [id(n)],
          )
        ).rows[0].organization_id;
        await owner.query(
          'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'member\',now())',
          [org, id(n + 1)],
        );
        await owner.query(
          "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES($1,'Plugin fixture',$2,'active')",
          [id(n + 3), id(n)],
        );
        await owner.query(
          'INSERT INTO webdock_auth.plugin_grant(plugin_id,customer_id,project_id,capabilities,jobs) VALUES($1,$2,$3,\'["plugin.data.read","plugin.data.write"]\',\'["maintenance"]\')',
          [plugin, id(n), id(n + 3)],
        );
        await owner.query(
          "INSERT INTO webdock_auth.plugin_document(plugin_id,customer_id,project_id,key,value) VALUES($1,$2,$3,'same','{\"note\":\"private\"}')",
          [plugin, id(n), id(n + 3)],
        );
      }
      await owner.query(
        "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES($1,'Second project',$2,'active')",
        [id(6), id(0)],
      );
      await owner.query(
        "INSERT INTO webdock_auth.plugin_grant(plugin_id,customer_id,project_id,capabilities) VALUES($1,$2,$3,'[\"plugin.data.read\"]')",
        [plugin, id(0), id(6)],
      );
      await owner.query(
        "INSERT INTO webdock_auth.plugin_document(plugin_id,customer_id,project_id,key,value) VALUES($1,$2,$3,'same','{\"note\":\"other project\"}')",
        [plugin, id(0), id(6)],
      );
      await owner.query(
        "INSERT INTO webdock_auth.plugin_invocation(id,plugin_id,customer_id,project_id,runtime_generation,grant_revision,kind,name,intent,actor_id,session_id,operation_id,capabilities,expires_at) VALUES($1,$2,$3,$4,1,0,'action','notes.write','write',$5,$6,$7,'[\"plugin.data.read\",\"plugin.data.write\"]',now()+interval '50 seconds')",
        [invocation, plugin, id(0), id(3), id(1), id(2), id(4)],
      );
      // The apps service uses the same live invocation policy as document access.
      await owner.query(
        'UPDATE webdock_auth.plugin_configuration SET approved_capabilities=approved_capabilities || \'["plugin.apps.read","plugin.apps.manage"]\'::jsonb WHERE plugin_id=$1',
        [plugin],
      );
      await owner.query(
        'UPDATE webdock_auth.plugin_grant SET capabilities=capabilities || \'["plugin.apps.read","plugin.apps.manage"]\'::jsonb WHERE plugin_id=$1 AND project_id=$2',
        [plugin, id(3)],
      );
      await owner.query(
        "UPDATE webdock_auth.plugin_invocation SET intent='read',capabilities=capabilities || '[\"plugin.apps.read\",\"plugin.apps.manage\"]'::jsonb WHERE id=$1",
        [invocation],
      );
      const appScope = (actor: number, capability: string) =>
        scope(
          actor,
          async () =>
            (
              await api.query(
                "SELECT webdock_rls.plugin_context($1) AS scope",
                [capability],
              )
            ).rows[0].scope,
        );
      expect(await appScope(0, "plugin.apps.read")).toMatchObject({
        pluginID: plugin,
        projectID: id(3),
        operationID: id(4),
      });
      expect(await appScope(0, "plugin.apps.manage")).toBeNull();
      expect(await appScope(10, "plugin.apps.read")).toBeNull();
      expect(
        (
          await scope(0, () =>
            api.query(
              "SELECT webdock_rls.plugin_allows($1,$2,$3,'plugin.apps.read') AS allowed",
              [plugin, id(0), id(6)],
            ),
          )
        ).rows[0].allowed,
      ).toBe(false);
      await owner.query(
        "UPDATE webdock_auth.plugin_invocation SET intent='write' WHERE id=$1",
        [invocation],
      );
      expect(await appScope(0, "plugin.apps.manage")).toMatchObject({
        projectID: id(3),
      });
      await owner.query(
        "UPDATE webdock_auth.plugin_grant SET capabilities=capabilities - 'plugin.apps.manage' WHERE plugin_id=$1 AND project_id=$2",
        [plugin, id(3)],
      );
      expect(await appScope(0, "plugin.apps.manage")).toBeNull();
      await owner.query(
        "UPDATE webdock_auth.plugin_configuration SET approved_capabilities=approved_capabilities - 'plugin.apps.read' WHERE plugin_id=$1",
        [plugin],
      );
      expect(await appScope(0, "plugin.apps.read")).toBeNull();
      expect(
        (
          await scope(0, () =>
            api.query(
              "SELECT value FROM webdock_auth.plugin_document WHERE plugin_id=$1",
              [plugin],
            ),
          )
        ).rows,
      ).toEqual([{ value: { note: "private" } }]);
      expect(
        (
          await scope(10, () =>
            api.query(
              "SELECT value FROM webdock_auth.plugin_document WHERE plugin_id=$1",
              [plugin],
            ),
          )
        ).rows,
      ).toEqual([]);
      expect(
        (
          await scope(0, () =>
            api.query(
              "UPDATE webdock_auth.plugin_document SET value='\"changed\"',revision=revision+1 WHERE plugin_id=$1",
              [plugin],
            ),
          )
        ).rowCount,
      ).toBe(1);
      await expect(
        scope(0, () =>
          api.query(
            "UPDATE webdock_auth.plugin_document SET customer_id=$2,project_id=$3 WHERE plugin_id=$1",
            [plugin, id(10), id(13)],
          ),
        ),
      ).rejects.toThrow();
      expect(
        (
          await scope(0, () =>
            api.query(
              "SELECT secret_refs FROM webdock_auth.plugin_configuration",
            ),
          )
        ).rows,
      ).toEqual([]);
      await expect(
        scope(0, () =>
          api.query(
            "UPDATE webdock_auth.plugin_installation SET state='disabled' WHERE id=$1",
            [plugin],
          ),
        ),
      ).rejects.toThrow();
      await scope(0, () =>
        api.query(
          "INSERT INTO webdock_auth.plugin_document_receipt(plugin_id,customer_id,project_id,operation_id,idempotency_key,key,payload_sha256,result) VALUES($1,$2,$3,$4,'write','same',$5,'{}')",
          [plugin, id(0), id(3), id(4), "a".repeat(64)],
        ),
      );
      await expect(
        scope(0, () =>
          api.query(
            "UPDATE webdock_auth.plugin_document_receipt SET result='null' WHERE plugin_id=$1",
            [plugin],
          ),
        ),
      ).rejects.toThrow();
      await expect(
        scope(0, () =>
          api.query(
            "INSERT INTO webdock_auth.plugin_document_receipt(plugin_id,customer_id,project_id,operation_id,idempotency_key,key,payload_sha256,result) VALUES($1,$2,$3,$4,'foreign-operation','same',$5,'{}')",
            [plugin, id(0), id(3), id(9), "a".repeat(64)],
          ),
        ),
      ).rejects.toThrow();
      await owner.query(
        "UPDATE webdock_auth.plugin_installation SET current_generation=2 WHERE id=$1",
        [plugin],
      );
      expect(
        (
          await scope(0, () =>
            api.query(
              "SELECT * FROM webdock_auth.plugin_document WHERE plugin_id=$1",
              [plugin],
            ),
          )
        ).rows,
      ).toEqual([]);
      await owner.query(
        "UPDATE webdock_auth.plugin_installation SET current_generation=1 WHERE id=$1",
        [plugin],
      );
      await owner.query(
        "UPDATE webdock_auth.plugin_invocation SET created_at=now()+interval '1 day',expires_at=now()+interval '1 day 50 seconds' WHERE id=$1",
        [invocation],
      );
      expect(
        (
          await scope(0, () =>
            api.query(
              "SELECT * FROM webdock_auth.plugin_document WHERE plugin_id=$1",
              [plugin],
            ),
          )
        ).rows,
      ).toEqual([]);
      await owner.query(
        "UPDATE webdock_auth.plugin_invocation SET created_at=now(),expires_at=now()+interval '50 seconds' WHERE id=$1",
        [invocation],
      );
      await owner.query(
        "UPDATE webdock_auth.plugin_invocation SET capabilities='[\"plugin.data.write\"]' WHERE id=$1",
        [invocation],
      );
      expect(
        (
          await scope(0, () =>
            api.query(
              "UPDATE webdock_auth.plugin_document SET revision=revision+1 WHERE plugin_id=$1",
              [plugin],
            ),
          )
        ).rowCount,
      ).toBe(1);
      expect(
        (
          await scope(0, () =>
            api.query(
              "SELECT result FROM webdock_auth.plugin_document_receipt WHERE plugin_id=$1",
              [plugin],
            ),
          )
        ).rows,
      ).toHaveLength(1);
      await owner.query(
        'UPDATE webdock_auth.plugin_invocation SET capabilities=\'["plugin.data.read","plugin.data.write"]\' WHERE id=$1',
        [invocation],
      );
      await owner.query(
        "UPDATE webdock_auth.plugin_invocation SET intent='read' WHERE id=$1",
        [invocation],
      );
      await expect(
        scope(0, () =>
          api.query(
            "UPDATE webdock_auth.plugin_document SET value='null' WHERE plugin_id=$1",
            [plugin],
          ),
        ),
      ).rejects.toThrow();
      await owner.query(
        "UPDATE webdock_auth.plugin_invocation SET intent='write' WHERE id=$1",
        [invocation],
      );
      await owner.query(
        "UPDATE webdock_auth.plugin_grant SET revision=revision+1 WHERE plugin_id=$1 AND customer_id=$2",
        [plugin, id(0)],
      );
      expect(
        (
          await scope(0, () =>
            api.query(
              "SELECT * FROM webdock_auth.plugin_document WHERE plugin_id=$1",
              [plugin],
            ),
          )
        ).rows,
      ).toEqual([]);
      await owner.query(
        "UPDATE webdock_auth.plugin_invocation SET grant_revision=1 WHERE id=$1",
        [invocation],
      );
      await owner.query('DELETE FROM webdock_auth.member WHERE "userId"=$1', [
        id(1),
      ]);
      expect(
        (
          await scope(0, () =>
            api.query(
              "SELECT * FROM webdock_auth.plugin_document WHERE plugin_id=$1",
              [plugin],
            ),
          )
        ).rows,
      ).toEqual([]);
      // Durable jobs are authorized by their project grant, never by a fabricated browser session.
      const claimToken = randomUUID();
      await owner.query(
        "INSERT INTO webdock_auth.plugin_job_execution(id,plugin_id,customer_id,project_id,name,slot,grant_revision,runtime_generation,status,claim_token,lease_until) VALUES($1,$2,$3,$4,'maintenance',now(),1,1,'running',$5,now()+interval '60 seconds')",
        [id(5), plugin, id(0), id(3), claimToken],
      );
      await expect(
        owner.query(
          "INSERT INTO webdock_auth.plugin_job_execution(id,plugin_id,customer_id,project_id,name,slot,grant_revision,runtime_generation) SELECT $2,plugin_id,customer_id,project_id,name,slot,grant_revision,runtime_generation FROM webdock_auth.plugin_job_execution WHERE id=$1",
          [id(5), id(8)],
        ),
      ).rejects.toThrow();
      await owner.query(
        "INSERT INTO webdock_auth.plugin_invocation(id,plugin_id,customer_id,project_id,runtime_generation,grant_revision,kind,name,intent,job_execution_id,operation_id,capabilities,expires_at,job_claim_token) VALUES($1,$2,$3,$4,1,1,'job','maintenance','write',$5,$5,'[\"plugin.data.read\",\"plugin.data.write\"]',now()+interval '50 seconds',$6)",
        [jobInvocation, plugin, id(0), id(3), id(5), claimToken],
      );
      const job = <T>(run: () => T) =>
        withDatabaseScope(
          {
            kind: "system",
            purpose: "plugin-job",
            pluginInvocationID: jobInvocation,
          },
          run,
        );
      expect(
        (
          await job(() =>
            worker.query(
              "SELECT * FROM webdock_auth.plugin_document WHERE plugin_id=$1",
              [plugin],
            ),
          )
        ).rows,
      ).toHaveLength(1);
      await owner.query(
        "UPDATE webdock_auth.plugin_job_execution SET claim_token=$2 WHERE id=$1",
        [id(5), randomUUID()],
      );
      expect(
        (
          await job(() =>
            worker.query(
              "SELECT * FROM webdock_auth.plugin_document WHERE plugin_id=$1",
              [plugin],
            ),
          )
        ).rows,
      ).toEqual([]);
      await owner.query(
        "UPDATE webdock_auth.plugin_job_execution SET claim_token=$2 WHERE id=$1",
        [id(5), claimToken],
      );
      await owner.query(
        "UPDATE webdock_auth.plugin_job_execution SET lease_until=now()-interval '1 second' WHERE id=$1",
        [id(5)],
      );
      expect(
        (
          await job(() =>
            worker.query(
              "SELECT * FROM webdock_auth.plugin_document WHERE plugin_id=$1",
              [plugin],
            ),
          )
        ).rows,
      ).toEqual([]);
    } finally {
      for (const table of [
        "plugin_document_receipt",
        "plugin_invocation",
        "plugin_job_execution",
        "plugin_document",
        "plugin_grant",
        "plugin_runtime",
        "plugin_release",
        "plugin_configuration",
        "plugin_installation",
      ])
        await owner.query(
          `DELETE FROM webdock_auth.${table} WHERE ${table === "plugin_installation" ? "id" : "plugin_id"}=$1`,
          [plugin],
        );
      await owner.query("DELETE FROM webdock_admin.projects WHERE id=ANY($1)", [
        [id(3), id(6), id(13)],
      ]);
      await owner.query(
        "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1)",
        [[id(0), id(10)]],
      );
      await owner.query(
        "DELETE FROM webdock_admin.customers WHERE id=ANY($1)",
        [[id(0), id(10)]],
      );
      await owner.query('DELETE FROM webdock_auth."user" WHERE id=ANY($1)', [
        [id(1), id(11)],
      ]);
      await Promise.all([owner.end(), api.end(), worker.end()]);
    }
  },
);
(enabled ? test : test.skip)(
  "participant heartbeat writes require the exact signed participant identity",
  async () => {
    const fixture = JSON.parse(await readFile(enabled!, "utf8"));
    const ownerURL = new URL(process.env.API_TEST_OWNER_URL!);
    if (
      ownerURL.hostname !== "127.0.0.1" ||
      ownerURL.pathname !== "/webdock_rls_test"
    )
      throw Error("Isolated fixture required");
    const owner = new Pool({ connectionString: ownerURL.href, max: 1 });
    const worker = new ScopedPool(
      { connectionString: fixture.roles.webdock_worker_runtime, max: 1 },
      { secret: fixture.contextSecret, audience: "webdock_worker_runtime" },
    );
    const participantID = randomUUID(),
      foreign = randomUUID();
    const own = <T>(run: () => T) =>
      withDatabaseScope(
        { kind: "system", purpose: "plugin-participant", participantID },
        run,
      );
    try {
      await own(() =>
        worker.query(
          "INSERT INTO webdock_auth.plugin_participant(id,kind,protocol,domains,generation,expires_at) VALUES($1,'worker',1,'[\"mail\"]',1,now()+interval '60 seconds')",
          [participantID],
        ),
      );
      expect(
        (
          await own(() =>
            worker.query(
              "UPDATE webdock_auth.plugin_participant SET heartbeat_at=now(),expires_at=now()+interval '60 seconds' WHERE id=$1",
              [participantID],
            ),
          )
        ).rowCount,
      ).toBe(1);
      await expect(
        own(() =>
          worker.query(
            "INSERT INTO webdock_auth.plugin_participant(id,kind,protocol,domains,generation,expires_at) VALUES($1,'worker',1,'[]',1,now()+interval '60 seconds')",
            [foreign],
          ),
        ),
      ).rejects.toThrow();
      expect(
        (
          await withDatabaseScope(
            {
              kind: "system",
              purpose: "plugin-participant",
              participantID: foreign,
            },
            () =>
              worker.query(
                "SELECT id FROM webdock_auth.plugin_participant WHERE id=$1",
                [participantID],
              ),
          )
        ).rows,
      ).toEqual([]);
    } finally {
      await owner.query(
        "DELETE FROM webdock_auth.plugin_participant WHERE id=ANY($1)",
        [[participantID, foreign]],
      );
      await Promise.all([owner.end(), worker.end()]);
    }
  },
);
