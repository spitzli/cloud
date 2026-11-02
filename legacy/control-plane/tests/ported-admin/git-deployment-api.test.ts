import test from "node:test";
import assert from "node:assert/strict";
import { handleHostingRequest } from "../../src/modules/mcp/hosting-api";

test("Git deployment REST commands use the authorized hosting service", async () => {
  const calls: unknown[] = [];
  const deps = {
    origin: "https://studio.example",
    call: async (_token: string, cmd: unknown) => {
      calls.push(cmd);
      return { docs: [] };
    },
  };
  const request = (command: unknown) =>
    handleHostingRequest(
      new Request("https://studio.example/api/hosting/commands", {
        method: "POST",
        headers: {
          Authorization: "Bearer fixture",
          "Content-Type": "application/json",
          Origin: deps.origin,
        },
        body: JSON.stringify(command),
      }),
      ["commands"],
      deps,
    );
  assert.equal(
    (await request({ action: "git.source.get", projectID: "123" })).status,
    200,
  );
  assert.deepEqual(calls, [{ action: "git.source.get", projectID: "123" }]);
  assert.equal(
    (
      await request({
        action: "git.source.get",
        projectID: "123",
        actor: { role: "operator" },
      })
    ).status,
    400,
  );
  assert.equal(
    (
      await request({
        action: "git.connection.finish",
        code: "secret",
        state: "secret",
      })
    ).status,
    400,
  );
  assert.equal(calls.length, 1);
});
