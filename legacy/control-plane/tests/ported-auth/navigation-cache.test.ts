import { configureFullProducts } from "../../src/full-products";
configureFullProducts();
import test, { afterEach } from "node:test";
import { mockMethod, restoreMocks } from "../helpers/node-test";
import assert from "node:assert/strict";
import { randomBytes } from "node:crypto";
import { readFile } from "node:fs/promises";
import pg from "pg";
import { serializeSignedCookie } from "better-call";
import { assertTestDatabase } from "../helpers/database";
import { auth } from "../../src/modules/auth/auth";
import { database, closeDatabases } from "../../src/db/runtime";
import { listTenants } from "../../src/modules/access/tenants";
import { ViewCache, configureViewCache } from "../../src/cache";
afterEach(restoreMocks);

assertTestDatabase(process.env.DATABASE_URL!);
test.after(closeDatabases);

test("navigation caches only tenant summaries and invalidates membership, status and identity changes", async (t) => {
  await database.query(
    await readFile(
      new URL("../../drizzle/0036_authorized_view_cache.sql", import.meta.url),
      "utf8",
    ),
  );
  const base = BigInt(
    "92" + BigInt("0x" + randomBytes(4).toString("hex")).toString(),
  );
  const [subject, sessionID, otherSubject, otherSession, firstID, secondID] = [
    0, 1, 2, 3, 4, 5,
  ].map((n) => String(base + BigInt(n)));
  const store = new Map<string, string>();
  let reads = 0;
  configureViewCache(
    new ViewCache({
      get: async (key) => {
        reads++;
        return store.get(key) ?? null;
      },
      set: async (key, value) => {
        store.set(key, value);
      },
    }),
    { environment: "navigation-test" },
  );
  t.after(() => configureViewCache(undefined, { environment: "test" }));
  t.after(async () => {
    await database.query(
      "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1",
      [sessionID],
    );
    const orgs = (
      await database.query(
        "DELETE FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1) RETURNING organization_id",
        [[firstID, secondID]],
      )
    ).rows.map((row) => row.organization_id);
    await database.query(
      "DELETE FROM webdock_admin.customers WHERE id=ANY($1)",
      [[firstID, secondID]],
    );
    await database.query(
      "DELETE FROM webdock_auth.organization WHERE id=ANY($1)",
      [orgs],
    );
    await database.query('DELETE FROM webdock_auth."user" WHERE id=ANY($1)', [
      [subject, otherSubject],
    ]);
  });
  for (const [id, sid] of [
    [subject, sessionID],
    [otherSubject, otherSession],
  ]) {
    await database.query(
      'INSERT INTO webdock_auth."user"(id,name,email,role,"emailVerified","mustChangePassword") VALUES($1,\'Cache member\',$2,\'user\',true,false)',
      [id, id + "@example.invalid"],
    );
    await database.query(
      'INSERT INTO webdock_auth.session(id,token,"userId","expiresAt","updatedAt") VALUES($1,$1,$2,now()+interval \'1 hour\',now())',
      [sid, id],
    );
  }
  await database.query(
    "INSERT INTO webdock_admin.customers(id,name,status,notes) VALUES($1,'First tenant','active','secret profile note'),($2,'Second tenant','active','another private note')",
    [firstID, secondID],
  );
  const mappings = (
    await database.query(
      "SELECT customer_id,organization_id FROM webdock_auth.tenant_customer WHERE customer_id=ANY($1)",
      [[firstID, secondID]],
    )
  ).rows;
  const org = (id: string) =>
    mappings.find((row) => row.customer_id === id).organization_id;
  await database.query(
    'INSERT INTO webdock_auth.member("organizationId","userId",role,"createdAt") VALUES($1,$2,\'admin\',now()),($3,$4,\'member\',now())',
    [org(firstID), subject, org(secondID), otherSubject],
  );
  const context = await auth.$context;
  const headers = async (sid: string) =>
    new Headers({
      cookie: (
        await serializeSignedCookie(
          context.authCookies.sessionToken.name,
          sid,
          context.secret,
        )
      ).split(";")[0],
    });
  const ownHeaders = await headers(sessionID),
    otherHeaders = await headers(otherSession);
  const statements: string[] = [];
  const original = pg.Client.prototype.query;
  const tracked = mockMethod(
    pg.Client.prototype,
    "query",
    function (this: pg.Client, ...args: any[]) {
      statements.push(typeof args[0] === "string" ? args[0] : args[0].text);
      return Reflect.apply(original, this, args);
    },
  );
  const first = await listTenants(ownHeaders);
  assert.deepEqual(
    first.map((row) => row.id),
    [firstID],
  );
  statements.length = 0;
  assert.deepEqual(await listTenants(ownHeaders), first);
  assert.equal(
    statements.some((sql) =>
      sql.includes("SELECT c.id,c.name,c.status,m.role"),
    ),
    false,
    "warm navigation skips tenant enumeration",
  );
  assert(store.size > 0);
  assert.deepEqual(Object.keys(first[0]).sort(), [
    "id",
    "name",
    "role",
    "status",
  ]);
  assert.equal(
    JSON.stringify([...store.values()]).includes("private note"),
    false,
  );
  assert.deepEqual(
    (await listTenants(otherHeaders)).map((row) => row.id),
    [secondID],
  );
  await database.query(
    "UPDATE webdock_admin.customers SET name='Renamed tenant' WHERE id=$1",
    [firstID],
  );
  assert.equal((await listTenants(ownHeaders))[0].name, "Renamed tenant");
  await database.query(
    "UPDATE webdock_auth.member SET role='member' WHERE \"userId\"=$1",
    [subject],
  );
  assert.equal((await listTenants(ownHeaders))[0].role, "member");
  await database.query(
    "UPDATE webdock_admin.customers SET status='archived' WHERE id=$1",
    [firstID],
  );
  assert.deepEqual(await listTenants(ownHeaders), []);
  await database.query(
    "UPDATE webdock_admin.customers SET status='active' WHERE id=$1",
    [firstID],
  );
  assert.equal((await listTenants(ownHeaders)).length, 1);
  await database.query('DELETE FROM webdock_auth.member WHERE "userId"=$1', [
    subject,
  ]);
  assert.deepEqual(await listTenants(ownHeaders), []);
  // A preview must bypass the already warm operator directory for this same session.
  await database.query(
    `UPDATE webdock_auth."user" SET role='operator',"twoFactorEnabled"=true WHERE id=$1`,
    [subject],
  );
  const { handleNativeStudioRequest } =
    await import("../../src/modules/portal/studio-api");
  const call = () =>
    handleNativeStudioRequest(
      new Request(process.env.BETTER_AUTH_URL + "/api/auth/studio", {
        method: "POST",
        headers: new Headers([
          ...ownHeaders,
          ["origin", process.env.BETTER_AUTH_URL!],
          ["content-type", "application/json"],
        ]),
        body: JSON.stringify({ operation: "listTenants", args: [] }),
      }),
    );
  const operatorResponse = await call();
  assert.equal(operatorResponse.status, 200);
  assert(
    (await operatorResponse.json()).data.some(
      (row: { id: string }) => row.id === secondID,
    ),
  );
  const beforePreview = reads;
  await database.query(
    "INSERT INTO webdock_auth.studio_tenant_preview(session_id,actor_id,customer_id,organization_id,expires_at) VALUES($1,$2,$3,$4,now()+interval '1 hour')",
    [sessionID, subject, firstID, org(firstID)],
  );
  const preview = await call();
  assert.equal(preview.status, 200);
  assert.deepEqual(
    (await preview.json()).data.map((row: { id: string }) => row.id),
    [firstID],
  );
  assert.equal(
    reads,
    beforePreview,
    "preview cannot read the operator directory cache",
  );
  await database.query(
    "DELETE FROM webdock_auth.studio_tenant_preview WHERE session_id=$1",
    [sessionID],
  );
  const before = reads;
  await database.query(
    'UPDATE webdock_auth."user" SET banned=true WHERE id=$1',
    [subject],
  );
  await assert.rejects(listTenants(ownHeaders));
  assert.equal(
    reads,
    before,
    "revoked identity cannot consult cached tenant data",
  );
  await database.query(
    'UPDATE webdock_auth."user" SET banned=false WHERE id=$1',
    [subject],
  );
  await database.query("DELETE FROM webdock_auth.session WHERE id=$1", [
    sessionID,
  ]);
  await assert.rejects(listTenants(ownHeaders));
  assert.equal(reads, before);
  tracked.mock.restore();
});
