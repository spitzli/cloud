import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { parsePluginManifest } from "../src/contracts/index.ts";
const fixture = () =>
  JSON.parse(
    readFileSync(
      new URL(
        "../../../tools/plugins/fixtures/project-notes.json",
        import.meta.url,
      ),
      "utf8",
    ),
  );
test("accepts a pinned manifest without changing it", () => {
  const m = fixture();
  assert.deepEqual(parsePluginManifest(m), m);
});
test("rejectsUnknownPrivilegedFields", () => {
  for (const key of [
    "command",
    "namespace",
    "rbac",
    "mounts",
    "sql",
    "hostURL",
  ]) {
    const m = fixture();
    m[key] = "forbidden";
    assert.throws(() => parsePluginManifest(m), key);
  }
});
test("rejectsUnpinnedImage", () => {
  for (const image of [
    "registry.example.com/notes:latest",
    "notes",
    "https://example.com/notes@sha256:" + "a".repeat(64),
  ]) {
    const m = fixture();
    m.image = image;
    assert.throws(() => parsePluginManifest(m));
  }
});
test("rejectsDuplicateAndCyclicDependencies", () => {
  for (const dependencies of [
    [
      { id: "other", version: "1.0.0" },
      { id: "other", version: "1.0.0" },
    ],
    [{ id: "project-notes", version: "1.0.0" }],
    [{ id: "other", version: "^1.0.0" }],
  ]) {
    const m = fixture();
    m.dependencies = dependencies;
    assert.throws(() => parsePluginManifest(m));
  }
});
test("rejectsUnsupportedProtocolOrDataProtocol", () => {
  for (const key of ["protocolVersion", "sdkVersion", "dataProtocolVersion"]) {
    const m = fixture();
    m[key] = 2;
    assert.throws(() => parsePluginManifest(m));
  }
});
test("rejectsInvalidSchemaCompatibilityRange", () => {
  for (const range of [
    { min: 2, max: 1 },
    { min: 0, max: 1 },
    { min: 2, max: 3 },
  ]) {
    const m = fixture();
    m.readableSchema = range;
    assert.throws(() => parsePluginManifest(m));
  }
});
test("rejects unsafe paths, duplicate actions and undeclared or write capabilities in read actions", () => {
  for (const path of [
    "//evil.example",
    "/ui/../webdock/actions",
    "/ui/%2e%2e/secrets",
    "/ui/x?token=secret",
  ]) {
    const m = fixture();
    m.navigation = [{ id: "notes", label: "Notes", path }];
    assert.throws(() => parsePluginManifest(m));
  }
  const m = fixture();
  m.actions.push(m.actions[0]);
  assert.throws(() => parsePluginManifest(m));
  for (const cap of ["plugin.data.write", "plugin.secrets.read"]) {
    const m = fixture();
    m.actions[0].capabilities = [cap];
    assert.throws(() => parsePluginManifest(m));
  }
});
test("rejects reserved ids, oversized manifests and unsafe configuration schemas", () => {
  for (const name of [
    "mail",
    "hosting",
    "databases",
    "cms",
    "storage",
    "billing",
    "core",
    "../notes",
  ]) {
    const m = fixture();
    m.name = name;
    assert.throws(() => parsePluginManifest(m));
  }
  const m = fixture();
  m.description = "x".repeat(262145);
  assert.throws(() => parsePluginManifest(m));
  for (const schema of [
    { $ref: "https://evil.example/schema" },
    { type: "object", properties: { x: { $ref: "#" } } },
    { type: "object", patternProperties: { ".*": { type: "string" } } },
  ]) {
    const m = fixture();
    m.configuration = schema;
    assert.throws(() => parsePluginManifest(m));
  }
});
test("bounds job interval and rejects destructive or unordered migrations", () => {
  const m = fixture();
  m.jobs[0].intervalSeconds = 59;
  assert.throws(() => parsePluginManifest(m));
  for (const migrations of [
    [
      {
        version: 1,
        checksum: "a".repeat(64),
        steps: [{ op: "sql", sql: "DROP TABLE users" }],
      },
    ],
    [
      { version: 2, checksum: "a".repeat(64), steps: [] },
      { version: 1, checksum: "b".repeat(64), steps: [] },
    ],
  ]) {
    const m = fixture();
    m.migrations = migrations;
    assert.throws(() => parsePluginManifest(m));
  }
});
test("rejects type-mismatched defaults and invalid schema constraints", () => {
  for (const configuration of [
    { type: "string", default: 7 },
    { type: "object", properties: {}, required: ["missing"] },
    { type: "string", minLength: 8, maxLength: 2 },
    { type: "object", properties: { x: { type: "array" } } },
  ]) {
    const m = fixture();
    m.configuration = configuration;
    assert.throws(() => parsePluginManifest(m));
  }
});
test("column defaults match their type and nullability before migration", () => {
  const sample = (type: string, value: unknown, nullable = false) => {
    const m = fixture();
    const table = {
      name: "notes",
      columns: [
        { name: "id", type: "string", nullable: false },
        { name: "value", type, nullable, default: value },
      ],
      primaryKey: ["id"],
      indexes: [],
      foreignKeys: [],
    };
    m.tables = [table];
    m.migrations = [
      {
        version: 1,
        checksum: "a".repeat(64),
        steps: [{ op: "createTable", table }],
      },
    ];
    return m;
  };
  for (const [type, value] of [
    ["integer", "no"],
    ["integer", 1.5],
    ["integer", Number.MAX_SAFE_INTEGER + 1],
    ["boolean", "false"],
    ["timestamp", "tomorrow"],
    ["decimal", 1.2],
    ["decimal", "NaN"],
    ["string", null],
  ])
    assert.throws(() => parsePluginManifest(sample(type as string, value)));
  for (const [type, value] of [
    ["integer", 42],
    ["boolean", false],
    ["timestamp", "2026-10-16T10:00:00Z"],
    ["decimal", "12.50"],
    ["string", "hello"],
  ])
    assert.doesNotThrow(() =>
      parsePluginManifest(sample(type as string, value)),
    );
  assert.doesNotThrow(() => parsePluginManifest(sample("integer", null, true)));
});

test("requires runtime fields at the manifest root and rejects the retired wrapper", () => {
  const m = fixture();
  assert.equal(Object.hasOwn(m, "x-webdock"), false);
  const { name, version, kind, description, ...runtime } = m;
  assert.throws(() =>
    parsePluginManifest({
      name,
      version,
      kind,
      description,
      "x-webdock": runtime,
    }),
  );
  delete m.image;
  assert.throws(() => parsePluginManifest(m));
});

test("accepts namespaced API routes and MCP tools referencing declared actions", () => {
  const m = fixture();
  m.api = [{ method: "GET", path: "/notes", action: m.actions[0].name }];
  m.mcp = [
    {
      name: "list_notes",
      description: "List project notes",
      action: m.actions[0].name,
    },
  ];
  assert.deepEqual(parsePluginManifest(m), m);
});
test("rejects route collisions, ambiguous paths and unknown action references", () => {
  for (const api of [
    [{ method: "GET", path: "/../admin", action: "notes.list" }],
    [{ method: "GET", path: "/notes", action: "missing" }],
    [
      { method: "GET", path: "/notes", action: "notes.list" },
      { method: "GET", path: "/notes", action: "notes.list" },
    ],
    [{ method: "GET", path: "/notes/:id", action: "notes.list" }],
  ]) {
    const m = fixture();
    m.api = api;
    assert.throws(() => parsePluginManifest(m));
  }
});
test("read HTTP methods cannot invoke write actions and MCP names are local", () => {
  const m = fixture();
  const write = m.actions.find((a: any) => a.intent === "write");
  m.api = [{ method: "GET", path: "/save", action: write.name }];
  assert.throws(() => parsePluginManifest(m));
  delete m.api;
  for (const name of ["other/tool", "../admin", "a".repeat(65)]) {
    m.mcp = [{ name, description: "test", action: m.actions[0].name }];
    assert.throws(() => parsePluginManifest(m));
  }
});
test("website-scoped actions declare exactly one HTTP alias and no extra authority", () => {
  const m = fixture();
  m.capabilities = [
    "plugin.http.read",
    "plugin.http.request",
    "plugin.apps.provision",
  ];
  m.httpBindings = ["cms"];
  m.actions = [
    {
      name: "cms.write",
      intent: "write",
      httpBinding: "cms",
      capabilities: ["plugin.http.request"],
    },
  ];
  m.jobs = [];
  m.slots = [];
  assert.equal(parsePluginManifest(m).actions[0].httpBinding, "cms");
  for (const patch of [
    { httpBinding: "other" },
    { capabilities: ["plugin.http.request", "plugin.apps.provision"] },
    { capabilities: [] },
    { capabilities: ["plugin.http.read"] },
  ])
    assert.throws(() =>
      parsePluginManifest({ ...m, actions: [{ ...m.actions[0], ...patch }] }),
    );
});

test("schema fields support camelCase while action names and prototype paths stay fenced", () => {
  const m = fixture();
  m.actions[0].output = {type:"object",properties:{expiresAt:{type:"integer"},exactBytes:{type:"integer"}},required:["expiresAt","exactBytes"]};
  assert.doesNotThrow(() => parsePluginManifest(m));
  for (const key of ["constructor","prototype","__proto__","ExpiresAt","a.constructor","a.__proto__.b"]) {
    const bad = structuredClone(m);
    bad.actions[0].output = JSON.parse(JSON.stringify({type:"object",properties:{[key]:{type:"string"}}}));
    assert.throws(() => parsePluginManifest(bad));
  }
  m.actions[0].name="downloadObject";
  assert.throws(() => parsePluginManifest(m));
});
