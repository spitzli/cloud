import test, { afterEach } from "node:test";
import assert from "node:assert/strict";
import { launchDatabaseWorkspace } from "../src/lib/database-launch";

const launch = { code: "x".repeat(43), gatewayOrigin: "https://database.webdock.dev" };
const unavailable = { error: "Database access is unavailable." };
// Each test swaps fetch for a fixture; the real one comes back afterwards.
const realFetch = globalThis.fetch;
afterEach(() => { globalThis.fetch = realFetch; });
function mockFetch(handler: (input: unknown, init?: RequestInit) => Promise<Response>) {
  globalThis.fetch = handler as typeof fetch;
}

for (const managed of [false, true]) {
  test(`browser launches a ${managed ? "managed" : "registered"} database through the same-origin portal`, async () => {
    const calls: { input: unknown; init?: RequestInit }[] = [];
    mockFetch(async (input: unknown, init?: RequestInit) => {
      calls.push({ input, init });
      return Response.json({ data: { ...launch, binding: { id: "123" }, accessToken: "must-not-propagate" } });
    });
    assert.deepEqual(await launchDatabaseWorkspace("123", managed), { launch });
    assert.equal(calls.length, 1);
    assert.equal(calls[0].input, "/api/portal/studio");
    const { signal, ...request } = calls[0].init!;
    assert.ok(signal instanceof AbortSignal);
    assert.equal(signal.aborted, false);
    assert.deepEqual(request, {
      method: "POST", credentials: "same-origin", redirect: "error", cache: "no-store",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ operation: "database.command", args: [managed ? { action: "openManaged", databaseID: "123" } : { action: "open", bindingID: "123" }] }),
    });
  });
}

test("launch preserves reviewed domain errors while hiding transport and unknown errors", async () => {
  mockFetch(async () => Response.json({ error: { kind: "HostingError", message: "Database access has been disabled." } }, { status: 403 }));
  assert.deepEqual(await launchDatabaseWorkspace("123"), { error: "Database access has been disabled." });
  for (const response of [
    Response.json({ error: { kind: "Error", message: "private database credentials" } }, { status: 500 }),
    Response.json({ error: { kind: "PortalSessionMissing", message: "private token" } }, { status: 401 }),
    Response.json({ data: launch }, { status: 403 }),
    Response.json({ data: launch, error: { kind: "Error", message: "private detail" } }),
    new Response("<html>private proxy diagnostic</html>", { status: 502 }),
  ]) {
    mockFetch(async () => response);
    assert.deepEqual(await launchDatabaseWorkspace("123"), unavailable);
  }
  mockFetch(async () => { throw Error("Network error with private details"); });
  assert.deepEqual(await launchDatabaseWorkspace("123"), unavailable);
  mockFetch(async () => { throw new DOMException("Private request details", "TimeoutError"); });
  assert.deepEqual(await launchDatabaseWorkspace("123"), unavailable);
});

test("launch rejects malformed launch codes and unsafe gateway destinations", async () => {
  for (const data of [
    null, {}, [], { ...launch, code: "short" }, { ...launch, code: "!".repeat(43) },
    ...["javascript:alert(1)", "http://outside.example", "https://user:password@database.webdock.dev", "https://database.webdock.dev/path", "https://database.webdock.dev?secret=token", "https://database.webdock.dev#fragment"].map(gatewayOrigin => ({ ...launch, gatewayOrigin })),
  ]) {
    mockFetch(async () => Response.json({ data }));
    assert.deepEqual(await launchDatabaseWorkspace("123"), unavailable);
  }
  for (const gatewayOrigin of ["http://localhost:55442", "http://127.0.0.1:55442", "http://[::1]:55442"]) {
    mockFetch(async () => Response.json({ data: { ...launch, gatewayOrigin } }));
    assert.deepEqual(await launchDatabaseWorkspace("123"), { launch: { ...launch, gatewayOrigin } });
  }
});
