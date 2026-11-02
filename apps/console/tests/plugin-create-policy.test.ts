import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { z } from "zod";
import { ApiClient } from "../src/api";
import { creatable, pluginViewSchema } from "../src/plugins/views";

const manifest: unknown = JSON.parse(readFileSync(new URL("../../../plugins/postgres/manifest.json", import.meta.url), "utf8"));
const view = (blockedCreateActions?: string[]) => pluginViewSchema.parse({
  state: "enabled", runtime: "ready", manifest,
  features: ["create-shared", "create-dedicated"],
  resources: [{ id: "retained", name: "Existing database", offering: "shared", createdVia: "self-service" }],
  ...(blockedCreateActions ? { blockedCreateActions } : {}),
});

await test("retired PostgreSQL creates stay hidden despite retained feature grants", () => {
  const retained = view(["databases.create", "instances.create"]);
  assert.deepEqual(creatable(retained), []);
  assert.deepEqual(retained.features, ["create-shared", "create-dedicated"]);
  assert.equal(retained.resources[0]?.id, "retained");
  assert.ok(retained.manifest.actions.some(action => action.name === "databases.credentials"));
  assert.ok(retained.manifest.actions.some(action => action.name === "databases.delete"));
});

await test("create projection blocks only named actions and accepts older unrestricted responses", () => {
  assert.deepEqual(creatable(view(["instances.create"])).map(option => option.action.name), ["databases.create"]);
  assert.deepEqual(creatable(view()).map(option => option.action.name), ["databases.create", "instances.create"]);
});

await test("the real plugin API parser keeps create restrictions alongside retained maintenance access", async () => {
  const response = { ...view(), blockedCreateActions: ["databases.create", "instances.create"] };
  const api = new ApiClient({ apiOrigin: "https://api.example", authOrigin: "https://auth.example", enabledFeatures: [] }, (input, init) => {
    assert.ok(input instanceof URL);
    assert.equal(input.href, "https://api.example/api/console/plugins?projectID=123");
    assert.equal(init?.credentials, "include");
    return Promise.resolve(Response.json([response]));
  });
  const [received] = await api.get("/api/console/plugins?projectID=123", z.array(pluginViewSchema).max(64));
  assert.ok(received);
  assert.deepEqual(received.blockedCreateActions, ["databases.create", "instances.create"]);
  assert.deepEqual(creatable(received), []);
  assert.deepEqual(received.features, ["create-shared", "create-dedicated"]);
  assert.equal(received.resources[0]?.id, "retained");
  assert.deepEqual(received.manifest.actions.filter(action => ["databases.credentials", "databases.rotate", "databases.delete", "instances.delete"].includes(action.name)).map(action => action.name), ["databases.credentials", "databases.rotate", "databases.delete", "instances.delete"]);
});
