import assert from "node:assert/strict";
import { test } from "node:test";
import {
  appSchema,
  actionCommand,
  settingsCommand,
  createCommand,
  deleteCommand,
  deletionPlanSchema,
  operationPolling,
  canManageApp,
} from "../src/hosting-contracts";
const spec = {
  template: "custom",
  image: `registry.example/app@sha256:${"a".repeat(64)}`,
  args: [],
  port: 8080,
  healthPath: "/",
  cpuMillicores: 100,
  memoryBytes: 67108864,
  ephemeralBytes: 67108864,
  replicas: 1,
  volumeBytes: 0,
};
const app = appSchema.parse({
  id: "301",
  projectID: "201",
  name: "Application",
  lifecycle: "deployed",
  spec,
  revision: 7,
  observedRevision: 6,
  status: "ready",
  operationID: null,
  logs: null,
  lastError: null,
  environmentNames: ["TOKEN"],
  subscriptionRevision: 3,
  operator: false,
  canManage: true,
  canRequestLogs: true,
});
await test("hosting controls carry reviewed revisions and stable explicit request identity", () => {
  const key = "test-request-identity-1";
  const command = actionCommand(app, "apps.stop", key);
  assert.deepEqual(command, {
    action: "apps.stop",
    appID: "301",
    revision: 7,
    subscriptionRevision: 3,
    idempotencyKey: key,
  });
  assert.deepEqual(JSON.parse(JSON.stringify(command)), command);
  assert.equal(canManageApp(app, true), false);
  assert.equal(canManageApp({ ...app, canManage: false }, false), false);
  assert.equal(canManageApp({ ...app, status: "deleted" }, false), false);
});
await test("settings preserve retained storage and never substitute secret values", () => {
  const data = new FormData();
  for (const [name, value] of Object.entries({
    image: spec.image,
    args: '["--serve"]',
    port: "8081",
    healthPath: "/health",
    cpuMillicores: "0.25 cores",
    memoryBytes: "128 MB",
    ephemeralBytes: "128 MB",
    replicas: "1",
  }))
    data.set(name, value);
  const command = settingsCommand(app, data, "test-request-identity-2");
  assert.equal(command.action, "apps.update");
  assert.equal(command.spec.volumeBytes, 0);
  assert.equal(command.spec.cpuMillicores, 250);
  assert.equal(command.revision, 7);
  assert.equal("environment" in command, false);
  const withHost = appSchema.parse({ ...app, spec: { ...app.spec, healthHost: "api.example.test:3130" } });
  assert.equal(settingsCommand(withHost, data, "probe-host-settings-preserved").spec.healthHost, "api.example.test:3130");
  data.set("replicas", "2.5");
  assert.throws(() => settingsCommand(app, data, "test-request-identity-3"));
});
await test("environment changes are explicit write-only patches; untouched values are never sent", () => {
  const data = new FormData();
  for (const [name, value] of Object.entries({
    image: spec.image, args: "[]", port: "8080", healthPath: "/",
    cpuMillicores: "0.1 cores", memoryBytes: "64 MB", ephemeralBytes: "64 MB", replicas: "1",
  }))
    data.set(name, value);
  data.set("environment", "[]");
  assert.equal("environment" in settingsCommand(app, data, "environment-request-none"), false);
  data.set("environment", JSON.stringify([{ name: "API_TOKEN", value: "replacement" }, { name: "OLD_FLAG", value: null }]));
  const command = settingsCommand(app, data, "environment-request-patch");
  assert.equal(command.action, "apps.update");
  assert.deepEqual("environment" in command ? command.environment : undefined, [{ name: "API_TOKEN", value: "replacement" }, { name: "OLD_FLAG", value: null }]);
  for (const invalid of [
    [{ name: "1BAD", value: "x" }],
    [{ name: "DUP", value: "a" }, { name: "DUP", value: "b" }],
    [{ name: "NUL", value: "a\u0000b" }],
    Array.from({ length: 33 }, (_, i) => ({ name: `V${String(i)}`, value: "x" })),
    "not json",
  ]) {
    data.set("environment", typeof invalid === "string" ? invalid : JSON.stringify(invalid));
    assert.throws(() => settingsCommand(app, data, "environment-request-invalid"));
  }
});
await test("only outstanding hosting operations poll; errors and reconciliation stop", () => {
  assert.equal(operationPolling("queued", false), 2000);
  assert.equal(operationPolling("running", false), 2000);
  for (const status of ["succeeded", "failed", "needs-reconciliation"])
    assert.equal(operationPolling(status, false), false);
  assert.equal(operationPolling("running", true), false);
});

await test("network and server failures retain request identity, while denial and conflict are definitive", async () => {
  const { uncertainOutcome } = await import("../src/hosting-client");
  const { ApiError } = await import("../src/api");
  assert.equal(uncertainOutcome(new Error("Disconnected")), true);
  assert.equal(uncertainOutcome(new ApiError(503, "Unavailable")), true);
  assert.equal(uncertainOutcome(new ApiError(409, "Changed")), false);
  assert.equal(uncertainOutcome(new ApiError(403, "Denied")), false);
  assert.equal(uncertainOutcome(null), false);
});

await test("hosting deep links identify selectors without inferring tenant ownership", async () => {
  const { hostingLocation } = await import("../src/hosting-context");
  for (const suffix of ["", "/settings", "/logs"])
    assert.deepEqual(
      hostingLocation(`/hosting/apps/9007199254740993${suffix}`),
      { kind: "app", id: "9007199254740993" },
    );
  assert.deepEqual(hostingLocation("/hosting/operations/701"), {
    kind: "operation",
    id: "701",
  });
  assert.equal(hostingLocation("/hosting/apps/not-an-id").kind, "invalid");
  assert.equal(hostingLocation("/projects/201").kind, "none");
});
function createForm(overrides: Record<string, string> = {}) {
  const data = new FormData();
  for (const [name, value] of Object.entries({
    name: "new-app", image: spec.image, args: '["--serve"]', port: "8080", healthPath: "/health",
    cpuMillicores: "0.1 cores", memoryBytes: "64 MB", ephemeralBytes: "64 MB", replicas: "1", volumeBytes: "0 MB",
    environment: "[]", ...overrides,
  }))
    data.set(name, value);
  return data;
}
await test("creation builds one custom application with explicit resources and optional environment", () => {
  const command = createCommand("201", 3, createForm(), "create-request-identity-1");
  assert.equal(command.action, "apps.create");
  assert.equal(command.projectID, "201");
  assert.equal(command.subscriptionRevision, 3);
  assert.equal(command.spec.template, "custom");
  assert.deepEqual(command.spec.args, ["--serve"]);
  assert.equal(command.spec.cpuMillicores, 100);
  assert.equal("environment" in command, false);
  const withEnvironment = createCommand("201", 3, createForm({ environment: JSON.stringify([{ name: "TOKEN", value: "v" }]) }), "create-request-identity-2");
  assert.deepEqual("environment" in withEnvironment ? withEnvironment.environment : undefined, [{ name: "TOKEN", value: "v" }]);
  for (const invalid of [
    { name: "" }, { image: "nginx:latest" }, { args: "--serve" }, { replicas: "2", volumeBytes: "128 MB" },
    { environment: JSON.stringify([{ name: "GONE", value: null }]) },
  ])
    assert.throws(() => createCommand("201", 3, createForm(invalid), "create-request-identity-3"));
});
await test("deletion requires the exact typed name and the reviewed plan hash", () => {
  const plan = deletionPlanSchema.parse({ plan: { name: "Application", resources: ["Deployment", "Service", "Secret"], persistentData: false, volumeBytes: 0 }, planHash: "b".repeat(64) });
  const command = deleteCommand(app, plan, "Application", "delete-request-identity-1");
  assert.deepEqual(command, { action: "apps.delete", appID: "301", confirmName: "Application", planHash: "b".repeat(64), idempotencyKey: "delete-request-identity-1" });
  assert.throws(() => deleteCommand(app, plan, "application", "delete-request-identity-2"));
  const stale = deletionPlanSchema.parse({ ...plan, plan: { ...plan.plan, name: "Renamed" } });
  assert.throws(() => deleteCommand(app, stale, "Application", "delete-request-identity-3"));
});

await test('public address requests are explicit and do not alter existing private app forms',()=>{
 const input=createForm({publicAddress:'on'});
 assert.deepEqual(createCommand('201',3,input,'public-address-request-01').publicAddress,{});
 assert.equal(createCommand('201',3,createForm(),'public-address-request-02').publicAddress,undefined);
});
