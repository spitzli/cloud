import { test, expect } from "bun:test";
import { createApprovedTemplateExecution } from "../src/modules/hosting/plugin-provisioning-runtime";
const record: any = {
  id: "1",
  pluginID: "plugin",
  customerID: "2",
  projectID: "3",
  templateID: "app",
  name: "App",
  pluginGeneration: 1,
  grantRevision: 1,
  actor: { subject: "u", sessionID: "s" },
  binding: null,
};
const row: any = {
  id: "4",
  project_id: "3",
  customer_id: "2",
  actor: record.actor,
  configuration: { name: "App" },
  resources: { appID: "5" },
};
test("revoked grant stops template hooks and unconfigured templates fail closed", async () => {
  let calls = 0;
  let live = false;
  const deps: any = {
    templates: [{ id: "app", configuration: {} }],
    hooks: {
      beforeRelease: async () => {
        calls++;
        return { ready: true };
      },
      publish: async () => {
        calls++;
        return {
          appID: "5",
          websiteBindingID: "site",
          origin: "https://site.example",
          secretReference: "ref",
          pinnedAddresses: ["1.1.1.1"],
        };
      },
    },
    store: {
      bySetup: async () => record,
      assertLive: async () => {
        if (!live) throw Error("revoked");
      },
      setBinding: async () => {},
    },
  };
  const execution = createApprovedTemplateExecution(deps);
  await expect(execution.beforeRelease(row)).rejects.toThrow("revoked");
  expect(calls).toBe(0);
  live = true;
  expect(await execution.beforeRelease(row)).toBe(true);
  await expect(
    createApprovedTemplateExecution({ ...deps, templates: [] }).beforeRelease(
      row,
    ),
  ).rejects.toThrow();
});
test("publication validates the exact app before storing ready binding", async () => {
  let saved: any = null;
  const binding = {
    appID: "99",
    websiteBindingID: "site",
    origin: "https://site.example",
    secretReference: "ref",
    pinnedAddresses: ["1.1.1.1"],
  };
  const execution = createApprovedTemplateExecution({
    templates: [{ id: "app", configuration: {} }] as any,
    hooks: {
      beforeRelease: async () => ({ ready: true }),
      publish: async () => binding,
    },
    store: {
      bySetup: async () => record,
      assertLive: async () => {},
      setBinding: async (_r: any, b: any) => {
        saved = b;
      },
    },
  } as any);
  await expect(execution.beforeReady(row)).rejects.toThrow();
  expect(saved).toBeNull();
  binding.appID = "5";
  expect(await execution.beforeReady(row)).toBe(true);
  expect(saved).toEqual(binding);
});
test("a saved setup cannot substitute an unapproved image or sizing", async () => {
  let called = false;
  const changed = {
    ...row,
    configuration: {
      name: "App",
      source: { kind: "image", reference: "evil" },
    },
  };
  const execution = createApprovedTemplateExecution({
    templates: [
      {
        id: "app",
        configuration: { source: { kind: "image", reference: "approved" } },
      },
    ] as any,
    hooks: {
      beforeRelease: async () => {
        called = true;
        return { ready: true };
      },
      publish: async () => null,
    },
    store: {
      bySetup: async () => ({ ...record, name: "App" }),
      assertLive: async () => {},
      setBinding: async () => {},
    },
  });
  await expect(execution.beforeRelease(changed)).rejects.toThrow();
  expect(called).toBe(false);
});
