import assert from "node:assert/strict";
import test from "node:test";
import { Pool } from "pg";
import {
  tenantSchemaSQL,
  tenantProfileGrants,
} from "../../src/modules/access/tenant-schema";

test("customer creation provisions one tenant atomically and profile grants exclude operator data", async () => {
  const url = new URL(process.env.DATABASE_URL!);
  assert.ok(["localhost", "127.0.0.1"].includes(url.hostname));
  assert.equal(url.pathname, "/webdock_api_test");
  const pool = new Pool({ connectionString: url.toString(), max: 1 });
  const client = await pool.connect();
  const runtime = `tenant_profile_test_${Date.now()}`;
  try {
    await client.query("BEGIN");
    await client.query(tenantSchemaSQL);
    await client.query(`CREATE ROLE "${runtime}" NOLOGIN`);
    await client.query(tenantProfileGrants(runtime));
    const id = (
      await client.query("SELECT webdock_auth.next_snowflake() AS id")
    ).rows[0].id;
    await client.query(
      "INSERT INTO webdock_admin.customers(id,name,status,notes) VALUES($1,'Tenant fixture','active','private operator notes')",
      [id],
    );
    const mapped = async () =>
      (
        await client.query(
          "SELECT t.organization_id,o.name FROM webdock_auth.tenant_customer t JOIN webdock_auth.organization o ON o.id=t.organization_id WHERE t.customer_id=$1",
          [id],
        )
      ).rows;
    const first = await mapped();
    assert.equal(first.length, 1);
    assert.equal(first[0].name, "Tenant fixture");
    await client.query(
      "UPDATE webdock_admin.customers SET name='Renamed tenant' WHERE id=$1",
      [id],
    );
    const second = await mapped();
    assert.equal(second[0].organization_id, first[0].organization_id);
    assert.equal(second[0].name, "Renamed tenant");
    const privileges = (
      await client.query(
        `SELECT
      has_column_privilege($1,'webdock_admin.customers','notes','SELECT') AS notes,
      has_column_privilege($1,'webdock_admin.customers','status','UPDATE') AS archive,
      has_column_privilege($1,'webdock_admin.customers','contact_email','SELECT') AS read,
      has_column_privilege($1,'webdock_admin.customers','address_line1','UPDATE') AS edit`,
        [runtime],
      )
    ).rows[0];
    assert.deepEqual(privileges, {
      notes: false,
      archive: false,
      read: true,
      edit: true,
    });
    await client.query("SAVEPOINT rejected_customer");
    const rollbackID = (
      await client.query("SELECT webdock_auth.next_snowflake() AS id")
    ).rows[0].id;
    await client.query(
      "INSERT INTO webdock_admin.customers(id,name,status) VALUES($1,'Rolled back tenant','active')",
      [rollbackID],
    );
    await client.query("ROLLBACK TO SAVEPOINT rejected_customer");
    assert.equal(
      (
        await client.query(
          "SELECT 1 FROM webdock_auth.tenant_customer WHERE customer_id=$1",
          [rollbackID],
        )
      ).rowCount,
      0,
    );
    assert.equal(
      (
        await client.query(
          "SELECT 1 FROM webdock_auth.organization WHERE slug=$1",
          ["customer-" + rollbackID],
        )
      ).rowCount,
      0,
    );
    assert.throws(() => tenantProfileGrants('invalid"role'));
  } finally {
    await client.query("ROLLBACK");
    client.release();
    await pool.end();
  }
});
