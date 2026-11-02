import test from "node:test";
import assert from "node:assert/strict";
import {
  assertMailTestDatabase,
  withMailFixture,
} from "./helpers/mail-product";
assertMailTestDatabase(process.env.DATABASE_URL!);
const { database } = await import("../../src/db/runtime");
const { tenantOwnerSchemaSQL } =
  await import("../../src/modules/access/tenant-owner-schema");
await database.query(tenantOwnerSchemaSQL);
const { auth } = await import("../../src/modules/auth/auth");
const { manageTenant } = await import("../../src/modules/access/tenants");
test.after(async () => {
  await auth.$context;
  await database.end();
});
test("owner-governed roles reject escalation and serialize the final owner", async () =>
  withMailFixture(async (f) => {
    const customer = f.customers[0],
      org = f.organizations[0];
    const memberID = async (user: string) =>
      (
        await f.db.query(
          'SELECT id FROM webdock_auth.member WHERE "organizationId"=$1 AND "userId"=$2',
          [org, user],
        )
      ).rows[0].id;
    const admin = await memberID(f.admin.id),
      member = await memberID(f.member.id);
    await assert.rejects(
      manageTenant(f.admin.headers, customer, {
        action: "role",
        member: admin,
        role: "owner",
      }),
    );
    await manageTenant(f.operator.headers, customer, {
      action: "role",
      member: admin,
      role: "owner",
    });
    await manageTenant(f.admin.headers, customer, {
      action: "role",
      member,
      role: "admin",
    });
    await assert.rejects(
      manageTenant(f.member.headers, customer, {
        action: "remove",
        member: admin,
      }),
    );
    await assert.rejects(
      manageTenant(f.member.headers, customer, {
        action: "role",
        member,
        role: "owner",
      }),
    );
    await assert.rejects(
      manageTenant(f.admin.headers, customer, {
        action: "role",
        member: admin,
        role: "admin",
      }),
      /owner/i,
    );
    await manageTenant(f.admin.headers, customer, {
      action: "role",
      member,
      role: "owner",
    });
    const results = await Promise.allSettled([
      manageTenant(f.admin.headers, customer, {
        action: "role",
        member: admin,
        role: "admin",
      }),
      manageTenant(f.member.headers, customer, {
        action: "role",
        member,
        role: "admin",
      }),
    ]);
    assert.equal(results.filter((r) => r.status === "fulfilled").length, 1);
    assert.equal(
      (
        await f.db.query(
          `SELECT count(*)::int AS n FROM webdock_auth.member WHERE "organizationId"=$1 AND role='owner'`,
          [org],
        )
      ).rows[0].n,
      1,
    );
  }));

test("concurrent owner ban and demotion cannot leave the tenant ownerless", async () =>
  withMailFixture(async (f) => {
    const org = f.organizations[0];
    await f.db.query(
      `UPDATE webdock_auth.member SET role='owner' WHERE "organizationId"=$1`,
      [org],
    );
    const results = await Promise.allSettled([
      f.db.query('UPDATE webdock_auth."user" SET banned=true WHERE id=$1', [
        f.admin.id,
      ]),
      f.db.query(
        `UPDATE webdock_auth.member SET role='admin' WHERE "organizationId"=$1 AND "userId"=$2`,
        [org, f.member.id],
      ),
    ]);
    assert.equal(results.filter((r) => r.status === "fulfilled").length, 1);
    const owners = (
      await f.db.query(
        `SELECT u.id FROM webdock_auth.member m JOIN webdock_auth."user" u ON u.id=m."userId" WHERE m."organizationId"=$1 AND m.role='owner' AND NOT coalesce(u.banned,false) AND u."emailVerified" AND NOT u."mustChangePassword"`,
        [org],
      )
    ).rows;
    assert.equal(owners.length, 1);
    await assert.rejects(
      f.db.query(
        'UPDATE webdock_auth."user" SET "emailVerified"=false WHERE id=$1',
        [owners[0].id],
      ),
      /owner/,
    );
  }));

test("promotion waits for an in-flight account ban and rechecks eligibility", async () =>
  withMailFixture(async (f) => {
    const org = f.organizations[0],
      customer = f.customers[0];
    const rows = (
      await f.db.query(
        'SELECT id,"userId" FROM webdock_auth.member WHERE "organizationId"=$1',
        [org],
      )
    ).rows;
    const target = rows.find((r) => r.userId === f.member.id).id;
    const connection = await f.db.connect();
    try {
      await connection.query("BEGIN");
      await connection.query(
        'UPDATE webdock_auth."user" SET banned=true WHERE id=$1',
        [f.member.id],
      );
      const promotion = manageTenant(f.operator.headers, customer, {
        action: "role",
        member: target,
        role: "owner",
      });
      const rejection = assert.rejects(promotion, /active|verified/);
      await connection.query("COMMIT");
      await rejection;
      assert.equal(
        (
          await f.db.query("SELECT role FROM webdock_auth.member WHERE id=$1", [
            target,
          ])
        ).rows[0].role,
        "member",
      );
    } finally {
      await connection.query("ROLLBACK");
      connection.release();
    }
  }));
async function waitForOrganizationLock() {
  for (let attempt = 0; attempt < 100; attempt++) {
    const blocked = await database.query(
      `SELECT 1 FROM pg_stat_activity WHERE datname=current_database() AND wait_event_type='Lock' AND query=$1`,
      ["SELECT id FROM webdock_auth.organization WHERE id=$1 FOR UPDATE"],
    );
    if (blocked.rowCount) return;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
  throw Error("Expected waiting tenant mutation");
}
test("operator authority is rechecked after waiting for the tenant lock", async () =>
  withMailFixture(async (f) => {
    const connection = await f.db.connect(),
      org = f.organizations[0];
    const target = (
      await f.db.query(
        'SELECT id FROM webdock_auth.member WHERE "organizationId"=$1 AND "userId"=$2',
        [org, f.member.id],
      )
    ).rows[0].id;
    let result: Promise<void> | undefined;
    try {
      await connection.query("BEGIN");
      await connection.query(
        "SELECT id FROM webdock_auth.organization WHERE id=$1 FOR UPDATE",
        [org],
      );
      result = assert.rejects(
        manageTenant(f.operator.headers, f.customers[0], {
          action: "role",
          member: target,
          role: "owner",
        }),
        /account setup/,
      );
      await waitForOrganizationLock();
      await f.db.query(
        'UPDATE webdock_auth."user" SET "twoFactorEnabled"=false WHERE id=$1',
        [f.operator.id],
      );
      await connection.query("COMMIT");
      await result;
      assert.equal(
        (
          await f.db.query("SELECT role FROM webdock_auth.member WHERE id=$1", [
            target,
          ])
        ).rows[0].role,
        "member",
      );
    } finally {
      await connection.query("ROLLBACK");
      connection.release();
      await result;
    }
  }));
test("owner guard executes with the restricted Auth runtime grant shape", async () =>
  withMailFixture(async (f) => {
    await f.db.query(
      `UPDATE webdock_auth.member SET role='owner' WHERE "organizationId"=$1 AND "userId"=$2`,
      [f.organizations[0], f.admin.id],
    );
    const db = await f.db.connect(),
      role = "mail_owner_smoke_" + f.suffix;
    try {
      await db.query("BEGIN");
      await db.query(
        `CREATE ROLE ${role} NOLOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE`,
      );
      await db.query(
        `GRANT USAGE ON SCHEMA webdock_auth TO ${role}; GRANT SELECT,UPDATE ON webdock_auth."user",webdock_auth.member,webdock_auth.organization TO ${role}; GRANT SELECT ON webdock_auth.tenant_customer TO ${role}`,
      );
      await db.query(`SET LOCAL ROLE ${role}`);
      assert.equal(
        (
          await db.query(
            'UPDATE webdock_auth."user" SET banned=true WHERE id=$1',
            [f.member.id],
          )
        ).rowCount,
        1,
      );
      await assert.rejects(
        db.query('UPDATE webdock_auth."user" SET banned=true WHERE id=$1', [
          f.admin.id,
        ]),
        { code: "23514" },
      );
    } finally {
      await db.query("ROLLBACK");
      db.release();
    }
  }));
