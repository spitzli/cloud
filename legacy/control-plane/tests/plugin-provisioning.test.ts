import { test, expect } from "bun:test";
import { createPluginAppProvisioning } from "../src/modules/hosting/plugin-provisioning";
const scope = {
  pluginID: "example",
  projectID: "10",
  customerID: "20",
  subject: "user",
  sessionID: "session",
  operationID: "op",
  pluginGeneration: 2,
  grantRevision: 3,
  consoleIdentity: {
    issuer: "issuer",
    userID: "clerk-user",
    sessionID: "clerk-session",
    organizationID: null,
  },
};
const template: any = {
  id: "approved",
  configuration: {
    source: {
      kind: "image",
      reference: "registry.example/app@sha256:" + "a".repeat(64),
    },
    spec: {
      args: [],
      port: 3000,
      healthPath: "/health",
      cpuMillicores: 100,
      memoryBytes: 67108864,
      ephemeralBytes: 67108864,
      replicas: 1,
      volumeBytes: 67108864,
    },
    database: {
      mode: "new",
      name: "app",
      planRevisionID: "1",
      variable: "DATABASE_URL",
    },
    mail: null,
    autoPublish: false,
  },
  environment: async () => [],
};
function fixture(environment = template.environment) {
  let record: any = null;
  const requests = new Map<string, string>();
  const effects: any[] = [];
  let live = true;
  const setup: any = {
    id: "500",
    projectID: "10",
    revision: 1,
    state: "draft",
    steps: [],
    resources: {},
    preview: { id: "preview", offers: [], blockers: [] },
  };
  const store: any = {
    status: async () => setup,
    list: async () => (record ? [record] : []),
    reserve: async (s: any, input: any) => {
      const hash = JSON.stringify(input);
      const key = input.request_id;
      if (requests.has(key) && requests.get(key) !== hash)
        throw Object.assign(new Error("conflict"), { code: "conflict" });
      requests.set(key, hash);
      return (record ??= {
        id: "100",
        pluginID: s.pluginID,
        projectID: s.projectID,
        customerID: s.customerID,
        templateID: input.template_id,
        name: input.name,
        actor: {
          subject: s.subject,
          sessionID: s.sessionID,
          consoleIdentity: s.consoleIdentity,
          source: "studio",
          scopes: ["hosting:read", "hosting:write"],
        },
        pluginGeneration: s.pluginGeneration,
        grantRevision: s.grantRevision,
        setupID: null,
        binding: null,
      });
    },
    get: async (s: any, id: string) =>
      record &&
      record.id === id &&
      s.pluginID === record.pluginID &&
      s.projectID === record.projectID
        ? record
        : null,
    attachSetup: async (_r: any, id: string) => {
      record.setupID = id;
    },
    assertLive: async () => {
      if (!live)
        throw Object.assign(new Error("denied"), { code: "permission_denied" });
    },
  };
  const service = createPluginAppProvisioning({
    templates: [{ ...template, environment }],
    store,
    executeHosting: async (actor: any, cmd: any) => {
      effects.push({ actor, cmd });
      if (cmd.action === "setup.save") return setup;
      if (cmd.action === "setup.preview")
        return { ...setup, state: "awaiting-confirmation" };
      if (cmd.action === "setup.confirm") {
        setup.state = "running";
        return setup;
      }
      return setup;
    },
  });
  return {
    service,
    effects,
    setup,
    record: () => record,
    revoke: () => {
      live = false;
    },
  };
}
const input = {
  operation: "provision" as const,
  input: { template_id: "approved", name: "App", request_id: "request-1" },
};
test("approved provisioning replays one durable setup and preserves actor identity", async () => {
  const f = fixture();
  const first: any = await f.service.handler(scope, input);
  const second: any = await f.service.handler(scope, input);
  const third: any = await f.service.handler(scope, {
    ...input,
    input: { ...input.input, request_id: "request-2" },
  });
  expect(first.provisioning_id).toBe(second.provisioning_id);
  expect(third.provisioning_id).toBe(first.provisioning_id);
  expect(f.effects.filter((e) => e.cmd.action === "setup.save")).toHaveLength(
    1,
  );
  expect(f.effects[0].actor.consoleIdentity).toEqual(scope.consoleIdentity);
  expect(JSON.stringify(first)).not.toContain("actor");
});
test("unapproved templates, changed replay and foreign status fail before side effects", async () => {
  const f = fixture();
  await expect(
    f.service.handler(scope, {
      ...input,
      input: { ...input.input, template_id: "other" },
    }),
  ).rejects.toMatchObject({ code: "permission_denied" });
  expect(f.effects).toHaveLength(0);
  await f.service.handler(scope, input);
  await expect(
    f.service.handler(scope, {
      ...input,
      input: { ...input.input, name: "Changed" },
    }),
  ).rejects.toMatchObject({ code: "conflict" });
  await expect(
    f.service.handler(
      { ...scope, projectID: "99" },
      { operation: "provisioningStatus", input: { provisioning_id: "100" } },
    ),
  ).rejects.toMatchObject({ code: "not_found" });
});
test("revocation blocks provisioning and status before Hosting is called", async () => {
  const f = fixture();
  f.revoke();
  await expect(f.service.handler(scope, input)).rejects.toMatchObject({
    code: "permission_denied",
  });
  expect(f.effects).toHaveLength(0);
});
test("template finalization cannot report ready without publication binding", async () => {
  const f = fixture();
  await f.service.handler(scope, input);
  f.record().binding = null;
  const result: any = await f.service.handler(scope, {
    operation: "provisioningStatus",
    input: { provisioning_id: "100" },
  });
  expect(result.state).not.toBe("ready");
  expect(result.origin).toBeUndefined();
});
test("discovery uses the bounded persisted projection without Hosting management authority", async () => {
  const f = fixture();
  await f.service.handler(scope, input);
  const before = f.effects.length;
  const result: any = await f.service.handler(scope, {
    operation: "provisioningStatus",
    input: { provisioning_id: "100" },
  });
  expect(result.provisioning_id).toBe("100");
  expect(f.effects.length).toBe(before);
});
test("explicit retry refreshes expired database admission before continuing", async () => {
  const f = fixture();
  await f.service.handler(scope, input);
  f.setup.state = "blocked";
  f.setup.configuration = template.configuration;
  f.setup.preview.expiresAt = new Date(0).toISOString();
  const before = f.effects.length;
  await f.service.handler(scope, input);
  expect(f.effects.slice(before).map((e) => e.cmd.action)).toEqual([
    "setup.get",
    "setup.preview",
    "setup.confirm",
  ]);
});

test("approved environment receives the immutable saved instance name", async () => {
  const names: string[] = [];
  const f = fixture(async (_scope: unknown, _id: string, name: string) => {
    names.push(name);
    return [{ name: "CMS_SITE_NAME", value: name }];
  });
  await f.service.handler(scope, {
    ...input,
    input: { ...input.input, name: "Own project CMS" },
  });
  expect(names).toEqual(["Own project CMS"]);
  expect(
    f.effects.find((effect) => effect.cmd.action === "setup.save").cmd
      .environment,
  ).toEqual([{ name: "CMS_SITE_NAME", value: "Own project CMS" }]);
});
