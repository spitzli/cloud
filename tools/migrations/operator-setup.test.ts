import { test } from "node:test";
import assert from "node:assert/strict";
import { request } from "node:http";
import { startOperatorSetup } from "./operator-setup";

const env = {
  DATABASE_URL_UNPOOLED:
    "postgresql://webdock_offline_migrate:private@127.0.0.1:5544/webdock",
  BETTER_AUTH_URL: "https://auth.example.test",
  SPITZLI_STUDIO_ORIGIN: "https://studio.example.test",
  SPITZLI_CONSOLE_ORIGIN: "https://console.example.test",
};
const profile = {
  name: "Initial operator",
  email: "Owner@Example.test",
  password: "private-initial-password",
  ownershipConfirmed: true,
};
const options = {
  env,
  target: "127.0.0.1:5544/webdock",
  destination: "/unused/private-output.json",
};
const summary = {
  created: true as const,
  operatorID: "1",
  clientID: "2",
  checksum: "checksum",
  securitySetupRequired: true as const,
};

async function send(
  url: string,
  path: string,
  headers: Record<string, string> = {},
  body?: string,
) {
  const response = await fetch(url + path, {
    method: body === undefined ? "GET" : "POST",
    headers,
    body,
  });
  return {
    status: response.status,
    headers: response.headers,
    text: await response.text(),
  };
}
function auth(session: { url: string; capability: string }) {
  return {
    Authorization: "Bearer " + session.capability,
    Origin: session.url,
    "Content-Type": "application/json",
  };
}

test("setup binds only loopback, keeps the capability out of HTTP HTML and hardens every response", async () => {
  const session = await startOperatorSetup({
    ...options,
    provision: async () => summary,
  });
  try {
    assert.equal(new URL(session.url).hostname, "127.0.0.1");
    assert.ok(session.capability.length >= 43);
    const page = await send(session.url, "/");
    assert.equal(page.status, 200);
    assert.equal(page.headers.get("cache-control"), "no-store");
    assert.equal(page.headers.get("referrer-policy"), "no-referrer");
    assert.match(
      page.headers.get("content-security-policy")!,
      /default-src 'none'/,
    );
    assert.match(
      page.headers.get("content-security-policy")!,
      /frame-ancestors 'none'/,
    );
    assert.ok(!page.text.includes(session.capability));
    assert.match(page.text, /autocomplete="new-password"/);
    assert.match(page.text, /https:\/\/auth.example.test/);
    assert.equal(
      (await send(session.url, "/?token=" + session.capability)).status,
      404,
    );
    assert.equal(
      (await send(session.url, "/", { Origin: "https://attacker.invalid" }))
        .status,
      403,
    );
    const rebinding = await new Promise<number>((resolve, reject) => {
      request(session.url, { headers: { Host: "attacker.invalid" } }, (res) => {
        res.resume();
        resolve(res.statusCode!);
      })
        .on("error", reject)
        .end();
    });
    assert.equal(rebinding, 403);
  } finally {
    await session.close();
  }
});

test("setup rejects missing capabilities, cross-origin, non-JSON and oversized requests before provisioning", async () => {
  let calls = 0;
  const session = await startOperatorSetup({
    ...options,
    provision: async () => {
      calls++;
      return summary;
    },
  });
  try {
    assert.equal(
      (await send(session.url, "/setup", {}, JSON.stringify(profile))).status,
      403,
    );
    assert.equal(
      (
        await send(
          session.url,
          "/setup",
          { ...auth(session), Authorization: "Bearer wrong" },
          JSON.stringify(profile),
        )
      ).status,
      403,
    );
    assert.equal(
      (
        await send(
          session.url,
          "/setup",
          { ...auth(session), Origin: "https://attacker.invalid" },
          JSON.stringify(profile),
        )
      ).status,
      403,
    );
    assert.equal(
      (
        await send(
          session.url,
          "/setup",
          { ...auth(session), "Content-Type": "text/plain" },
          JSON.stringify(profile),
        )
      ).status,
      415,
    );
    assert.equal(
      (await send(session.url, "/setup", auth(session), "x".repeat(17000)))
        .status,
      413,
    );
    assert.equal(calls, 0);
  } finally {
    await session.close();
  }
});

test("setup requires ownership and validated identity, and refuses submitted origin changes", async () => {
  let calls = 0;
  const session = await startOperatorSetup({
    ...options,
    provision: async (input) => {
      calls++;
      assert.equal(input.email, "owner@example.test");
      assert.equal(input.authOrigin, "https://auth.example.test");
      return summary;
    },
  });
  try {
    for (const body of [
      "{",
      JSON.stringify({ ...profile, ownershipConfirmed: false }),
      JSON.stringify({ ...profile, password: "short" }),
      JSON.stringify({ ...profile, authOrigin: "https://attacker.invalid" }),
      JSON.stringify({ ...profile, email: "bad" }),
    ])
      assert.equal(
        (await send(session.url, "/setup", auth(session), body)).status,
        400,
      );
    assert.equal(calls, 0);
    assert.equal(
      (
        await send(
          session.url,
          "/setup",
          auth(session),
          JSON.stringify(profile),
        )
      ).status,
      200,
    );
    assert.equal(calls, 1);
  } finally {
    await session.close();
  }
});

test("setup permits one in-flight submission and returns no output credentials or IDs on success", async () => {
  let finish!: () => void;
  const pending = new Promise<void>((resolve) => {
    finish = resolve;
  });
  let entered!: () => void;
  const started = new Promise<void>((resolve) => {
    entered = resolve;
  });
  const session = await startOperatorSetup({
    ...options,
    provision: async () => {
      entered();
      await pending;
      return summary;
    },
  });
  try {
    const first = send(
      session.url,
      "/setup",
      auth(session),
      JSON.stringify(profile),
    );
    await started;
    assert.equal(
      (
        await send(
          session.url,
          "/setup",
          auth(session),
          JSON.stringify(profile),
        )
      ).status,
      409,
    );
    finish();
    const result = await first;
    assert.equal(result.status, 200);
    assert.deepEqual(JSON.parse(result.text), {
      created: true,
      securitySetupRequired: true,
    });
    await session.closed;
    await assert.rejects(fetch(session.url));
  } finally {
    finish();
    await session.close();
  }
});

test("setup expires and does not reveal database diagnostics", async () => {
  const session = await startOperatorSetup({
    ...options,
    ttlMs: 40,
    provision: async () => {
      throw Error("SQL secret-password localhost private");
    },
  });
  const result = await send(
    session.url,
    "/setup",
    auth(session),
    JSON.stringify(profile),
  );
  assert.equal(result.status, 500);
  assert.ok(!result.text.includes("secret-password"));
  await session.closed;
  await assert.rejects(fetch(session.url));
});

test("setup validates configured origins before opening a listener", async () => {
  await assert.rejects(
    startOperatorSetup({
      ...options,
      env: { ...env, BETTER_AUTH_URL: "https://auth.example.test/path" },
    }),
  );
  await assert.rejects(
    startOperatorSetup({
      ...options,
      env: { ...env, SPITZLI_CONSOLE_ORIGIN: env.SPITZLI_STUDIO_ORIGIN },
    }),
  );
});

test("setup closes after an uncertain commit and refuses retries", async () => {
  const { OperatorRunError } = await import("./operator-offline");
  const session = await startOperatorSetup({
    ...options,
    provision: async () => {
      throw new OperatorRunError(
        "Initial setup commit could not be confirmed; retain the private output and inspect the recorded IDs before retrying",
        true,
      );
    },
  });
  const result = await send(
    session.url,
    "/setup",
    auth(session),
    JSON.stringify(profile),
  );
  assert.equal(result.status, 500);
  assert.match(result.text, /retain the private output/);
  assert.equal((await session.closed).status, "uncertain");
  await assert.rejects(fetch(session.url));
});

test("setup waits for provisioning after the browser disconnects and still reports success", async () => {
  let finish!: () => void;
  const pending = new Promise<void>((resolve) => {
    finish = resolve;
  });
  let entered!: () => void;
  const started = new Promise<void>((resolve) => {
    entered = resolve;
  });
  const session = await startOperatorSetup({
    ...options,
    ttlMs: 2000,
    provision: async () => {
      entered();
      await pending;
      return summary;
    },
  });
  try {
    const req = request(session.url + "/setup", {
      method: "POST",
      headers: auth(session),
    });
    req.on("error", () => {});
    req.end(JSON.stringify(profile));
    await started;
    req.destroy();
    await new Promise((resolve) => setTimeout(resolve, 10));
    let closed = false;
    void session.closed.then(() => {
      closed = true;
    });
    const stopping = session.close();
    await new Promise((resolve) => setTimeout(resolve, 10));
    assert.equal(
      closed,
      false,
      "closing must not lose an in-progress database outcome",
    );
    finish();
    assert.equal((await stopping).status, "success");
  } finally {
    finish();
    await session.close();
  }
});

test(
  "setup rejects a slow partial body and releases its submission slot",
  { timeout: 10000 },
  async () => {
    const session = await startOperatorSetup({
      ...options,
      provision: async () => summary,
    });
    try {
      const response = await new Promise<{ status: number; text: string }>(
        (resolve, reject) => {
          const req = request(
            session.url + "/setup",
            {
              method: "POST",
              headers: { ...auth(session), "Content-Length": "400" },
            },
            (res) => {
              let body = "";
              res.on("data", (data) => {
                body += data;
              });
              res.on("end", () =>
                resolve({ status: res.statusCode!, text: body }),
              );
            },
          );
          req.on("error", reject);
          req.write("{");
        },
      );
      assert.equal(response.status, 408);
      assert.match(response.text, /timed out/);
      assert.equal(
        (
          await send(
            session.url,
            "/setup",
            auth(session),
            JSON.stringify(profile),
          )
        ).status,
        200,
      );
    } finally {
      await session.close();
    }
  },
);

test("setup shuts itself down when the browser disconnects before provisioning completes", async () => {
  let finish!: () => void;
  const pending = new Promise<void>((resolve) => {
    finish = resolve;
  });
  let entered!: () => void;
  const started = new Promise<void>((resolve) => {
    entered = resolve;
  });
  const session = await startOperatorSetup({
    ...options,
    ttlMs: 2000,
    provision: async () => {
      entered();
      await pending;
      return summary;
    },
  });
  try {
    const req = request(session.url + "/setup", {
      method: "POST",
      headers: auth(session),
    });
    req.on("error", () => {});
    req.end(JSON.stringify(profile));
    await started;
    req.destroy();
    await new Promise((resolve) => setTimeout(resolve, 30));
    finish();
    const outcome = await Promise.race([
      session.closed,
      new Promise<{ status: string }>((resolve) =>
        setTimeout(() => resolve({ status: "still-listening" }), 500),
      ),
    ]);
    assert.equal(
      outcome.status,
      "success",
      "consumed sessions must stop without external close or TTL",
    );
  } finally {
    finish();
    await session.close();
  }
});
