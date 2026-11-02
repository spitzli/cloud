import { test, expect } from "bun:test";
import { verifyRole, verifySchema } from "../src/db/readiness";
const role = {
  name: "webdock_auth_runtime",
  session: "webdock_auth_runtime",
  superuser: false,
  bypass: false,
  owner: false,
};
test("readiness rejects owner, bypass, switched or wrong runtime roles", async () => {
  for (const patch of [
    { superuser: true },
    { bypass: true },
    { owner: true },
    { name: "postgres" },
    { session: "postgres" },
  ])
    await expect(
      verifyRole(
        { query: async () => ({ rows: [{ ...role, ...patch }] }) },
        "webdock_auth_runtime",
      ),
    ).rejects.toThrow();
  await verifyRole(
    { query: async () => ({ rows: [role] }) },
    "webdock_auth_runtime",
  );
});
test("readiness requires every declared column and forced row isolation", async () => {
  const table = {
    schema: "webdock_auth",
    name: "session",
    rls: true,
    forced: true,
    columns: ["id", "token"],
  };
  const expected = [
    { schema: "webdock_auth", name: "session", columns: ["id", "token"] },
  ];
  await verifySchema({ query: async () => ({ rows: [table] }) }, expected);
  for (const patch of [{ rls: false }, { forced: false }, { columns: ["id"] }])
    await expect(
      verifySchema(
        { query: async () => ({ rows: [{ ...table, ...patch }] }) },
        expected,
      ),
    ).rejects.toThrow();
  await expect(
    verifySchema({ query: async () => ({ rows: [] }) }, expected),
  ).rejects.toThrow();
});
