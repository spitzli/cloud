import assert from "node:assert/strict";
import test from "node:test";
import { assertMailTestDatabase } from "./helpers/mail-product";

test("mail tests refuse remote, ambiguous, production or non-disposable databases", () => {
  const safe = { AUTH_TEST_MAIL: "true", NODE_ENV: "test" };
  for (const url of [
    "postgresql://test:test@db.example.com/webdock_api_test",
    "postgresql://test:test@127.0.0.1:5438/webdock",
    "postgresql://test:test@localhost/webdock_api_test?host=db.example.com",
    "postgresql://test:test@localhost/webdock_api_test?options=-csearch_path=production",
    "https://localhost/webdock_api_test",
  ])
    assert.throws(() => assertMailTestDatabase(url, safe));
  const local = "postgresql://test:test@127.0.0.1:5438/webdock_api_test";
  assert.doesNotThrow(() => assertMailTestDatabase(local, safe));
  for (const env of [
    {},
    { ...safe, NODE_ENV: "production" },
    { ...safe, VERCEL_ENV: "production" },
    { ...safe, VERCEL_ENV: "preview" },
  ]) {
    assert.throws(() => assertMailTestDatabase(local, env));
  }
});

test("fixtures sign in real users, isolate tenants and remove only their own rows", async () => {
  const { withMailFixture } = await import("./helpers/mail-product");
  const { database } = await import("../../src/db/runtime");
  const { getTenant } = await import("../../src/modules/access/tenants");
  let ids: string[] = [];
  try {
    await withMailFixture(async (f) => {
      ids = f.customers;
      assert.equal((await getTenant(f.adminHeaders, ids[0])).canManage, true);
      assert.equal((await getTenant(f.memberHeaders, ids[0])).canManage, false);
      await assert.rejects(getTenant(f.adminHeaders, ids[1]));
      assert.equal((await getTenant(f.operatorHeaders, ids[1])).operator, true);
    });
    assert.equal(
      (
        await database.query(
          "SELECT id FROM webdock_admin.customers WHERE id=ANY($1)",
          [ids],
        )
      ).rowCount,
      0,
    );
  } finally {
    await database.end();
  }
});
