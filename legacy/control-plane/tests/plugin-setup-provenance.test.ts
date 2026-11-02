import { test, expect } from "bun:test";
import { createHostingProjectSetup } from "../../../modules/hosting/src/server/project-setup";
test("setup save refuses an unattached or foreign provisioning owner in the same transaction", async () => {
  const db: any = {
    query: async (sql: string) => {
      if (sql.includes("next_snowflake")) return { rows: [{ id: "100" }] };
      if (sql.includes("INSERT INTO webdock_auth.hosting_setup"))
        return {
          rows: [
            {
              id: "100",
              project_id: "10",
              configuration: {},
              revision: 1,
              state: "draft",
              resources: {},
              steps: [],
            },
          ],
        };
      return { rows: [], rowCount: 0 };
    },
  };
  const service = createHostingProjectSetup({
    transaction: async (run: any) => run(db),
    authorizeHosting: async () => ({ customerID: "20" }),
    replay: async () => ({ hash: "hash" }),
    authorizeSetupConfiguration: async () => {},
    sealEnvironment: () => null,
    patchEnvironment: () => ({}),
    openEnvironment: () => ({}),
    recordResult: async () => {},
    audit: async () => {},
    requireDatabases: () => {},
    requireMail: () => {},
    env: { WEBDOCK_SETUP_ENABLED: "true" },
  } as any);
  const actor: any = {
    subject: "u",
    sessionID: "s",
    source: "studio",
    scopes: ["hosting:read", "hosting:write"],
    pluginProvisioningID: "99",
  };
  const config = {
    name: "App",
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
    },
    database: { mode: "none" },
    mail: null,
    autoPublish: false,
  };
  await expect(
    service.executeProjectSetup(actor, {
      action: "setup.save",
      projectID: "10",
      revision: 0,
      configuration: config,
      idempotencyKey: "provisioning-save-99",
    }),
  ).rejects.toMatchObject({ status: 409 });
});
