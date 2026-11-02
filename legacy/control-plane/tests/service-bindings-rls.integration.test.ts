import { test, expect } from "bun:test";
import { Pool } from "pg";
import { randomBytes } from "node:crypto";
import { ScopedPool, withDatabaseScope } from "@webdock/database-runtime";
const enabled = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(enabled ? test : test.skip)(
  "bindings enforce tenant and agent-cluster ownership with restricted database roles",
  async () => {
    const url = new URL(process.env.API_TEST_OWNER_URL!);
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Isolated RLS fixture required");
    const owner = new Pool({ connectionString: url.href, max: 1 });
    const app = new ScopedPool(
      { connectionString: process.env.DATABASE_URL, max: 1 },
      {
        secret: process.env.DATABASE_RLS_SECRET,
        audience: "webdock_auth_runtime",
      },
    );
    const worker = new ScopedPool(
      { connectionString: process.env.DATABASE_WORKER_URL, max: 1 },
      {
        secret: process.env.DATABASE_RLS_SECRET,
        audience: "webdock_worker_runtime",
      },
    );
    const seed = async (sql: string, values?: unknown[]) => {
      try {
        return await owner.query(sql, values);
      } catch (error) {
        throw new Error(sql, { cause: error });
      }
    };
    const base = BigInt(
      "95" + BigInt("0x" + randomBytes(5).toString("hex")).toString(),
    );
    const id = (n: number) => (base + BigInt(n)).toString();
    const ids = [id(0), id(10)],
      orgs: string[] = [];
    try {
      for (let n = 0; n < 2; n++) {
        const offset = n * 10;
        await seed(
          'INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword","twoFactorEnabled") VALUES($1::varchar,$1::text,$2,\'user\',true,false,false)',
          [id(offset + 1), id(offset + 1) + "@example.invalid"],
        );
        await seed(
          'INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1::varchar,$1::text,$2,now()+interval \'1 hour\',now())',
          [id(offset + 2), id(offset + 1)],
        );
        await seed(
          "INSERT INTO webdock_admin.customers(id,name,status) VALUES($1::varchar,$1::text,'active')",
          [id(offset)],
        );
        const org = (
          await seed(
            "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
            [id(offset)],
          )
        ).rows[0].organization_id;
        orgs.push(org);
        await seed(
          'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now())',
          [org, id(offset + 1)],
        );
        await seed(
          "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES($1::varchar,$1::text,$2,'active')",
          [id(offset + 3), id(offset)],
        );
        await seed(
          "INSERT INTO webdock_auth.hosting_cluster(id,name,provider,country,region,location_evidence) VALUES($1::varchar,$1::text,'fixture','DE','test','Disposable local test')",
          [id(offset + 4)],
        );
        await seed(
          "INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,mode,cluster_id,namespace) VALUES($1,$2,'k3s','selfservice',$3,$4)",
          [id(offset + 3), id(offset), id(offset + 4), "wd-" + id(offset + 3)],
        );
        await seed(
          "INSERT INTO webdock_auth.hosting_app(id,project_id,name,spec,lifecycle,status) VALUES($1::varchar,$2,$1::text,'{}','draft','draft')",
          [id(offset + 5), id(offset + 3)],
        );
      }
      const scope = <T>(n: number, run: () => T) =>
        withDatabaseScope(
          {
            kind: "session",
            subject: id(n * 10 + 1),
            sessionID: id(n * 10 + 2),
          },
          run,
        );
      await scope(0, () =>
        app.query(
          'INSERT INTO webdock_auth.hosting_service_binding(id,customer_id,project_id,app_id,service,variable) VALUES($1,$2,$3,$4,\'{"kind":"postgres","databaseID":"42","variable":"DATABASE_URL"}\',\'DATABASE_URL\')',
          [id(6), id(0), id(3), id(5)],
        ),
      );
      expect(
        (
          await scope(0, () =>
            app.query(
              "SELECT id FROM webdock_auth.hosting_service_binding WHERE id=$1",
              [id(6)],
            ),
          )
        ).rowCount,
      ).toBe(1);
      expect(
        (
          await scope(1, () =>
            app.query(
              "SELECT id FROM webdock_auth.hosting_service_binding WHERE id=$1",
              [id(6)],
            ),
          )
        ).rowCount,
      ).toBe(0);
      expect(
        (
          await scope(1, () =>
            app.query(
              "UPDATE webdock_auth.hosting_service_binding SET state='disconnected' WHERE id=$1",
              [id(6)],
            ),
          )
        ).rowCount,
      ).toBe(0);
      await expect(
        scope(1, () =>
          app.query(
            "INSERT INTO webdock_auth.hosting_service_binding(customer_id,project_id,app_id,service,variable) VALUES($1,$2,$3,'{}','OTHER')",
            [id(0), id(3), id(5)],
          ),
        ),
      ).rejects.toThrow();
      expect(
        (
          await withDatabaseScope(
            { kind: "system", purpose: "agent", clusterID: id(14) },
            () =>
              worker.query(
                "SELECT id FROM webdock_auth.hosting_service_binding WHERE id=$1",
                [id(6)],
              ),
          )
        ).rowCount,
      ).toBe(0);
      expect(
        (
          await withDatabaseScope(
            { kind: "system", purpose: "agent", clusterID: id(4) },
            () =>
              worker.query(
                "SELECT id FROM webdock_auth.hosting_service_binding WHERE id=$1",
                [id(6)],
              ),
          )
        ).rowCount,
      ).toBe(1);
      await seed(
        "UPDATE webdock_auth.member SET role='member' WHERE \"organizationId\"=$1",
        [orgs[0]],
      );
      await expect(
        scope(0, () =>
          app.query(
            "UPDATE webdock_auth.hosting_service_binding SET state='disconnected' WHERE id=$1",
            [id(6)],
          ),
        ),
      ).rejects.toThrow();
    } finally {
      await seed(
        "DELETE FROM webdock_auth.hosting_service_binding WHERE customer_id=ANY($1)",
        [ids],
      );
      await seed(
        "DELETE FROM webdock_auth.hosting_app WHERE project_id=ANY($1)",
        [[id(3), id(13)]],
      );
      await seed(
        "DELETE FROM webdock_auth.hosting_project WHERE customer_id=ANY($1)",
        [ids],
      );
      await seed("DELETE FROM webdock_auth.hosting_cluster WHERE id=ANY($1)", [
        [id(4), id(14)],
      ]);
      await seed(
        "DELETE FROM webdock_admin.projects WHERE customer_id=ANY($1)",
        [ids],
      );
      await seed(
        'DELETE FROM webdock_auth.member WHERE "organizationId"=ANY($1)',
        [orgs],
      );
      await seed(
        "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1)",
        [ids],
      );
      await seed("DELETE FROM webdock_admin.customers WHERE id=ANY($1)", [ids]);
      await seed("DELETE FROM webdock_auth.organization WHERE id=ANY($1)", [
        orgs,
      ]);
      await seed('DELETE FROM webdock_auth.session WHERE "userId"=ANY($1)', [
        [id(1), id(11)],
      ]);
      await seed('DELETE FROM webdock_auth."user" WHERE id=ANY($1)', [
        [id(1), id(11)],
      ]);
      await app.end();
      await worker.end();
      await owner.end();
    }
  },
);
