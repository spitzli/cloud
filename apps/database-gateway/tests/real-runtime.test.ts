import test from "node:test";
import assert from "node:assert/strict";

test("real Tabularis gateway reads PostgreSQL and denies writes and cross-schema access", { skip: process.env.SPITZLI_DATABASE_RUNTIME_TEST !== "true" }, async () => {
  const launch = await fetch("http://localhost:3136/api/databases/bridge", { method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ accessToken: "alice-fixture-token", command: { action: "open", bindingID: "10000" } }) }).then(r => r.json());
  const response = await fetch("http://localhost:3137/session", { method: "POST", headers: { Origin: "http://localhost:3130", "Content-Type": "application/json" }, body: JSON.stringify({ code: launch.data.code }) });
  assert.equal(response.status, 200);
  const session = await response.json();
  assert.match(session.baseUrl, /^http:\/\/localhost:3137\/s\/[0-9]+$/);
  const cookie = response.headers.get("set-cookie")!.split(";")[0];
  const headers = { Origin: "http://localhost:3130", Cookie: cookie };
  const negotiation = await fetch(session.baseUrl+"/api/v1/session", { headers }).then(r => r.json());
  const call = async (command: string, args: unknown) => {
    const response = await fetch(session.baseUrl+"/api/v1/rpc/"+command, { method: "POST", headers: { ...headers, "Content-Type": "application/json", "X-Tabularis-Csrf": negotiation.csrfToken }, body: JSON.stringify(args) });
    return { status: response.status, body: await response.json() };
  };
  assert.equal((await call("test_connection", { request: { connection_id: session.connectionID, params: { host: "attacker.invalid" } } })).status, 200);
  const read = await call("execute_query", { connectionId: session.connectionID, query: "SELECT id,customer,total,status FROM browser_demo.orders ORDER BY id", limit: 10 });
  assert.equal(read.status, 200); assert.equal(read.body.data.rows.length, 3);
  for (const query of ["DELETE FROM browser_demo.orders", "CREATE TABLE browser_demo.forbidden(id int)", "SET ROLE postgres", "SELECT * FROM webdock_auth.session", "WITH changed AS (DELETE FROM browser_demo.orders RETURNING *) SELECT * FROM changed"]) {
    const denied = await call("execute_query", { connectionId: session.connectionID, query, limit: 10 });
    assert.equal(denied.body.ok, false); assert.match(denied.body.error.message, /permission denied/i);
  }
  await fetch(session.baseUrl+"/api/v1/logout", { method: "POST", headers: { ...headers, "X-Tabularis-Csrf": negotiation.csrfToken } });
});
