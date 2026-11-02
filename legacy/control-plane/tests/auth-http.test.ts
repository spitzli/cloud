import { expect, test } from "bun:test";
import { createApp } from "../src/app";
import { readConfig } from "../src/config";
import { authProtocol } from "../src/protocols/auth";
test("discovery and native handler preserve the canonical URL and raw body", async () => {
  const config = readConfig({ NODE_ENV: "test" });
  const handler = async (request: Request) =>
    Response.json({ url: request.url, body: await request.text() });
  const app = await createApp(config, {
    mounts: [authProtocol(config, handler)],
  });
  const discovery = await app.handle(
    new Request("http://localhost:3125/.well-known/openid-configuration"),
  );
  expect((await discovery.json()).url).toBe(
    "http://localhost:3125/api/auth/.well-known/openid-configuration",
  );
  const body = "grant_type=authorization_code&code=a%2Bb";
  const token = await app.handle(
    new Request("http://localhost:3125/api/auth/oauth2/token", {
      method: "POST",
      headers: { "content-type": "application/x-www-form-urlencoded" },
      body,
    }),
  );
  expect((await token.json()).body).toBe(body);
  expect(
    (
      await app.handle(
        new Request("http://localhost:3130/api/auth/oauth2/token", {
          method: "POST",
          body,
        }),
      )
    ).status,
  ).toBe(404);
});
