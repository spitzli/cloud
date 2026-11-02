import test from "node:test";
import assert from "node:assert/strict";
import { handleHostingRequest } from "../../src/modules/mcp/hosting-api";
import { HostingError, type HostingCommand } from "@webdock/hosting-contracts";
test("hosting REST strictly maps routes and keeps operator/membership enforcement in the shared service", async () => {
  const commands: HostingCommand[] = [];
  const deps = {
    origin: "http://localhost:3120",
    call: async (token: string, command: HostingCommand) => {
      commands.push(command);
      if (token === "customer" && command.action === "limits.set")
        throw new HostingError(403, "Platform operator access is required.");
      return { ok: true };
    },
  };
  const call = (
    path: string[],
    method = "GET",
    token = "customer",
    body?: unknown,
    origin?: string,
  ) =>
    handleHostingRequest(
      new Request("http://localhost:3120/api/hosting/" + path.join("/"), {
        method,
        headers: {
          Authorization: "Bearer " + token,
          "Content-Type": "application/json",
          ...(origin ? { Origin: origin } : {}),
        },
        ...(body ? { body: JSON.stringify(body) } : {}),
      }),
      path,
      deps,
    );
  assert.equal((await call(["customers", "123", "limits"])).status, 200);
  assert.deepEqual(commands[0], { action: "limits.get", customerID: "123" });
  assert.equal(
    (
      await call(["customers", "123", "limits"], "PUT", "customer", {
        values: { apps: 2 },
        revision: 0,
        subscriptionRevision: 1,
      })
    ).status,
    403,
  );
  assert.equal(
    (
      await call(["clusters"], "POST", "operator", {
        name: "x",
        actor: { role: "operator" },
      })
    ).status,
    400,
  );
  assert.equal(
    (
      await call(
        ["clusters"],
        "GET",
        "customer",
        undefined,
        "https://evil.invalid",
      )
    ).status,
    403,
  );
  assert.equal(
    (await call(["customers", "123", "limits"], "GET", "")).status,
    401,
  );
  assert.equal((await call(["clusters", "123", "unexpected"])).status, 404);
  assert.equal(
    (await call(["apps", "123", "storage-deletion"], "GET", "operator")).status,
    200,
  );
  assert.deepEqual(commands.at(-1), {
    action: "apps.storageDeletion",
    appID: "123",
  });
  assert.equal(
    (
      await call(["apps", "123", "storage"], "DELETE", "operator", {
        confirmName: "Fixture",
        planHash: "a".repeat(64),
        idempotencyKey: "storage-purge-fixture",
      })
    ).status,
    200,
  );
  assert.equal(commands.at(-1)?.action, "apps.purgeStorage");
  assert.equal(
    (await call(["customers", "123", "limits"])).headers.get("cache-control"),
    "no-store",
  );
});

test("hosting REST preserves bounded-body validation status", async () => {
  const deps = {
    origin: "http://localhost:3120",
    call: async () => {
      throw Error("must not dispatch");
    },
  };
  for (const [contentType, body, status] of [
    ["text/plain", "{}", 415],
    ["application/json", "{", 400],
    ["application/json", JSON.stringify({ name: "x".repeat(70000) }), 413],
  ] as const) {
    const result = await handleHostingRequest(
      new Request("http://localhost:3120/api/hosting/clusters", {
        method: "POST",
        headers: {
          Authorization: "Bearer fixture",
          "Content-Type": contentType,
        },
        body,
      }),
      ["clusters"],
      deps,
    );
    assert.equal(result.status, status);
  }
});
