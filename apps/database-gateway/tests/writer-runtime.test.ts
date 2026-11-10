import test from "node:test";
import assert from "node:assert/strict";

test("separate writer runtime can edit rows but cannot change schema or access auth data", { skip: process.env.SPITZLI_DATABASE_RUNTIME_TEST !== "true" }, async () => {
  const launch = await fetch("http://localhost:3136/api/databases/bridge", { method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ accessToken: "bob-fixture-token", command: { action: "open", bindingID: "10000" } }) }).then(r => r.json());
  const response = await fetch("http://localhost:3137/session", { method: "POST", headers: { Origin: "http://localhost:3130", "Content-Type": "application/json" }, body: JSON.stringify({ code: launch.data.code }) });
  assert.equal(response.status, 200); const session = await response.json();
  assert.match(session.baseUrl, /^http:\/\/localhost:3137\/s\/[0-9]+$/);
  const headers = { Origin: "http://localhost:3130", Cookie: response.headers.get("set-cookie")!.split(";")[0] };
  const negotiation = await fetch(session.baseUrl+"/api/v1/session", { headers }).then(r => r.json());
  const call = async (command: string, args: unknown) => {
    const response = await fetch(session.baseUrl+"/api/v1/rpc/"+command, { method: "POST", headers: { ...headers, "Content-Type": "application/json", "X-Tabularis-Csrf": negotiation.csrfToken }, body: JSON.stringify(args) });
    return { status: response.status, body: await response.json() };
  };
  const target = { connectionId: session.connectionID, table: "orders", schema: "browser_demo" };
  try {
    const manifest = await call("get_driver_manifest", { driverId: "postgres" });
    assert.equal(manifest.body.data.capabilities.readonly, false);
    assert.equal(manifest.body.data.capabilities.manage_tables, false);
    assert.equal((await call("insert_record", { ...target, data: { id: 90001, customer: "Writer fixture", total: 4.5, status: "pending" } })).status, 200);
    assert.equal((await call("update_record", { ...target, pkMap: { id: 90001 }, colName: "status", newVal: "paid" })).status, 200);
    const rows = await call("execute_query", { connectionId: session.connectionID, query: "SELECT status FROM browser_demo.orders WHERE id=90001", limit: 10 });
    assert.equal(rows.body.data.rows[0][0], "paid");
    for (const query of ["CREATE TABLE browser_demo.not_allowed(id int)", "SELECT * FROM webdock_auth.session"]) {
      const denied = await call("execute_query", { connectionId: session.connectionID, query });
      assert.equal(denied.body.ok, false); assert.match(denied.body.error.message, /permission denied/i);
    }
  } finally {
    await call("delete_record", { ...target, pkMap: { id: 90001 } });
    await fetch(session.baseUrl+"/api/v1/logout", { method: "POST", headers: { ...headers, "X-Tabularis-Csrf": negotiation.csrfToken } });
  }
});
