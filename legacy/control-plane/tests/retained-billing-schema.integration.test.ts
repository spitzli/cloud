import { test, expect } from "bun:test";
import { Pool } from "pg";
import { randomBytes } from "node:crypto";
import { withDatabaseScope } from "@webdock/database-runtime";
const enabled = process.env.WEBDOCK_RLS_FIXTURE_FILE;
(enabled ? test : test.skip)(
  "billing schema isolates environments and preserves immutable financial history",
  async () => {
    const url = new URL(process.env.API_TEST_OWNER_URL!);
    if (
      url.hostname !== "127.0.0.1" ||
      url.pathname !== "/webdock_rls_test" ||
      url.search
    )
      throw Error("Isolated RLS fixture required");
    const owner = new Pool({ connectionString: url.href, max: 1 });
    const { databasePools } = await import("../src/db/runtime");
    const base = BigInt(
      "98" + BigInt("0x" + randomBytes(4).toString("hex")).toString(),
    );
    const id = (n: number) => (base + BigInt(n)).toString();
    const customer = id(1),
      product = id(2),
      version = id(3),
      other = id(4),
      project = id(5),
      otherProject = id(6),
      binding = id(7),
      client = id(8),
      contract = id(9);
    try {
      await owner.query(
        "INSERT INTO webdock_admin.customers(id,name,status) VALUES($1,'Billing fixture','active')",
        [customer],
      );
      await owner.query(
        "INSERT INTO webdock_auth.billing_product(id,sku,name,family) VALUES($1,$2,'Fixture','website')",
        [product, "fixture-" + product],
      );
      await owner.query(
        "INSERT INTO webdock_auth.billing_product_version(id,product_id,version,amount,currency,tax_behavior,interval,description,entitlements,terms_hash) VALUES($1,$2,1,5880,'EUR','inclusive','year','Fixture',$3,$4)",
        [
          version,
          product,
          JSON.stringify({
            websites: 1,
            editors: 1,
            mediaBytes: "500000000",
            mailMonthly: 100,
          }),
          "a".repeat(64),
        ],
      );
      await expect(
        owner.query(
          "UPDATE webdock_auth.billing_product_version SET amount=$1 WHERE id=$2",
          ["1.5", version],
        ),
      ).rejects.toThrow();
      await expect(
        owner.query(
          "UPDATE webdock_auth.billing_product_version SET currency='USD' WHERE id=$1",
          [version],
        ),
      ).rejects.toThrow();
      await owner.query(
        "UPDATE webdock_auth.billing_product_version SET frozen=true WHERE id=$1",
        [version],
      );
      await expect(
        owner.query(
          "UPDATE webdock_auth.billing_product_version SET amount=1 WHERE id=$1",
          [version],
        ),
      ).rejects.toThrow();
      await owner.query(
        "INSERT INTO webdock_auth.billing_customer(environment,customer_id,is_test) VALUES('sandbox',$1,true),('live',$1,false)",
        [customer],
      );
      await owner.query(
        "INSERT INTO webdock_admin.customers(id,name,status) VALUES($1,'Other billing fixture','active')",
        [other],
      );
      await owner.query(
        "INSERT INTO webdock_admin.projects(id,name,customer_id,status) VALUES($1,'Billing project',$2,'active'),($3,'Other project',$4,'active')",
        [project, customer, otherProject, other],
      );
      const org = (
        await owner.query(
          "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
          [customer],
        )
      ).rows[0].organization_id;
      const otherOrg = (
        await owner.query(
          "SELECT organization_id FROM webdock_auth.tenant_customer WHERE customer_id=$1",
          [other],
        )
      ).rows[0].organization_id;
      await owner.query(
        'INSERT INTO webdock_auth."oauthClient"(id,"clientId","redirectUris") VALUES($1,$1,$2)',
        [client, "[]"],
      );
      await owner.query(
        "INSERT INTO webdock_auth.app_binding(id,client_id,label,organization_id) VALUES($1,$2,'Billing site',$3)",
        [binding, client, org],
      );
      await owner.query(
        "INSERT INTO webdock_auth.billing_customer(environment,customer_id,is_test) VALUES('sandbox',$1,true)",
        [other],
      );
      await owner.query(
        "INSERT INTO webdock_auth.billing_product_mapping(environment,version_id,status) VALUES('sandbox',$1,'ready')",
        [version],
      );
      const insert = (
        key: string,
        env: string,
        c: string,
        o: string,
        p: string,
      ) =>
        owner.query(
          "INSERT INTO webdock_auth.billing_contract(id,environment,customer_id,version_id,organization_id,binding_id,project_id,snapshot,terms_hash,accepted_by) VALUES($1,$2,$3,$4,$5,$6,$7,'{}',$8,'fixture')",
          [key, env, c, version, o, binding, p, "a".repeat(64)],
        );
      await insert(contract, "sandbox", customer, org, project);
      await expect(
        insert(id(10), "sandbox", customer, org, project),
      ).rejects.toHaveProperty("code", "23505");
      await expect(
        insert(id(11), "live", customer, org, project),
      ).rejects.toHaveProperty("code", "23503");
      await expect(
        insert(id(12), "sandbox", other, otherOrg, otherProject),
      ).rejects.toHaveProperty("code", "23503");
      await expect(
        owner.query(
          "UPDATE webdock_auth.billing_contract SET terms_hash=$1 WHERE id=$2",
          ["b".repeat(64), contract],
        ),
      ).rejects.toThrow();
      await expect(
        owner.query(
          "DELETE FROM webdock_auth.billing_customer WHERE environment='sandbox' AND customer_id=$1",
          [customer],
        ),
      ).rejects.toMatchObject({
        code: expect.stringMatching(/^(23503|23001)$/),
      });
      const sandbox = <T>(fn: () => T) =>
        withDatabaseScope(
          {
            kind: "system",
            purpose: "polar:sandbox:customer",
            customerID: customer,
          },
          fn,
        );
      const read = () =>
        databasePools.worker.query(
          "SELECT environment FROM webdock_auth.billing_customer WHERE customer_id=$1",
          [customer],
        );
      expect((await sandbox(read)).rows).toEqual([{ environment: "sandbox" }]);
      expect((await read()).rows).toEqual([]);
      expect(
        (
          await sandbox(() =>
            databasePools.worker.query(
              "UPDATE webdock_auth.billing_customer SET billing_email='wrong@example.invalid' WHERE environment='live' AND customer_id=$1 RETURNING customer_id",
              [customer],
            ),
          )
        ).rowCount,
      ).toBe(0);
      await expect(
        owner.query("DELETE FROM webdock_admin.customers WHERE id=$1", [
          customer,
        ]),
      ).rejects.toThrow();
      await expect(
        owner.query(
          "INSERT INTO webdock_auth.billing_customer(environment,customer_id,is_test) VALUES('sandbox',$1,false)",
          [id(9)],
        ),
      ).rejects.toThrow();
    } finally {
      const cleanup = await owner.connect();
      try {
        await cleanup.query("BEGIN");
        await cleanup.query("SET LOCAL session_replication_role='replica'");
        await cleanup.query(
          "DELETE FROM webdock_auth.billing_contract WHERE customer_id=ANY($1)",
          [[customer, other]],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.billing_customer WHERE customer_id=ANY($1)",
          [[customer, other]],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.billing_product_mapping WHERE version_id=$1",
          [version],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.billing_product_version WHERE id=$1",
          [version],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.billing_product WHERE id=$1",
          [product],
        );
        await cleanup.query(
          "DELETE FROM webdock_auth.app_binding WHERE id=$1",
          [binding],
        );
        await cleanup.query(
          'DELETE FROM webdock_auth."oauthClient" WHERE id=$1',
          [client],
        );
        const orgs = (
          await cleanup.query(
            "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1) RETURNING organization_id",
            [[customer, other]],
          )
        ).rows.map((row) => row.organization_id);
        await cleanup.query(
          "DELETE FROM webdock_auth.organization WHERE id=ANY($1)",
          [orgs],
        );
        await cleanup.query(
          "DELETE FROM webdock_admin.projects WHERE id=ANY($1)",
          [[project, otherProject]],
        );
        await cleanup.query(
          "DELETE FROM webdock_admin.customers WHERE id=ANY($1)",
          [[customer, other]],
        );
        await cleanup.query("COMMIT");
      } catch (error) {
        await cleanup.query("ROLLBACK");
        throw error;
      } finally {
        cleanup.release();
      }
      await owner.end();
      await databasePools.close();
    }
  },
);
