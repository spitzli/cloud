import { configureFullInstallation } from "../src/full-installation";
configureFullInstallation();
import { test, expect } from "bun:test";
import { readConfig } from "../src/config";
import { createApp } from "../src/app";
import { legacyAuthProtocols } from "../src/protocols/legacy-auth";
import {
  configureInstallation,
  installationRegistry,
  publicInstallation,
  validateWorkerModules,
} from "../src/installation";
import {
  isMailOperation,
  isMailProvisioning,
} from "@webdock/module-mail/manifest";
const disabled = {
  WEBDOCK_MODULES: JSON.stringify({
    configVersion: 1,
    modules: { mail: { enabled: false } },
  }),
};
test("disabled Mail exposes no canonical, legacy, documentation or callback endpoints", async () => {
  const config = readConfig({ ...disabled, WEBDOCK_API_DOMAINS: "mail" });
  const mounts = await legacyAuthProtocols(config);
  expect(mounts).toEqual([]);
  const app = await createApp(config, { mounts });
  for (const host of [config.apiOrigin, config.authOrigin])
    for (const path of [
      "/api/mail/v1/emails",
      "/api/mail/v1/openapi.json",
      "/api/mail/gateway",
      "/api/mail/provider-events/123",
      "/mail/docs",
    ]) {
      const response = await app.handle(new Request(host + path));
      expect(response.status).toBe(404);
      expect((await response.json()).error.code).toBe("module_disabled");
    }
});
test("disabled Mail cannot be assigned to a worker, while draining keeps jobs running", () => {
  expect(() => validateWorkerModules(["mail"], disabled)).toThrow("disabled");
  expect(() => validateWorkerModules(["storage"], disabled)).toThrow(
    "disabled",
  );
  validateWorkerModules(["mail"], {
    WEBDOCK_MODULES: JSON.stringify({
      configVersion: 1,
      modules: { mail: { state: "draining" } },
    }),
  });
});
test("installation configuration fails closed and ownership restricts the public view", () => {
  expect(() => installationRegistry({ WEBDOCK_MODULES: "" })).toThrow();
  expect(
    publicInstallation({ WEBDOCK_API_DOMAINS: "auth,portal" }).modules.mail,
  ).toBe("disabled");
  expect(
    publicInstallation({ WEBDOCK_API_DOMAINS: "auth,portal,mail", WEBDOCK_MODULES: JSON.stringify({configVersion: 1, modules: {mail: {enabled: true}}}) }).modules
      .mail,
  ).toBe("enabled");
});
test("legacy and current Mail operations are covered without disabling authentication email", () => {
  for (const op of [
    "mailConnections",
    "manageTenantMail",
    "getTenantMailboxes",
    "getTenantSenderDomains",
    "getNativeSending",
    "launchTenantWebmail",
    "saveMailCredentials",
  ])
    expect(isMailOperation(op)).toBe(true);
  expect(isMailOperation("session")).toBe(false);
  expect(
    isMailProvisioning("manageTenantMail", ["123", { action: "provision" }]),
  ).toBe(true);
  // Native activation distinguishes new services from retained instances in the service itself.
  expect(
    isMailProvisioning("manageTenantMailService", [
      "123",
      { action: "activate" },
    ]),
  ).toBe(false);
  expect(
    isMailProvisioning("manageTenantMailService", [
      "123",
      { action: "suspend" },
    ]),
  ).toBe(false);
});
test("MCP omits disabled Mail tools and rejects calls from an already-created tool session", async () => {
  const { registerHostingTools } =
    await import("../src/modules/mcp/hosting-mcp");
  const previous = process.env.WEBDOCK_MODULES;
  try {
    process.env.WEBDOCK_MODULES = JSON.stringify({configVersion: 1, modules: {mail: {enabled: true}, hosting: {enabled: true}}});
    const callbacks = new Map<string, Function>();
    let called = false;
    const server = {
      registerTool: (name: string, _: unknown, callback: Function) =>
        callbacks.set(name, callback),
    };
    registerHostingTools(
      server as never,
      async () => {
        called = true;
        return {};
      },
      true,
    );
    const mailTool = [...callbacks].find(([name]) => name === "byok_mail_get");
    expect(mailTool).toBeDefined();
    process.env.WEBDOCK_MODULES = disabled.WEBDOCK_MODULES;
    expect((await mailTool![1]({ customerID: "123" })).isError).toBe(true);
    expect(called).toBe(false);
    callbacks.clear();
    registerHostingTools(server as never, async () => ({}), true);
    expect(
      [...callbacks.keys()].some((name) => name.startsWith("byok_mail_")),
    ).toBe(false);
  } finally {
    if (previous === undefined) delete process.env.WEBDOCK_MODULES;
    else process.env.WEBDOCK_MODULES = previous;
  }
});

test("all product defaults, omitted modules, unknown IDs and worker ownership are strict", () => {
  expect(
    Object.keys(installationRegistry({}).publicView().modules).sort(),
  ).toEqual(["databases", "hosting", "mail", "storage"]);
  expect(Object.values(installationRegistry({}).publicView().modules).every(state => state === "disabled")).toBe(true);
  const env = {
    WEBDOCK_MODULES: JSON.stringify({
      configVersion: 1,
      modules: { hosting: { state: "draining" } },
    }),
  };
  const registry = installationRegistry(env);
  expect(registry.state("hosting")).toBe("draining");
  expect(registry.state("databases")).toBe("disabled");
  registry.require("hosting");
  expect(() => registry.require("hosting", "provision")).toThrow("draining");
  expect(() => validateWorkerModules(["storage"], env)).toThrow("disabled");
  validateWorkerModules(["hosting"], env);
  expect(() =>
    installationRegistry({
      WEBDOCK_MODULES:
        '{"configVersion":1,"modules":{"unknown":{"enabled":true}}}',
    }),
  ).toThrow();
  expect(() => validateWorkerModules(["unknown"], {})).toThrow("disabled");
});

test("disabled products reject direct requests before any mounted handler", async () => {
  const config = readConfig({
    WEBDOCK_MODULES: '{"configVersion":1,"modules":{}}',
  });
  const app = await createApp(config);
  for (const path of [
    "/api/hosting/bridge",
    "/api/databases/gateway",
  ]) {
    const response = await app.handle(new Request(config.apiOrigin + path));
    expect(response.status).toBe(404);
    expect((await response.json()).error.code).toBe("module_disabled");
  }
});

test("Studio command classifiers distinguish databases and retain existing-resource actions during draining", async () => {
  const { moduleForOperation, isModuleProvisioning, moduleForPath } =
    await import("@webdock/module-sdk/selection");
  expect(
    moduleForOperation("hosting.command", [{ action: "databases.list" }]),
  ).toBe("databases");
  expect(moduleForOperation("registry.query", ["projects"])).toBeUndefined();
  expect(moduleForOperation("cms.content")).toBeUndefined();
  expect(moduleForOperation("getTenantStorage")).toBe("storage");
  for (const action of [
    "apps.start",
    "apps.reconcile",
    "apps.delete",
    "apps.purgeStorage",
    "databases.resume",
    "databases.purge",
    "setup.resume",
  ])
    expect(isModuleProvisioning("hosting.command", [{ action }])).toBe(false);
  for (const action of ["apps.create", "databases.create", "setup.confirm"])
    expect(isModuleProvisioning("hosting.command", [{ action }])).toBe(true);
  expect(moduleForPath("/hosting/databases/123")).toBe("databases");
});

test("database MCP tools are independent from customer Hosting and retain call-time gates", async () => {
  const { registerHostingTools } =
    await import("../src/modules/mcp/hosting-mcp");
  const previous = process.env.WEBDOCK_MODULES;
  const callbacks = new Map<string, Function>();
  let calls = 0;
  try {
    process.env.WEBDOCK_MODULES = JSON.stringify({
      configVersion: 1,
      modules: { databases: { enabled: true } },
    });
    registerHostingTools(
      {
        registerTool: (name: string, _: unknown, fn: Function) =>
          callbacks.set(name, fn),
      } as never,
      async () => {
        calls++;
        return {};
      },
      true,
    );
    expect(callbacks.has("databases_list")).toBe(true);
    expect(callbacks.has("list_hosting_apps")).toBe(false);
    process.env.WEBDOCK_MODULES = '{"configVersion":1,"modules":{}}';
    expect(
      (await callbacks.get("databases_list")!({ projectID: "123" })).isError,
    ).toBe(true);
    expect(calls).toBe(0);
  } finally {
    if (previous === undefined) delete process.env.WEBDOCK_MODULES;
    else process.env.WEBDOCK_MODULES = previous;
  }
});

test("removed billing configuration and worker ownership are rejected", () => {
 expect(() => installationRegistry({WEBDOCK_MODULES: JSON.stringify({configVersion: 1, modules: {billing: {enabled: true}}})})).toThrow();
 expect(() => validateWorkerModules(["billing"], {})).toThrow("disabled");
});

test("core installation metadata contains no product contributions", () => {
  configureInstallation([]);
  try {
    expect(publicInstallation({ WEBDOCK_API_DOMAINS: "auth,portal,registry" }).modules).toEqual({});
    expect(() => installationRegistry({ WEBDOCK_MODULES: JSON.stringify({ configVersion: 1, modules: { mail: { enabled: true } } }) })).toThrow();
  } finally {
    configureFullInstallation();
  }
});
