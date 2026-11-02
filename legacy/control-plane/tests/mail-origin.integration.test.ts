import { expect, test } from "bun:test";
import { createApp } from "../src/app";
import { readConfig } from "../src/config";
import { legacyAuthProtocols } from "../src/protocols/legacy-auth";
import { mailOperations } from "../src/modules/mail/product/api-contract";

test("every Mail operation requires its API key on canonical and legacy hosts", async () => {
  const config = readConfig({
    NODE_ENV: "test",
    WEBDOCK_API_ORIGIN: "http://localhost:3130",
    BETTER_AUTH_URL: "http://localhost:3125",
    WEBDOCK_STUDIO_ORIGIN: "http://localhost:3120",
    WEBDOCK_API_DOMAINS: "mail",
  });
  const app = await createApp(config, {
    mounts: await legacyAuthProtocols(config),
  });
  for (const origin of [config.apiOrigin, config.authOrigin]) {
    const specResponse = await app.handle(
      new Request(origin + "/api/mail/v1/openapi.json"),
    );
    expect(specResponse.status).toBe(200);
    expect((await specResponse.json()).info.title).toBe("Webdock Mail API");
    for (const operation of mailOperations) {
      const path = operation.path.replace(/\{[^}]+\}/g, "123");
      const response = await app.handle(
        new Request(origin + "/api/mail/v1" + path, {
          method: operation.method,
          ...(operation.method === "GET"
            ? {}
            : { body: "{}", headers: { "content-type": "application/json" } }),
        }),
      );
      expect(response.status).toBe(401);
      expect((await response.json()).name).toBe("missing_api_key");
    }
  }
  expect(
    (
      await app.handle(
        new Request(config.studioOrigin + "/api/mail/v1/openapi.json"),
      )
    ).status,
  ).toBe(404);
  for (const path of ["/api/mail/gateway", "/api/mail/provider-events/123"])
    expect(
      (
        await app.handle(
          new Request(config.apiOrigin + path, { method: "POST", body: "{}" }),
        )
      ).status,
    ).toBe(404);
});
