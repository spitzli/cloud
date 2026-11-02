import { expect, test } from "bun:test";
import { readFile, access } from "node:fs/promises";
import { createApp } from "../src/app";
import { readConfig } from "../src/config";

test("removed billing HTTP surfaces cannot be mounted in the core API", async () => {
  const config = readConfig({});
  const app = await createApp(config);
  for (const path of [
    "/v1/tenant/billing/status",
    "/v1/tenant/billing/checkout",
    "/v1/operator/billing/products",
    "/webhooks/polar",
    "/billing/return",
  ]) {
    expect(
      (await app.handle(new Request(config.apiOrigin + path))).status,
    ).toBe(404);
  }
  expect(() => readConfig({ WEBDOCK_API_DOMAINS: "billing" })).toThrow(
    "Unknown API domain",
  );
});

test("billing has no RPC dispatch, MCP registration or provider implementation", async () => {
  for (const path of [
    "modules/portal/studio-api.ts",
    "modules/mcp/mcp-server.ts",
    "index.ts",
    "worker.ts",
  ]) {
    const source = await readFile(
      new URL("../src/" + path, import.meta.url),
      "utf8",
    );
    expect(source).not.toMatch(
      /billing\.|billing\/|billing_product|polar-hosted/,
    );
  }
  await expect(
    access(new URL("../../../modules/billing", import.meta.url)),
  ).rejects.toThrow();
  await expect(
    access(new URL("../../../adapters/billing/polar", import.meta.url)),
  ).rejects.toThrow();
});
