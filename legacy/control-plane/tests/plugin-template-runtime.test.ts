import { test, expect } from "bun:test";
import { managedCMSTemplate } from "../src/modules/hosting/plugin-template-runtime";
const approval = {
  image: "ghcr.io/example/cms@sha256:" + "a".repeat(64),
  domain: "cms.example.com",
  addresses: ["192.0.2.1"],
  certificateIssuer: "cms-acme",
  ingressClass: "traefik",
  planRevisionID: "123",
  authIssuer: "https://auth.example.com/api/auth",
  authAddresses: ["192.0.2.2"],
};
test("operator template pins image/database and retains single replica media", () => {
  const { templates } = managedCMSTemplate(approval);
  const config = templates[0].configuration;
  expect(config.source).toEqual({ kind: "image", reference: approval.image });
  expect(config.database).toEqual({
    mode: "new",
    name: "CMS",
    planRevisionID: "123",
    variable: "DATABASE_URL",
  });
  expect(config.spec.replicas).toBe(1);
  expect(config.spec.volumeBytes).toBeGreaterThan(0);
  expect(config.spec.args).toEqual([]);
});
test("operator approval rejects mutable images, credential URLs and extra commands", () => {
  for (const changed of [
    { image: "nginx:latest" },
    { authIssuer: "https://user:secret@auth.example.com/api/auth" },
    { domain: "https://cms.example.com" },
    { command: ["sh"] },
    { addresses: ["cms.example.com"] },
  ])
    expect(() => managedCMSTemplate({ ...approval, ...changed })).toThrow();
});

for (const deniedAt of ["actor", "grant"] as const)
  test(`an oldest template with revoked ${deniedAt} is blocked and the next valid operation is claimed`, async () => {
    const { createTemplateAgentRequest } =
      await import("../src/modules/hosting/plugin-template-runtime");
    const blocked: string[] = [];
    const actorChecks: string[] = [];
    const row = (id: string) => ({
      id,
      template_id: "managed-cms",
      project_id: "20",
      cluster_id: "30",
      namespace: "wd-20",
      resources: { appID: "40", databaseID: "60" },
      app_revision: 2,
      configuration: { spec: { replicas: 1 } },
      actor: { subject: "user" },
      template_state: {
        instanceID: "50",
        origin: "https://50.cms.example.com",
        approval,
        secrets: "sealed",
        migration: { phase: "migration", status: "succeeded", generation: 1 },
        publication: { phase: "publication", status: "queued", generation: 0 },
      },
    });
    const db = {
      query: async (sql: string, values: any[] = []) => {
        if (sql.startsWith("SELECT p.*"))
          return { rows: [row("10"), row("11")] };
        if (sql.includes("plugin_provisioning_live"))
          return {
            rows: [{ allowed: deniedAt !== "grant" || values[0] !== "10" }],
          };
        if (sql.includes("block_template_operation")) {
          blocked.push(values[0]);
          return { rows: [{ blocked: true }] };
        }
        if (sql.includes("hosting_database d JOIN"))
          return {
            rows: [
              {
                id: "60",
                backend_id: "70",
                state: "ready",
                backend_state: "ready",
                endpoint_ready: true,
                credential_revision: 1,
                confirmed_credential_revision: 1,
              },
            ],
          };
        return { rows: [] };
      },
    } as any;
    const handler = createTemplateAgentRequest({
      transaction: async (run: any) => run(db),
      verifyAgent: async () => ({}) as any,
      assertProvisioningActor: async (id: string) => {
        actorChecks.push(id);
        if (id === "10") throw new Error("permission revoked");
      },
      environment: async () => ({ CMS_INSTANCE_ID: "50" }),
    });
    const claimed: any = await handler(
      { clusterID: "30", generation: 1, credentialHash: "unused" },
      "template-claim",
      {},
    );
    expect(blocked).toEqual(["10"]);
    expect(actorChecks).toEqual(deniedAt === "actor" ? ["10", "11"] : ["11"]);
    expect(claimed.operationID).toBe("11");
    expect(claimed.leaseToken).toHaveLength(43);
  });

for (const scenario of ["revoked", "storage-reset", "storage-ready"] as const)
  test(`checkpoint ${scenario} preserves authorization and allocation evidence`, async () => {
    const { createTemplateAgentRequest } =
      await import("../src/modules/hosting/plugin-template-runtime");
    const writes: any[] = [];
    const op = {
      phase: "migration",
      status: "running",
      generation: 7,
      leaseToken: "a".repeat(43),
      leaseUntil: new Date(Date.now() + 60_000).toISOString(),
      storageState: scenario === "storage-ready" ? "allocating" : "ready",
    };
    const db = {
      query: async (sql: string, values: any[] = []) => {
        if (sql.startsWith("SELECT p.*"))
          return {
            rows: [
              { id: "10", project_id: "20", template_state: { migration: op } },
            ],
          };
        if (sql.includes("plugin_provisioning_live"))
          return { rows: [{ allowed: true }] };
        if (sql.startsWith("UPDATE")) writes.push(values);
        return { rows: [] };
      },
    } as any;
    const handler = createTemplateAgentRequest({
      transaction: async (run: any) => run(db),
      verifyAgent: async () => ({}) as any,
      assertProvisioningActor: async () => {
        if (scenario === "revoked") throw new Error("revoked");
      },
      environment: async () => ({}),
    });
    const call = handler(
      { clusterID: "30", generation: 1, credentialHash: "unused" },
      "template-checkpoint",
      {
        operationID: "10",
        generation: 7,
        leaseToken: "a".repeat(43),
        phase: "migration",
        storageState: scenario === "storage-ready" ? "ready" : "allocating",
      },
    );
    if (scenario === "storage-ready") {
      await call;
      expect(JSON.parse(writes[0][1]).migration.storageState).toBe("ready");
    } else {
      await expect(call).rejects.toThrow();
      expect(writes).toHaveLength(0);
    }
  });
