import test from "node:test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { Pool } from "pg";
import { inspectManagedConnection } from "../../src/modules/lifecycle/project-deletion-database";
const url = new URL(process.env.DATABASE_URL!);
if (
  !["localhost", "127.0.0.1"].includes(url.hostname) ||
  url.pathname !== "/webdock_api_test"
)
  throw Error("Disposable local database required");
const pool = new Pool({ connectionString: url.href });
test.after(() => pool.end());
test("real PostgreSQL deletion rejects cross-schema dependencies, sharing and elevated roles", async () => {
  const schema = "demo_lifecycle_test_" + randomBytes(5).toString("hex"),
    role = schema + "_runtime",
    external = schema + "_external";
  const c = await pool.connect();
  try {
    await c.query(`CREATE ROLE "${role}" NOLOGIN`);
    await c.query(`CREATE ROLE "${external}" NOLOGIN`);
    // Provisioning grants the runtime role TO the database owner for maintenance.
    // This must not be confused with granting a more powerful role TO the runtime.
    const owner = (
      await c.query(
        "SELECT pg_get_userbyid(datdba) AS name FROM pg_database WHERE datname=current_database()",
      )
    ).rows[0].name as string;
    await c.query(`GRANT "${role}" TO "${owner.replaceAll('"', '""')}"`);
    await c.query(`CREATE SCHEMA "${schema}" AUTHORIZATION "${role}"`);
    await c.query(`CREATE SCHEMA "${external}"`);
    await c.query(`SET ROLE "${role}"`);
    await c.query(`CREATE TABLE "${schema}".items(id integer PRIMARY KEY)`);
    await c.query("RESET ROLE");
    await c.query(`GRANT "${role}" TO "${external}"`);
    await c.query(`SET ROLE "${role}"`);
    await assert.rejects(
      inspectManagedConnection(c, schema, role),
      /privileges beyond/,
    );
    await c.query("RESET ROLE");
    await c.query(`REVOKE "${role}" FROM "${external}"`);
    await c.query(`GRANT "${external}" TO "${role}"`);
    await c.query(`SET ROLE "${role}"`);
    await assert.rejects(
      inspectManagedConnection(c, schema, role),
      /privileges beyond/,
    );
    await c.query("RESET ROLE");
    await c.query(`REVOKE "${external}" FROM "${role}"`);
    await c.query(
      `CREATE VIEW "${external}".dependent AS SELECT * FROM "${schema}".items`,
    );
    await c.query(`SET ROLE "${role}"`);
    await assert.rejects(
      inspectManagedConnection(c, schema, role, true),
      /Another application depends/,
    );
    await c.query("RESET ROLE");
    assert.equal(
      (
        await c.query("SELECT to_regclass($1) AS object", [
          `${external}.dependent`,
        ])
      ).rows[0].object,
      `${external}.dependent`,
    );
    await c.query(`DROP VIEW "${external}".dependent`);
    await c.query(
      `CREATE TABLE "${external}".referencing(id integer REFERENCES "${schema}".items(id))`,
    );
    await c.query(`SET ROLE "${role}"`);
    await assert.rejects(
      inspectManagedConnection(c, schema, role, true),
      /Another application depends/,
    );
    await c.query("RESET ROLE");
    await c.query(`DROP TABLE "${external}".referencing`);
    await c.query(`SET ROLE "${role}"`);
    await c.query(
      `CREATE FUNCTION "${schema}".trigger_fn() RETURNS trigger LANGUAGE plpgsql AS 'BEGIN RETURN NEW; END'`,
    );
    await c.query("RESET ROLE");
    await c.query(`CREATE TABLE "${external}".events(id integer)`);
    await c.query(
      `CREATE TRIGGER cross_schema BEFORE INSERT ON "${external}".events FOR EACH ROW EXECUTE FUNCTION "${schema}".trigger_fn()`,
    );
    await c.query(`SET ROLE "${role}"`);
    await assert.rejects(
      inspectManagedConnection(c, schema, role, true),
      /Another application depends/,
    );
    await c.query("RESET ROLE");
    await c.query(`DROP TABLE "${external}".events`);
    await c.query(`GRANT USAGE ON SCHEMA "${schema}" TO PUBLIC`);
    await c.query(`SET ROLE "${role}"`);
    await assert.rejects(
      inspectManagedConnection(c, schema, role, true),
      /accessible to another role/,
    );
    await c.query("RESET ROLE");
    await c.query(`REVOKE USAGE ON SCHEMA "${schema}" FROM PUBLIC`);
    await c.query(`ALTER ROLE "${role}" CREATEDB`);
    await c.query(`SET ROLE "${role}"`);
    await assert.rejects(
      inspectManagedConnection(c, schema, role, true),
      /privileges beyond/,
    );
    await c.query("RESET ROLE");
    await c.query(`ALTER ROLE "${role}" NOCREATEDB`);
    await c.query(`SET ROLE "${role}"`);
    assert.equal(
      (await inspectManagedConnection(c, schema, role)).alreadyAbsent,
      false,
    );
    assert.equal(
      (await inspectManagedConnection(c, schema, role, true)).alreadyAbsent,
      false,
    );
    assert.equal(
      (await inspectManagedConnection(c, schema, role, true, true))
        .alreadyAbsent,
      true,
    );
    await c.query("RESET ROLE");
    assert.equal(
      (await c.query("SELECT to_regnamespace($1) AS object", [external]))
        .rows[0].object,
      external,
    );
  } finally {
    await c.query("ROLLBACK");
    await c.query("RESET ROLE");
    await c.query(`DROP SCHEMA IF EXISTS "${schema}" CASCADE`);
    await c.query(`DROP SCHEMA IF EXISTS "${external}" CASCADE`);
    await c.query(`DROP ROLE IF EXISTS "${role}"`);
    await c.query(`DROP ROLE IF EXISTS "${external}"`);
    c.release();
  }
});
