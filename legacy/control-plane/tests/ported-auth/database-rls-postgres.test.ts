import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { randomBytes, createHmac } from "node:crypto";
import pg from "pg";
import { ScopedPool, withDatabaseScope } from "@webdock/database-runtime";

// Explicit opt-in: uses only synthetic rows in the disposable local restore.
const fixtureFile = process.env.WEBDOCK_RLS_FIXTURE_FILE;
test(
  "real PostgreSQL roles enforce signed tenant, preview, key and worker isolation",
  { skip: !fixtureFile },
  async () => {
    const ownerURL = process.env.DATABASE_URL!;
    const endpoint = new URL(ownerURL);
    assert.ok(["localhost", "127.0.0.1"].includes(endpoint.hostname));
    assert.equal(endpoint.pathname, "/webdock_rls_test");
    const fixture = JSON.parse(readFileSync(fixtureFile!, "utf8"));
    const owner = new pg.Pool({ connectionString: ownerURL, max: 1 });
    const runtime = new ScopedPool(
      { connectionString: fixture.roles.webdock_auth_runtime, max: 2 },
      { secret: fixture.contextSecret, audience: "webdock_auth_runtime" },
    );
    const worker = new ScopedPool(
      { connectionString: fixture.roles.webdock_worker_runtime, max: 1 },
      { secret: fixture.contextSecret, audience: "webdock_worker_runtime" },
    );
    const bootstrap = new pg.Pool({
      connectionString: fixture.roles.webdock_auth_bootstrap,
      max: 1,
    });
    const raw = new pg.Pool({
      connectionString: fixture.roles.webdock_auth_runtime,
      max: 1,
    });
    const browser = new pg.Pool({
      connectionString: fixture.roles.webdock_browser_website,
      max: 1,
    });
    const base = BigInt(
      "91" + BigInt("0x" + randomBytes(5).toString("hex")).toString(),
    );
    const id = (n: number) => (base + BigInt(n)).toString();
    const [
      a,
      b,
      op,
      ca,
      cb,
      oa,
      ob,
      sa,
      sb,
      so,
      pa,
      pb,
      storea,
      storeb,
      conna,
      connb,
      keya,
      keyb,
    ] = Array.from({ length: 18 }, (_, i) => id(i));
    const reader = id(40),
      sr = id(41),
      clusterA = id(50),
      clusterB = id(51);
    const asReader = <T>(run: () => T) =>
      withDatabaseScope(
        { kind: "session", subject: reader, sessionID: sr },
        run,
      );
    const asA = <T>(run: () => T) =>
      withDatabaseScope({ kind: "session", subject: a, sessionID: sa }, run);
    const asB = <T>(run: () => T) =>
      withDatabaseScope({ kind: "session", subject: b, sessionID: sb }, run);
    const asOp = <T>(run: () => T) =>
      withDatabaseScope({ kind: "session", subject: op, sessionID: so }, run);
    try {
      for (const [subject, session, role] of [
        [a, sa, "user"],
        [b, sb, "user"],
        [op, so, "operator"],
        [reader, sr, "user"],
      ]) {
        await owner.query(
          'INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword","twoFactorEnabled") VALUES($1,$1,$2,$3,true,false,true)',
          [subject, subject + "@rls.invalid", role],
        );
        await owner.query(
          'INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval \'1 hour\',now())',
          [session, subject],
        );
      }
      for (const [customer, org, subject, project, store, connection, key] of [
        [ca, oa, a, pa, storea, conna, keya],
        [cb, ob, b, pb, storeb, connb, keyb],
      ]) {
        await owner.query(
          "INSERT INTO webdock_admin.customers(id,name,status) VALUES($1,$1,'active')",
          [customer],
        );
        const generatedOrg = (
          await owner.query(
            "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
            [customer],
          )
        ).rows[0]?.organization_id;
        await owner.query(
          'INSERT INTO webdock_auth.organization(id,name,slug,"createdAt") VALUES($1,$1,$1,now())',
          [org],
        );
        await owner.query(
          'INSERT INTO webdock_auth.member(id,"organizationId","userId",role,"createdAt") VALUES($1,$2,$3,\'owner\',now())',
          [id(Number(subject === a ? 30 : 31)), org, subject],
        );
        await owner.query(
          "UPDATE webdock_auth.tenant_customer SET organization_id=$2 WHERE customer_id=$1",
          [customer, org],
        );
        if (generatedOrg)
          await owner.query(
            "DELETE FROM webdock_auth.organization WHERE id=$1",
            [generatedOrg],
          );
        await owner.query(
          "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES($1,$1,$2,'active')",
          [project, customer],
        );
        await owner.query(
          "INSERT INTO webdock_auth.tenant_storage(id,customer_id,label,environment,store_id,prefix,encrypted_token) VALUES($1,$2,$1,'production',$1,'rls-test/','synthetic')",
          [store, customer],
        );
        await owner.query(
          "INSERT INTO webdock_auth.mail_connection(id,customer_id,kind,provider_account_id,state) VALUES($1,$2,'byok',$1,'ready')",
          [connection, customer],
        );
        await owner.query(
          "INSERT INTO webdock_auth.mail_api_key(id,customer_id,connection_id,name,prefix,key_hash,permission) VALUES($1,$2,$3,$1,$1,$1,'sending_access')",
          [key, customer, connection],
        );
      }
      assert.match(
        (await bootstrap.query("SELECT webdock_auth.next_snowflake() AS id"))
          .rows[0].id,
        /^[0-9]+$/,
      );
      await asA(() =>
        runtime.query(
          "INSERT INTO webdock_auth.access_event(actor_id,action,target_id,outcome) VALUES($1,'rls-synthetic',$2,'succeeded')",
          [a, ca],
        ),
      );
      for (const [customer, project, app] of [
        [ca, pa, id(60)],
        [cb, pb, id(61)],
      ]) {
        await owner.query(
          "INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,mode) VALUES($1,$2,'vercel','selfservice')",
          [project, customer],
        );
        await owner.query(
          "INSERT INTO webdock_auth.hosting_app(id,project_id,name,spec) VALUES($1,$2,'synthetic','{}')",
          [app, project],
        );
      }
      assert.deepEqual(
        (
          await asA(() =>
            runtime.query("SELECT id FROM webdock_auth.hosting_app"),
          )
        ).rows,
        [{ id: id(60) }],
      );
      await assert.rejects(
        asA(() =>
          runtime.query(
            "UPDATE webdock_auth.hosting_app SET project_id=$2 WHERE id=$1",
            [id(60), pb],
          ),
        ),
        /row-level security/,
      );
      await owner.query(
        'INSERT INTO webdock_auth.member(id,"organizationId","userId",role,"createdAt") VALUES($1,$2,$3,\'member\',now())',
        [id(42), oa, reader],
      );
      await owner.query(
        'INSERT INTO webdock_auth.member(id,"organizationId","userId",role,"createdAt") VALUES($1,$2,$3,\'admin\',now())',
        [id(43), ob, reader],
      );
      // A reader in A who administers B must not steal A's row by assigning B's key.
      await assert.rejects(
        asReader(() =>
          runtime.query(
            "UPDATE webdock_auth.tenant_storage SET customer_id=$2,label='stolen' WHERE id=$1",
            [storea, cb],
          ),
        ),
        /row-level security/,
      );
      await assert.rejects(
        asReader(() =>
          runtime.query(
            "UPDATE webdock_auth.hosting_app SET project_id=$2,name='stolen-child' WHERE id=$1",
            [id(60), pb],
          ),
        ),
        /row-level security/,
      );
      assert.equal(
        (
          await asReader(() =>
            runtime.query(
              "UPDATE webdock_auth.tenant_storage SET label='allowed-in-b' WHERE id=$1",
              [storeb],
            ),
          )
        ).rowCount,
        1,
      );
      await owner.query("DELETE FROM webdock_auth.member WHERE id=$1", [
        id(43),
      ]);
      for (const cluster of [clusterA, clusterB])
        await owner.query(
          "INSERT INTO webdock_auth.hosting_cluster(id,name,provider,country,region,location_evidence) VALUES($1::text,$1::text,'contabo','DE','eu','synthetic')",
          [cluster],
        );
      await asReader(async () => {
        const c = await runtime.connect();
        try {
          await c.query("BEGIN");
          assert.equal(
            (
              await c.query(
                "SELECT id FROM webdock_admin.customers WHERE id=$1 FOR SHARE",
                [ca],
              )
            ).rowCount,
            1,
          );
          assert.equal(
            (
              await c.query(
                'SELECT id FROM webdock_auth.member WHERE "userId"=$1 FOR SHARE',
                [reader],
              )
            ).rowCount,
            1,
          );
          await c.query("ROLLBACK");
        } finally {
          c.release();
        }
      });
      await assert.rejects(
        asReader(() =>
          runtime.query(
            "UPDATE webdock_auth.member SET role='admin' WHERE id=$1",
            [id(42)],
          ),
        ),
        /row-level security/,
      );
      await assert.rejects(
        asReader(() =>
          runtime.query(
            "UPDATE webdock_admin.customers SET name='denied' WHERE id=$1",
            [ca],
          ),
        ),
        /row-level security/,
      );
      await assert.rejects(
        asReader(() =>
          runtime.query(
            "UPDATE webdock_auth.\"user\" SET role='operator' WHERE id=$1",
            [reader],
          ),
        ),
        /row-level security/,
      );
      await owner.query("DELETE FROM webdock_auth.member WHERE id=$1", [
        id(42),
      ]);
      assert.equal(
        (
          await asReader(() =>
            runtime.query("SELECT id FROM webdock_admin.customers"),
          )
        ).rowCount,
        0,
        "revoked membership loses access",
      );
      await withDatabaseScope(
        { kind: "system", purpose: "agent", clusterID: clusterA },
        async () =>
          assert.deepEqual(
            (
              await worker.query(
                "SELECT id FROM webdock_auth.hosting_cluster ORDER BY id",
              )
            ).rows,
            [{ id: clusterA }],
          ),
      );
      await withDatabaseScope({ kind: "system", purpose: "agent" }, async () =>
        assert.equal(
          (await worker.query("SELECT id FROM webdock_auth.hosting_cluster"))
            .rowCount,
          0,
        ),
      );
      assert.equal(
        (await runtime.query("SELECT id FROM webdock_auth.tenant_storage"))
          .rowCount,
        0,
        "no context",
      );
      assert.deepEqual(
        (
          await asA(() =>
            runtime.query(
              "SELECT id FROM webdock_auth.tenant_storage ORDER BY id",
            ),
          )
        ).rows,
        [{ id: storea }],
      );
      assert.deepEqual(
        (
          await asB(() =>
            runtime.query(
              "SELECT id FROM webdock_auth.tenant_storage ORDER BY id",
            ),
          )
        ).rows,
        [{ id: storeb }],
      );
      assert.equal(
        (
          await asA(() =>
            runtime.query(
              "UPDATE webdock_auth.tenant_storage SET label='denied' WHERE id=$1",
              [storeb],
            ),
          )
        ).rowCount,
        0,
      );
      await assert.rejects(
        asA(() =>
          runtime.query(
            "UPDATE webdock_auth.tenant_storage SET customer_id=$2 WHERE id=$1",
            [storea, cb],
          ),
        ),
        /row-level security/,
      );
      assert.equal(
        (
          await asA(() =>
            runtime.query(
              "UPDATE webdock_auth.tenant_storage SET label='allowed' WHERE id=$1",
              [storea],
            ),
          )
        ).rowCount,
        1,
      );
      await assert.rejects(
        asA(() =>
          runtime.query(
            "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
            [ca],
          ),
        ),
        /permission denied/,
      );
      await assert.rejects(
        asA(() =>
          runtime.query("DELETE FROM webdock_admin.customers WHERE id=$1", [
            ca,
          ]),
        ),
        /permission denied/,
      );
      await assert.rejects(
        asA(() =>
          runtime.query(
            "SELECT notes FROM webdock_admin.customers WHERE id=$1",
            [ca],
          ),
        ),
        /permission denied/,
      );
      await assert.rejects(
        asA(() =>
          runtime.query(
            "UPDATE webdock_admin.projects SET name='denied' WHERE id=$1",
            [pa],
          ),
        ),
        /permission denied/,
      );
      await asA(async () => {
        const c = await runtime.connect();
        try {
          await c.query("BEGIN");
          assert.equal(
            (
              await c.query(
                'SELECT id FROM webdock_auth."user" WHERE id=$1 FOR SHARE',
                [a],
              )
            ).rowCount,
            1,
          );
          assert.equal(
            (
              await c.query(
                "SELECT id FROM webdock_auth.session WHERE id=$1 FOR SHARE",
                [sa],
              )
            ).rowCount,
            1,
          );
          await c.query("ROLLBACK");
        } finally {
          c.release();
        }
      });
      assert.equal(
        (await runtime.query("SELECT id FROM webdock_auth.tenant_storage"))
          .rowCount,
        0,
        "pool scope does not leak",
      );
      assert.equal(
        (
          await asOp(() =>
            runtime.query(
              "SELECT id FROM webdock_auth.tenant_storage WHERE id=ANY($1)",
              [[storea, storeb]],
            ),
          )
        ).rowCount,
        2,
      );
      await asOp(() =>
        runtime.query(
          "INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id) VALUES($1,$2,$3,$4)",
          [so, op, ca, oa],
        ),
      );
      assert.deepEqual(
        (
          await asOp(() =>
            runtime.query(
              "SELECT id FROM webdock_auth.tenant_storage WHERE id=ANY($1)",
              [[storea, storeb]],
            ),
          )
        ).rows,
        [{ id: storea }],
      );
      await assert.rejects(
        asOp(() =>
          runtime.query(
            "UPDATE webdock_auth.tenant_storage SET label='denied' WHERE id=$1",
            [storea],
          ),
        ),
        /row-level security/,
      );
      await owner.query(
        "UPDATE webdock_auth.studio_tenant_preview SET expires_at=now()-interval '1 minute' WHERE session_id=$1",
        [so],
      );
      assert.equal(
        (
          await asOp(() =>
            runtime.query(
              "SELECT id FROM webdock_auth.tenant_storage WHERE id=ANY($1)",
              [[storea, storeb]],
            ),
          )
        ).rowCount,
        0,
        "expired preview stays restrictive",
      );
      assert.equal(
        (
          await asOp(() =>
            runtime.query(
              "UPDATE webdock_auth.studio_tenant_preview SET expired_at=now() WHERE session_id=$1",
              [so],
            ),
          )
        ).rowCount,
        1,
      );
      assert.equal(
        (
          await asOp(() =>
            runtime.query(
              "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1",
              [so],
            ),
          )
        ).rowCount,
        1,
      );
      assert.equal(
        (
          await asOp(() =>
            runtime.query(
              "SELECT id FROM webdock_auth.tenant_storage WHERE id=ANY($1)",
              [[storea, storeb]],
            ),
          )
        ).rowCount,
        2,
      );
      await withDatabaseScope(
        { kind: "mail-key", customerID: ca, connectionID: conna, keyID: keya },
        async () => {
          assert.deepEqual(
            (
              await runtime.query(
                "SELECT id FROM webdock_auth.mail_connection ORDER BY id",
              )
            ).rows,
            [{ id: conna }],
          );
          await assert.rejects(
            runtime.query(
              "UPDATE webdock_auth.mail_api_key SET permission='full_access' WHERE id=$1",
              [keya],
            ),
            /row-level security/,
          );
        },
      );
      await withDatabaseScope(
        { kind: "mail-key", customerID: cb, connectionID: connb, keyID: keya },
        async () =>
          assert.equal(
            (await runtime.query("SELECT id FROM webdock_auth.mail_connection"))
              .rowCount,
            0,
          ),
      );
      await owner.query(
        "UPDATE webdock_auth.mail_api_key SET revoked_at=now() WHERE id=$1",
        [keya],
      );
      await withDatabaseScope(
        { kind: "mail-key", customerID: ca, connectionID: conna, keyID: keya },
        async () =>
          assert.equal(
            (await runtime.query("SELECT id FROM webdock_auth.mail_connection"))
              .rowCount,
            0,
          ),
      );
      const c = await raw.connect();
      try {
        await c.query("BEGIN");
        await c.query(
          "SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",
          [
            JSON.stringify({
              v: 1,
              aud: "webdock_auth_runtime",
              exp: Math.floor(Date.now() / 1000) + 60,
              scope: { kind: "session", subject: op, sessionID: so },
            }),
            "0".repeat(64),
          ],
        );
        assert.equal(
          (await c.query("SELECT id FROM webdock_auth.tenant_storage"))
            .rowCount,
          0,
        );
        await c.query("ROLLBACK");
        for (const envelope of [
          {
            v: 1,
            aud: "webdock_worker_runtime",
            exp: Math.floor(Date.now() / 1000) + 60,
            scope: { kind: "system", purpose: "mail-cron" },
          },
          {
            v: 1,
            aud: "webdock_auth_runtime",
            exp: Math.floor(Date.now() / 1000) - 1,
            scope: { kind: "session", subject: op, sessionID: so },
          },
        ]) {
          const text = JSON.stringify(envelope);
          await c.query("BEGIN");
          await c.query(
            "SELECT set_config('webdock.rls_context',$1,true),set_config('webdock.rls_signature',$2,true)",
            [
              text,
              createHmac("sha256", fixture.contextSecret)
                .update(text)
                .digest("hex"),
            ],
          );
          assert.equal(
            (await c.query("SELECT webdock_rls.request_scope() AS scope"))
              .rows[0].scope,
            null,
          );
          await c.query("ROLLBACK");
        }
      } finally {
        c.release();
      }
      await assert.rejects(
        raw.query("SET ROLE webdock_platform_owner"),
        /permission denied/,
      );
      await assert.rejects(
        browser.query("SELECT secret FROM webdock_rls.context_key"),
        /permission denied/,
      );
      await assert.rejects(
        browser.query('SELECT id FROM webdock_auth."user"'),
        /permission denied/,
      );
      await withDatabaseScope(
        { kind: "system", purpose: "storage-cron" },
        async () =>
          assert.equal(
            (await worker.query("SELECT id FROM webdock_auth.mail_connection"))
              .rowCount,
            0,
            "worker purpose is bounded",
          ),
      );
    } finally {
      await owner.query(
        "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1",
        [so],
      );
      await owner.query(
        "DELETE FROM webdock_auth.mail_api_key WHERE id=ANY($1)",
        [[keya, keyb]],
      );
      await owner.query(
        "DELETE FROM webdock_auth.mail_connection WHERE id=ANY($1)",
        [[conna, connb]],
      );
      await owner.query(
        "DELETE FROM webdock_auth.tenant_storage WHERE id=ANY($1)",
        [[storea, storeb]],
      );
      await owner.query(
        "DELETE FROM webdock_auth.hosting_app WHERE id=ANY($1)",
        [[id(60), id(61)]],
      );
      await owner.query(
        "DELETE FROM webdock_auth.hosting_project WHERE project_id=ANY($1)",
        [[pa, pb]],
      );
      await owner.query(
        "DELETE FROM webdock_auth.access_event WHERE actor_id=ANY($1)",
        [[a, b, op, reader]],
      );
      await owner.query("DELETE FROM webdock_admin.projects WHERE id=ANY($1)", [
        [pa, pb],
      ]);
      await owner.query(
        "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1)",
        [[ca, cb]],
      );
      await owner.query(
        'DELETE FROM webdock_auth.member WHERE "userId"=ANY($1)',
        [[a, b, reader]],
      );
      await owner.query(
        "DELETE FROM webdock_auth.organization WHERE id=ANY($1)",
        [[oa, ob]],
      );
      await owner.query(
        "DELETE FROM webdock_admin.customers WHERE id=ANY($1)",
        [[ca, cb]],
      );
      await owner.query("DELETE FROM webdock_auth.session WHERE id=ANY($1)", [
        [sa, sb, so],
      ]);
      await owner.query('DELETE FROM webdock_auth."user" WHERE id=ANY($1)', [
        [a, b, op],
      ]);
      await owner.query(
        "DELETE FROM webdock_auth.hosting_cluster WHERE id=ANY($1)",
        [[clusterA, clusterB]],
      );
      await Promise.all([
        owner.end(),
        bootstrap.end(),
        runtime.end(),
        worker.end(),
        raw.end(),
        browser.end(),
      ]);
    }
  },
);
