import assert from "node:assert/strict";
import test from "node:test";
import { randomUUID } from "node:crypto";
import {
  assertMailTestDatabase,
  withMailFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { mailProductSchemaSQL } =
  await import("../../src/modules/mail/product/schema");
test.after(() => database.end());
test("additive schema reruns and rejects cross-tenant connections, domains and keys", async () => {
  await database.query(mailProductSchemaSQL);
  await database.query(mailProductSchemaSQL);
  await withMailFixture(async (f) => {
    const ids: string[] = [];
    for (const customer of f.customers)
      ids.push(
        (
          await f.db.query(
            "INSERT INTO webdock_auth.mail_connection(customer_id,kind,provider_account_id) VALUES($1,'platform',$2) RETURNING id",
            [customer, randomUUID()],
          )
        ).rows[0].id,
      );
    try {
      const domain = (
        await f.db.query(
          "INSERT INTO webdock_auth.mail_sender_domain(customer_id,domain,token) VALUES($1,$2,$3) RETURNING id",
          [f.customers[0], f.suffix + ".example.invalid", randomUUID()],
        )
      ).rows[0].id;
      const sql =
        "INSERT INTO webdock_auth.mail_message(customer_id,connection_id,domain_id,from_address,subject,dispatch_status,scheduled_at,provider_reference) VALUES($1,$2,$3,'sender@example.invalid','test','scheduled',now()+interval '1 hour',$4)";
      await assert.rejects(
        f.db.query(sql, [f.customers[0], ids[1], domain, randomUUID()]),
        { code: "23503" },
      );
      await assert.rejects(
        f.db.query(sql, [f.customers[1], ids[1], domain, randomUUID()]),
        { code: "23503" },
      );
      await f.db.query(sql, [f.customers[0], ids[0], domain, randomUUID()]);
      await assert.rejects(
        f.db.query(
          "INSERT INTO webdock_auth.mail_api_key(customer_id,connection_id,name,prefix,key_hash,permission) VALUES($1,$2,'test','wdm_foo',$3,'full_access')",
          [f.customers[0], ids[1], randomUUID()],
        ),
        { code: "23503" },
      );
      await assert.rejects(
        f.db.query(
          "UPDATE webdock_auth.mail_connection SET provider_account_id=(SELECT provider_account_id FROM webdock_auth.mail_connection WHERE id=$1) WHERE id=$2",
          ids,
        ),
        { code: "23505" },
      );
    } finally {
      await f.db.query(
        "DELETE FROM webdock_auth.mail_message WHERE customer_id=ANY($1)",
        [f.customers],
      );
      await f.db.query(
        "DELETE FROM webdock_auth.mail_connection WHERE customer_id=ANY($1)",
        [f.customers],
      );
      await f.db.query(
        "DELETE FROM webdock_auth.mail_sender_domain WHERE customer_id=ANY($1)",
        [f.customers],
      );
    }
  });
});

test("offline migration is checksummed, repeatable and grants DML without DDL", async () => {
  const { execFileSync } = await import("node:child_process");
  const role = "mail_fixture_runtime";
  await database.query(
    `DO $$ BEGIN IF NOT EXISTS(SELECT 1 FROM pg_roles WHERE rolname='${role}') THEN CREATE ROLE ${role} NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE; END IF; END $$; GRANT USAGE ON SCHEMA webdock_auth TO ${role}`,
  );
  const url = new URL(process.env.DATABASE_URL!);
  const target = `${url.hostname}:${url.port || "5432"}${url.pathname}`;
  const run = () =>
    JSON.parse(
      execFileSync(
        process.execPath,
        [
          "--env-file=.env.test.local",
          "scripts/migrate-mail-product.ts",
          target,
          role,
        ],
        {
          cwd: process.cwd(),
          encoding: "utf8",
          env: { ...process.env, DATABASE_URL_UNPOOLED: url.href },
        },
      ),
    );
  const first = run(),
    second = run();
  assert.equal(first.checksum, second.checksum);
  assert.equal(second.applied, false);
  const db = await database.connect();
  try {
    await db.query(`SET ROLE ${role}`);
    await db.query("SELECT id FROM webdock_auth.mail_message LIMIT 1");
    await assert.rejects(
      db.query("CREATE TABLE webdock_auth.mail_fixture_forbidden(id int)"),
      { code: "42501" },
    );
    await assert.rejects(
      db.query(
        "ALTER TABLE webdock_auth.mail_message ADD COLUMN forbidden text",
      ),
      { code: "42501" },
    );
  } finally {
    await db.query("RESET ROLE");
    db.release();
  }
});
