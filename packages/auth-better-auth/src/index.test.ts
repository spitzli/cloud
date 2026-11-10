import { test } from "node:test";
import assert from "node:assert/strict";
import { betterAuthIdentity, createBetterAuthAuthenticator } from "./index.ts";
import type { DatabaseReader } from "@spitzli/core/identity";
function reader(rows: unknown[][]) {
  const calls: { sql: string; values: unknown }[] = [];
  const db = {
    query: async (sql: string, values: unknown) => {
      calls.push({ sql, values });
      return { rows: rows.shift() ?? [], rowCount: rows.length };
    },
  } as unknown as DatabaseReader;
  return { db, calls };
}
test("header authentication returns local references only, forwards the actual headers and never caches", async () => {
  let active = true,
    calls = 0;
  const headers = new Headers({ cookie: "private" });
  const adapter = createBetterAuthAuthenticator(async (received) => {
    assert.equal(received, headers);
    calls++;
    return active
      ? {
          user: { id: "123", email: "private@example.invalid" },
          session: { id: "s123", token: "private-token" },
        }
      : null;
  });
  assert.deepEqual(await adapter.authenticate(headers), {
    subject: "123",
    sessionID: "s123",
  });
  active = false;
  assert.equal(await adapter.authenticate(headers), null);
  assert.equal(calls, 2);
});
test("session directory applies expiry in SQL, preserves native locks and reads preview independently", async () => {
  const { db, calls } = reader([
    [
      {
        id: "123",
        role: "operator",
        banned: false,
        emailVerified: true,
        mustChangePassword: false,
        twoFactorEnabled: true,
      },
    ],
    [{ session_id: "s123" }],
  ]);
  const identity = await betterAuthIdentity.session(
    db,
    { subject: "123", sessionID: "s123" },
    { lock: true },
  );
  assert.equal(identity?.preview, true);
  assert.equal(identity?.twoFactorEnabled, true);
  assert.match(calls[0].sql, /s\."expiresAt">now\(\)/);
  assert.match(calls[0].sql, /FOR SHARE OF u,s/);
  assert.deepEqual(calls[0].values, ["123", "s123"]);
  assert.equal(
    await betterAuthIdentity.session(reader([[]]).db, {
      subject: "123",
      sessionID: "expired",
    }),
    null,
  );
});
test("missing tenant mappings differ from revoked membership; identity flags are strict booleans", async () => {
  assert.equal(
    await betterAuthIdentity.membership(reader([[]]).db, "123", "456"),
    null,
  );
  assert.deepEqual(
    await betterAuthIdentity.membership(
      reader([[{ organization_id: "org" }], []]).db,
      "123",
      "456",
    ),
    { memberID: null, role: null },
  );
  const { db, calls } = reader([
    [{ organization_id: "org" }],
    [{ id: "m123", role: "admin" }],
  ]);
  assert.deepEqual(
    await betterAuthIdentity.membership(db, "123", "456", { lock: true }),
    { memberID: "m123", role: "admin" },
  );
  assert.match(calls[1].sql, /FOR SHARE/);
  const principal = await betterAuthIdentity.subject(
    reader([
      [
        {
          id: "123",
          role: "operator",
          twoFactorEnabled: "true",
          emailVerified: "true",
        },
      ],
    ]).db,
    "123",
  );
  assert.equal(principal?.twoFactorEnabled, false);
  assert.equal(principal?.emailVerified, false);
});
