import { test, expect } from "bun:test";
import * as core from "../../../packages/core/src/plugin-apps";
import * as adapter from "../src/modules/hosting/plugin-apps";
import { PluginError } from "@webdock/plugin-sdk/contracts";
const createHandler = (
  execute: Parameters<typeof adapter.createManagedAppsHandler>[0],
) => adapter.createManagedAppsHandler(execute, () => {});
const scope = {
  pluginID: "managed-apps",
  projectID: "10",
  customerID: "20",
  operationID: "a".repeat(64),
  subject: "user",
  sessionID: "session",
};
const bindings = [
  {
    appID: "123",
    kind: "tabularium" as const,
    origin: "https://registry.example",
  },
];
const app = {
  id: "123",
  projectID: "10",
  name: "Registry",
  status: "running",
  revision: 2,
  observedRevision: 2,
  subscriptionRevision: 3,
  operationID: null,
  environmentNames: ["PASSWORD"],
  logs: "secret",
  spec: { image: "secret" },
};
const request = new Request("https://host.example/v1/plugin/apps");
test("optional service is unavailable; denied authority never reaches Hosting", async () => {
  expect(typeof core.createPluginApps).toBe("function");
  let called = false;
  const service = core.createPluginApps({
    run: async () => {
      throw new PluginError("permission_denied", "Denied");
    },
    handler: () => async () => {
      called = true;
      return app;
    },
  });
  await expect(
    service.request(request, { operation: "list", input: {} }),
  ).rejects.toMatchObject({ code: "permission_denied" });
  expect(called).toBe(false);
});
test("strict request and project bindings deny writes before side effects", async () => {
  let calls = 0;
  const service = core.createPluginApps({
    run: (_req, _cap, fn) => fn(scope, bindings),
    handler: () => async () => {
      calls++;
      return app;
    },
  });
  for (const input of [
    { operation: "get", input: { app_id: "999" } },
    {
      operation: "command",
      input: {
        app_id: "123",
        operation: "delete",
        revision: 2,
        subscription_revision: 3,
      },
    },
    {
      operation: "command",
      input: {
        app_id: "123",
        operation: "stop",
        revision: 2,
        subscription_revision: 3,
        projectID: "9",
      },
    },
  ])
    await expect(service.request(request, input)).rejects.toBeDefined();
  expect(calls).toBe(0);
});
test("Hosting projection redacts secrets and checks project before mutations", async () => {
  const commands: any[] = [];
  const handler = createHandler(async (actor: any, command: any) => {
    commands.push({ actor, command });
    return command.action === "apps.get"
      ? app
      : { id: "456", appID: "123", status: "queued", credentials: "secret" };
  });
  const service = core.createPluginApps({
    run: (_req, _cap, fn) => fn(scope, bindings),
    handler: () => handler,
  });
  const result: any = await service.request(request, {
    operation: "list",
    input: {},
  });
  expect(result.apps[0]).toEqual({
    app_id: "123",
    name: "Registry",
    kind: "tabularium",
    status: "running",
    revision: 2,
    observed_revision: 2,
    subscription_revision: 3,
    operation_id: null,
    origin: "https://registry.example",
    console_path: "/hosting/apps/123",
  });
  expect(JSON.stringify(result)).not.toContain("secret");
  const command = {
    operation: "command",
    input: {
      app_id: "123",
      operation: "stop",
      revision: 2,
      subscription_revision: 3,
    },
  };
  expect(await service.request(request, command)).toEqual({
    operation_id: "456",
    app_id: "123",
    status: "queued",
  });
  expect(commands.at(-1).command).toEqual({
    action: "apps.stop",
    appID: "123",
    revision: 2,
    subscriptionRevision: 3,
    idempotencyKey: scope.operationID,
  });
  expect(commands.at(-1).actor).toMatchObject({
    subject: "user",
    sessionID: "session",
    source: "studio",
  });
  app.projectID = "99";
  const before = commands.length;
  await expect(service.request(request, command)).rejects.toMatchObject({
    code: "permission_denied",
  });
  expect(
    commands.slice(before).every((c) => c.command.action === "apps.get"),
  ).toBe(true);
  app.projectID = "10";
});
test("unsafe operator origins fail before invoking Hosting and capabilities separate reads from commands", async () => {
  let called = false;
  const caps: string[] = [];
  const service = core.createPluginApps({
    run: (_req, cap, fn) => {
      caps.push(cap);
      return fn(scope, [{ ...bindings[0], origin: "javascript:alert(1)" }]);
    },
    handler: () => async () => {
      called = true;
      return app;
    },
  });
  await expect(
    service.request(request, { operation: "get", input: { app_id: "123" } }),
  ).rejects.toBeDefined();
  await expect(
    service.request(request, {
      operation: "command",
      input: {
        app_id: "123",
        operation: "start",
        revision: 2,
        subscription_revision: 3,
      },
    }),
  ).rejects.toBeDefined();
  expect(caps).toEqual(["plugin.apps.read", "plugin.apps.manage"]);
  expect(called).toBe(false);
});
test("native Hosting conflicts and denial remain generic, with revisions forwarded unchanged", async () => {
  const { HostingError } = await import("@webdock/hosting-contracts");
  for (const [status, code] of [
    [403, "permission_denied"],
    [409, "conflict"],
  ] as const) {
    const handler = createHandler(async (_actor: any, command: any) => {
      if (command.action === "apps.get") return app;
      expect(command.revision).toBe(0);
      throw new HostingError(status, "private provider secret");
    });
    await expect(
      handler(scope, bindings[0], {
        operation: "command",
        input: {
          app_id: "123",
          operation: "restart",
          revision: 0,
          subscription_revision: 0,
        },
      }),
    ).rejects.toMatchObject({ code });
    try {
      await handler(scope, bindings[0], {
        operation: "command",
        input: {
          app_id: "123",
          operation: "restart",
          revision: 0,
          subscription_revision: 0,
        },
      });
    } catch (e) {
      expect(String(e)).not.toContain("secret");
    }
  }
});
test("offline RLS recognizes apps.read for read invocations, never apps.manage", async () => {
  const { pluginScopeSQL } = await import("../src/modules/plugins/scope-rls");
  const readGate = pluginScopeSQL
    .split("capability IN (")[1]
    .split(") OR v.intent='write'")[0];
  expect(readGate).toContain("'plugin.apps.read'");
  expect(readGate).not.toContain("'plugin.apps.manage'");
});
test("core-only service fails closed and unbound lists reveal nothing", async () => {
  const missing = core.createPluginApps({
    run: (_req, _cap, fn) => fn(scope, bindings),
    handler: () => undefined,
  });
  await expect(
    missing.request(request, { operation: "list", input: {} }),
  ).rejects.toMatchObject({ code: "capability_unavailable" });
  let calls = 0;
  const empty = core.createPluginApps({
    run: (_req, _cap, fn) => fn(scope, []),
    handler: () => async () => {
      calls++;
      return app;
    },
  });
  expect(
    await empty.request(request, { operation: "list", input: {} }),
  ).toEqual({ apps: [] });
  expect(calls).toBe(0);
});
test("a mismatched or extra-field adapter response never escapes the host", async () => {
  for (const output of [
    {
      app_id: "123",
      name: "Registry",
      kind: "tabularium",
      status: "running",
      revision: 2,
      observed_revision: 2,
      subscription_revision: 3,
      operation_id: null,
      origin: "https://evil.example",
      console_path: "/hosting/apps/123",
    },
    {
      app_id: "123",
      name: "Registry",
      kind: "tabularium",
      status: "running",
      revision: 2,
      observed_revision: 2,
      subscription_revision: 3,
      operation_id: null,
      origin: "https://registry.example",
      console_path: "/hosting/apps/123",
      logs: "private",
    },
  ]) {
    const service = core.createPluginApps({
      run: (_req, _cap, fn) => fn(scope, bindings),
      handler: () => async () => output,
    });
    await expect(
      service.request(request, { operation: "get", input: { app_id: "123" } }),
    ).rejects.toMatchObject({ code: "unavailable" });
  }
});
test("disabled Hosting denies before reading or operating an app", async () => {
  let called = false;
  const handler = adapter.createManagedAppsHandler(
    async () => {
      called = true;
      return app;
    },
    () => {
      throw new Error("Hosting disabled");
    },
  );
  await expect(
    handler(scope, bindings[0], { operation: "get", input: { app_id: "123" } }),
  ).rejects.toMatchObject({ code: "unavailable" });
  expect(called).toBe(false);
});
test("provisioning uses separate optional adapter and exact capability with bounded results", async () => {
  const caps: string[] = [];
  const status = {
    provisioning_id: "777",
    template_id: "approved-app",
    state: "running" as const,
    steps: [],
  };
  const service = core.createPluginApps({
    run: (_r, cap, fn) => {
      caps.push(cap);
      return fn(scope, []);
    },
    handler: () => undefined,
    provisioningHandler: () => async () => status,
  });
  expect(
    await service.request(request, {
      operation: "provision",
      input: { template_id: "approved-app", name: "App", request_id: "req" },
    }),
  ).toEqual(status);
  expect(
    await service.request(request, {
      operation: "provisioningStatus",
      input: { provisioning_id: "777" },
    }),
  ).toEqual(status);
  expect(caps).toEqual(["plugin.apps.provision", "plugin.apps.read"]);
  const missing = core.createPluginApps({
    run: (_r, _cap, fn) => fn(scope, []),
    handler: () => undefined,
  });
  await expect(
    missing.request(request, {
      operation: "provision",
      input: { template_id: "approved-app", name: "App", request_id: "req" },
    }),
  ).rejects.toMatchObject({ code: "capability_unavailable" });
});
