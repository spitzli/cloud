import { test, expect } from "bun:test";
import { readFile } from "node:fs/promises";
import { randomBytes } from "node:crypto";
import { Pool } from "pg";
import { withDatabaseScope } from "@webdock/database-runtime";
import { createDatabases } from "../src/db/client";
const fixtureFile = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(fixtureFile ? test : test.skip)(
  "native budget RLS denies tenant mutation and unrelated worker authority",
  async () => {
    const url = new URL(
      (process.env.API_TEST_OWNER_URL || process.env.DATABASE_URL)!,
    );
    if (url.hostname !== "127.0.0.1" || url.pathname !== "/webdock_rls_test")
      throw Error("Only isolated RLS fixture is allowed");
    const fixture = JSON.parse(await readFile(fixtureFile!, "utf8"));
    const owner = new Pool({ connectionString: url.href, max: 1 });
    const pools = createDatabases({
      environment: "test",
      businessURL: fixture.roles.webdock_auth_runtime,
      authURL: fixture.roles.webdock_auth_bootstrap,
      workerURL: fixture.roles.webdock_worker_runtime,
      signingSecret: fixture.contextSecret,
      max: 1,
    });
    const base = BigInt(
      "93" + BigInt("0x" + randomBytes(4).toString("hex")).toString(),
    );
    const [user, session, customer, other] = [0, 1, 2, 3].map((n) =>
      (base + BigInt(n)).toString(),
    );
    const asUser = <T>(fn: () => T) =>
      withDatabaseScope(
        { kind: "session", subject: user, sessionID: session },
        fn,
      );
    try {
      await owner.query(
        `INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword") VALUES($1,'RLS fixture',$2,'user',true,false)`,
        [user, user + "@example.invalid"],
      );
      await owner.query(
        `INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval '1 hour',now())`,
        [session, user],
      );
      await owner.query(
        "INSERT INTO webdock_admin.customers(id,name,status) VALUES($1,'Budget fixture','active'),($2,'Other budget fixture','active')",
        [customer, other],
      );
      await owner.query(
        `INSERT INTO webdock_auth.member("userId","organizationId",role,"createdAt") SELECT $1,organization_id,'admin',now() FROM webdock_auth.tenant_customer WHERE customer_id=$2`,
        [user, customer],
      );
      await owner.query(
        "INSERT INTO webdock_mail.month_usage(customer_id,period_start,reserved) VALUES($1,current_date,1),($2,current_date,2)",
        [customer, other],
      );
      const read = () =>
        pools.business.query(
          "SELECT customer_id FROM webdock_mail.month_usage WHERE customer_id=ANY($1)",
          [[customer, other]],
        );
      expect((await asUser(read)).rows).toEqual([{ customer_id: customer }]);
      expect((await read()).rows).toEqual([]);
      await expect(
        asUser(() =>
          pools.business.query(
            "UPDATE webdock_mail.month_usage SET reserved=0 WHERE customer_id=$1",
            [customer],
          ),
        ),
      ).rejects.toHaveProperty("code", "42501");
      const unrelated = await withDatabaseScope(
        { kind: "system", purpose: "git-worker" },
        () =>
          pools.worker.query(
            "SELECT customer_id FROM webdock_mail.month_usage WHERE customer_id=$1",
            [customer],
          ),
      );
      expect(unrelated.rowCount).toBe(0);
      await withDatabaseScope(
        { kind: "system", purpose: "mail-send-gateway" },
        () =>
          pools.worker.query(
            "UPDATE webdock_mail.month_usage SET reserved=reserved+1 WHERE customer_id=$1",
            [customer],
          ),
      );
      expect(
        (
          await owner.query(
            "SELECT reserved FROM webdock_mail.month_usage WHERE customer_id=$1",
            [customer],
          )
        ).rows[0].reserved,
      ).toBe("2");
      await owner.query("DELETE FROM webdock_auth.session WHERE id=$1", [
        session,
      ]);
      expect((await asUser(read)).rowCount).toBe(0);
    } finally {
      await owner.query(
        "DELETE FROM webdock_mail.month_usage WHERE customer_id=ANY($1)",
        [[customer, other]],
      );
      await owner.query('DELETE FROM webdock_auth.member WHERE "userId"=$1', [
        user,
      ]);
      const orgs = (
        await owner.query(
          "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1) RETURNING organization_id",
          [[customer, other]],
        )
      ).rows.map((x) => x.organization_id);
      await owner.query(
        "DELETE FROM webdock_admin.customers WHERE id=ANY($1)",
        [[customer, other]],
      );
      await owner.query(
        "DELETE FROM webdock_auth.organization WHERE id=ANY($1)",
        [orgs],
      );
      await owner.query('DELETE FROM webdock_auth."user" WHERE id=$1', [user]);
      await pools.close();
      await owner.end();
    }
  },
);
