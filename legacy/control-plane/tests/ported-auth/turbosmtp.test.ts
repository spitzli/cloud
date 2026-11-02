import assert from "node:assert/strict";
import test from "node:test";
import { jest } from "bun:test";
import {
  TurboSMTPClient,
  TurboSMTPError,
} from "../../src/modules/mail/turbosmtp";

const auth = { consumerKey: "parent-key", consumerSecret: "parent-secret" };
const row = {
  subaccount_id: 123,
  active: true,
  email: "tenant@example.com",
  ip: "192.0.2.1",
  limit: 100,
  sent: 3,
  plan_limit_interval: "Monthly",
  parent_id: 999,
  password: "never-return",
  consumerSecret: "never-return",
};
const profile = {
  first_name: "Test",
  last_name: "Tenant",
  ip: "192.0.2.1",
  policy_agree: true as const,
};

test("list uses documented pagination and headers, returns only safe fields", async () => {
  const client = new TurboSMTPClient(auth, async (url, init) => {
    assert.equal(
      String(url),
      "https://pro.api.serversmtp.com/api/v2/subaccounts/list?page=2&limit=25&filter_by_email=tenant%40example.com",
    );
    const headers = new Headers(init?.headers);
    assert.equal(headers.get("consumerKey"), auth.consumerKey);
    assert.equal(headers.get("consumerSecret"), auth.consumerSecret);
    assert.equal(headers.get("Authorization"), null);
    assert.equal(init?.redirect, "error");
    assert.equal(init?.cache, "no-store");
    assert.equal(init?.method, "GET");
    return Response.json({ count: 26, results: [row], auth: "never-return" });
  });
  assert.deepEqual(
    await client.listSubaccounts({
      page: 2,
      limit: 25,
      filterByEmail: row.email,
    }),
    {
      count: 26,
      results: [
        {
          id: "123",
          active: true,
          email: row.email,
          assignedIP: row.ip,
          limit: 100,
          sent: 3,
          interval: "Monthly",
        },
      ],
    },
  );
  assert.equal(JSON.stringify(client).includes("parent-secret"), false);
});

test("documented create, patch, quota, status and active plan paths are exact", async () => {
  const requests: {
    path: string;
    method: string | undefined;
    body: unknown;
  }[] = [];
  const client = new TurboSMTPClient(
    { apiKey: "raw-key" },
    async (url, init) => {
      assert.equal(new Headers(init?.headers).get("Authorization"), "raw-key");
      requests.push({
        path: new URL(String(url)).pathname,
        method: init?.method,
        body: init?.body ? JSON.parse(String(init.body)) : null,
      });
      return Response.json(row);
    },
  );
  const request = {
    ...profile,
    email: row.email,
    password: "GeneratedPassword1!",
    confirm_password: "GeneratedPassword1!",
  };
  await client.createSubaccount(request);
  await client.updateSubaccount("123", profile);
  await client.setSubaccountLimit("123", -1);
  await client.setSubaccountActive("123", false);
  await client.getSubaccount("123");
  await client.getSubaccountPlan("123");
  assert.deepEqual(requests, [
    { path: "/api/v2/subaccounts", method: "POST", body: request },
    { path: "/api/v2/subaccounts/123", method: "PATCH", body: profile },
    {
      path: "/api/v2/subaccounts/123/updatesubaccountsmtplimit",
      method: "POST",
      body: { limit: -1 },
    },
    {
      path: "/api/v2/subaccounts/123/updatesubaccountstatus",
      method: "POST",
      body: { active: false },
    },
    { path: "/api/v2/subaccounts/123", method: "GET", body: null },
    { path: "/api/v2/subaccounts/123/active-plan", method: "GET", body: null },
  ]);
});

test("subaccount authorization and consumer key creation use separate credentials", async () => {
  let calls = 0;
  const client = new TurboSMTPClient(auth, async (url, init) => {
    calls++;
    const headers = new Headers(init?.headers);
    if (String(url).endsWith("/subaccounts/authorize")) {
      assert.equal(headers.get("consumerSecret"), auth.consumerSecret);
      assert.deepEqual(JSON.parse(String(init?.body)), { email: row.email });
      return Response.json({ auth: "tenant-api-key" });
    }
    assert.equal(
      String(url),
      "https://pro.api.serversmtp.com/api/v2/user/consumerKeys",
    );
    assert.equal(headers.get("Authorization"), "tenant-api-key");
    assert.equal(headers.get("consumerKey"), null);
    assert.deepEqual(JSON.parse(String(init?.body)), {
      label: "Webdock",
      permissions: ["APIS"],
    });
    return Response.json({
      consumerKey: "tenant-key",
      consumerSecret: "tenant-secret",
      parentSecret: "never-return",
    });
  });
  const apiKey = await client.authorizeSubaccount(row.email);
  assert.deepEqual(await client.createConsumerKey(apiKey, "Webdock"), {
    consumerKey: "tenant-key",
    consumerSecret: "tenant-secret",
  });
  assert.equal(calls, 2);
});

test("invalid IDs, profiles, credentials and quota are rejected before network access", async () => {
  let calls = 0;
  const client = new TurboSMTPClient(auth, async () => {
    calls++;
    return Response.json(row);
  });
  for (const id of [
    "../authorize",
    "123?x=1",
    "https://evil.test",
    "0",
    "1.1",
    "9007199254740992",
  ]) {
    await assert.rejects(client.getSubaccount(id), { code: "invalid_input" });
  }
  await assert.rejects(client.setSubaccountLimit("123", -2), {
    code: "invalid_input",
  });
  await assert.rejects(
    client.setSubaccountActive("123", "false" as unknown as boolean),
    { code: "invalid_input" },
  );
  await assert.rejects(client.listSubaccounts({ page: 0 }), {
    code: "invalid_input",
  });
  await assert.rejects(
    client.createSubaccount({
      ...profile,
      policy_agree: false,
      email: row.email,
      password: "weak",
      confirm_password: "weak",
    } as never),
    { code: "invalid_input" },
  );
  await assert.rejects(
    client.updateSubaccount("123", { ...profile, ip: "::1" }),
    { code: "invalid_input" },
  );
  await assert.rejects(
    client.updateSubaccount("123", {
      ...profile,
      unexpected: "secret",
    } as never),
    { code: "invalid_input" },
  );
  assert.throws(
    () => new TurboSMTPClient({ apiKey: "secret\r\nHeader: bad" }),
    { code: "invalid_input" },
  );
  assert.equal(calls, 0);
});

test("provider and network errors do not leak credentials, bodies or personal data; writes are never retried", async () => {
  for (const status of [302, 400, 401, 429, 500]) {
    let calls = 0;
    const client = new TurboSMTPClient(auth, async () => {
      calls++;
      return new Response("parent-secret tenant@example.com", { status });
    });
    await assert.rejects(
      client.setSubaccountActive("123", true),
      (error: unknown) => {
        assert.ok(error instanceof TurboSMTPError);
        assert.equal(error.status, status);
        assert.equal(error.code, "upstream");
        assert.doesNotMatch(String(error), /parent-secret|tenant@example/);
        return true;
      },
    );
    assert.equal(calls, 1);
  }
  await assert.rejects(
    new TurboSMTPClient(auth, async () => {
      throw new Error("parent-secret");
    }).getSubaccount("123"),
    { message: "turboSMTP request failed.", code: "upstream" },
  );
});

test("malformed or oversized provider responses are rejected", async () => {
  for (const response of [
    new Response("parent-secret"),
    Response.json({ ...row, subaccount_id: "../../authorize" }),
    new Response("x".repeat(1_048_577)),
    new Response("{}", { headers: { "Content-Length": "1048577" } }),
  ]) {
    await assert.rejects(
      new TurboSMTPClient(auth, async () => response).getSubaccount("123"),
      { code: "invalid_response" },
    );
  }
});

test("ten second deadline covers both fetch and streamed body", async (t) => {
  jest.useFakeTimers();
  t.after(() => jest.useRealTimers());
  for (const bodyStalls of [false, true]) {
    const client = new TurboSMTPClient(auth, async (_url, init) => {
      if (bodyStalls) return new Response(new ReadableStream({ start() {} }));
      return new Promise((_resolve, reject) =>
        init?.signal?.addEventListener("abort", () =>
          reject(new Error("secret")),
        ),
      );
    });
    const pending = client.getSubaccount("123");
    const rejected = assert.rejects(pending, { code: "timeout" });
    await Promise.resolve();
    jest.advanceTimersByTime(10_000);
    await rejected;
  }
});

test("consumer key metadata and revocation use token auth, explicit sending scopes and encoded IDs", async () => {
  const calls: { url: string; options: RequestInit | undefined }[] = [];
  const provider = new TurboSMTPClient(
    { apiKey: "child-token" },
    async (url, options) => {
      calls.push({ url: String(url), options });
      if (options?.method === "DELETE")
        return Response.json({ message: "success" });
      if (options?.method === "POST")
        return Response.json({
          consumerKey: "new-key",
          consumerSecret: "one-time-secret",
        });
      return Response.json({
        count: 1,
        results: [
          {
            consumerKey: "key/with?path",
            label: "Website",
            creation_time: "2026-10-04 12:00:00",
            ips: [],
            is_legacy: false,
            permissions: ["SEND_SMTP"],
            consumerSecret: "must-not-leak",
          },
        ],
      });
    },
  );
  const listed = await provider.listConsumerKeys();
  assert.ok(!JSON.stringify(listed).includes("must-not-leak"));
  await provider.createConsumerKey("child-token", "Website", {
    permissions: ["SEND_SMTP", "SEND_API"],
    ips: ["2001:db8::1"],
  });
  assert.deepEqual(JSON.parse(String(calls[1].options?.body)).permissions, [
    "SEND_SMTP",
    "SEND_API",
  ]);
  await provider.deleteConsumerKey("key/with?path");
  assert.ok(calls[2].url.endsWith("key%2Fwith%3Fpath"));
  for (const call of calls)
    assert.equal(
      new Headers(call.options?.headers).get("Authorization"),
      "child-token",
    );
  await assert.rejects(
    provider.createConsumerKey("child-token", "Website", {
      permissions: ["SEND_SMTP"],
      ips: ["10.0.0.0/24"],
    }),
  );
  await assert.rejects(provider.deleteConsumerKey(".."));
  const pair = new TurboSMTPClient(
    { consumerKey: "key", consumerSecret: "secret" },
    async () => {
      throw Error("must not fetch");
    },
  );
  await assert.rejects(pair.listConsumerKeys(), /Invalid turboSMTP input/);
  await assert.rejects(
    pair.deleteConsumerKey("key"),
    /Invalid turboSMTP input/,
  );
});
