import { expect, test, mock } from "bun:test";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";
import { inspectContract, preflightV1 } from "./prepare";

const source = process.env.TABULARIUM_SOURCE_ROOT;
if (!source) throw new Error("Set TABULARIUM_SOURCE_ROOT to the reviewed Tabularium checkout");
const schemaPath = resolve("tools/plugins/tabularium-kind.schema.json");
const delta = JSON.parse(await readFile(schemaPath, "utf8"));
const { buildSchema } = await import(resolve(source, "packages/manifest/src/schema.ts"));
const { ManifestSchema } = await import(resolve(source, "packages/manifest/src/core.ts"));
const { validateManifest } = await import(resolve(source, "packages/manifest/src/validate.ts"));
mock.module("@tabularium/manifest", () => ({ ManifestSchema }));
// The schema safety checker only uses i18n constants. Prevent settings from
// initializing the sibling application's database in this offline proof.
mock.module(resolve(source, "apps/api/src/lib/settings.ts"), () => ({ getSetting: () => undefined, isSettingsInitialized: () => false }));
const { validateExtensionsDelta } = await import(resolve(source, "apps/api/src/lib/extensions-schema.ts"));
const legacy = JSON.parse(await readFile("../webdock-extension-project-notes/tabularium.json", "utf8"));

test("legacy property-delta API drops root constraints; explicit root admission is separate", () => {
  const proposed = { type: "object", properties: delta, oneOf: [
    { properties: { protocolVersion: { const: 1 }, sdkVersion: { const: 1 }, dataProtocolVersion: { const: 1 } } },
    { properties: { protocolVersion: { const: 2 }, sdkVersion: { const: 2 }, dataProtocolVersion: { const: 2 } } },
  ] };
  expect(validateExtensionsDelta(proposed)).toEqual(delta);
  expect(validateExtensionsDelta(proposed)).not.toHaveProperty("oneOf");
  expect(["blocked", "supported"]).toContain(inspectContract(source, schemaPath).status);
});

test("per-field enum widening demonstrably accepts mixed tuples", () => {
  const unsafe = structuredClone(delta);
  for (const key of ["protocolVersion", "sdkVersion", "dataProtocolVersion"]) {
    unsafe[key] = { type: "integer", enum: [1, 2], required: true };
  }
  const schema = buildSchema({ coreSchema: ManifestSchema, kind: "extension", kindOverrides: { extension: unsafe } });
  expect(validateManifest({ ...legacy, protocolVersion: 2 }, schema).ok).toBe(true);
  const global = buildSchema({ coreSchema: ManifestSchema, kindOverrides: { extension: unsafe } });
  expect(validateManifest({ ...legacy, protocolVersion: 2 }, global).ok).toBe(true);
});

test("actual published v1 validates without modifying its bytes", () => {
  const raw = JSON.stringify(legacy);
  expect(preflightV1(raw).name).toBe("project-notes");
  const schema = buildSchema({ coreSchema: ManifestSchema, kind: "extension", kindOverrides: { extension: delta } });
  expect(validateManifest(legacy, schema).ok).toBe(true);
  expect(JSON.stringify(legacy)).toBe(raw);
});

test("v1 preflight rejects mixed tuples, mutable or placeholder images and unknown privilege", () => {
  for (const patch of [
    { sdkVersion: 2 },
    { image: "ghcr.io/spitzli/project-notes:latest" },
    { image: "ghcr.io/spitzli/project-notes@sha256:" + "0".repeat(64) },
    { privileged: true },
    { capabilities: ["plugin.root"] },
    { egress: [{ host: "*", port: 443 }] },
    { services: { required: [{ name: "root", version: 1 }], optional: [] } },
    { actions: Array(129).fill(legacy.actions[0]) },
  ]) expect(() => preflightV1(JSON.stringify({ ...legacy, ...patch }))).toThrow();
  expect(() => preflightV1(" ".repeat(65537))).toThrow();
});
