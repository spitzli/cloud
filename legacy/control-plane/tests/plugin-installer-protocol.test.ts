import { test, expect } from "bun:test";
import { pluginInstallerProtocol } from "../src/protocols/plugin-installer";

test("installer HTTP authenticates before dispatch and bounds JSON without disclosing failures", async () => {
  const previous = {
    token: process.env.WEBDOCK_PLUGIN_INSTALLER_TOKEN,
    file: process.env.WEBDOCK_PLUGIN_RUNTIME_FILE,
  };
  const token = "fixture-installer-" + "a".repeat(40);
  process.env.WEBDOCK_PLUGIN_INSTALLER_TOKEN = token;
  process.env.WEBDOCK_PLUGIN_RUNTIME_FILE = "/fixture/runtime.json";
  let calls = 0;
  const { app } = pluginInstallerProtocol(
    {
      apiOrigin: "https://api.example",
      authOrigin: "https://auth.example",
      studioOrigin: "https://studio.example",
      production: false,
      domains: ["registry"],
      port: 3130,
    },
    async (action) => {
      calls++;
      if (action === "complete") throw Error("private-provider-token");
      return null;
    },
  );
  const request = (
    action: string,
    authorization: string | undefined,
    body = "{}",
  ) =>
    app.handle(
      new Request("https://api.example/v1/plugin-installer/" + action, {
        method: "POST",
        headers: {
          "content-type": "application/json",
          ...(authorization ? { authorization } : {}),
        },
        body,
      }),
    );
  try {
    expect((await request("claim", undefined)).status).toBe(401);
    expect((await request("claim", "Bearer wrong")).status).toBe(401);
    expect(calls).toBe(0);
    const claimed = await request("claim", "Bearer " + token);
    expect(claimed.status).toBe(200);
    expect(await claimed.json()).toBeNull();
    expect(calls).toBe(1);
    expect(
      (
        await request(
          "source",
          "Bearer " + token,
          '{"data":"' + "a".repeat(512 * 1024) + '"}',
        )
      ).status,
    ).toBe(413);
    expect(calls).toBe(1);
    const failed = await request("complete", "Bearer " + token);
    expect(failed.status).toBe(503);
    expect(await failed.text()).not.toContain("private-provider-token");
    expect((await request("unknown", "Bearer " + token)).status).toBe(404);
    expect(calls).toBe(2);
  } finally {
    if (previous.token === undefined)
      delete process.env.WEBDOCK_PLUGIN_INSTALLER_TOKEN;
    else process.env.WEBDOCK_PLUGIN_INSTALLER_TOKEN = previous.token;
    if (previous.file === undefined)
      delete process.env.WEBDOCK_PLUGIN_RUNTIME_FILE;
    else process.env.WEBDOCK_PLUGIN_RUNTIME_FILE = previous.file;
  }
});
