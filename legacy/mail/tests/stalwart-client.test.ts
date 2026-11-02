import assert from "node:assert/strict";
import { createServer, type RequestListener } from "node:http";
import test from "node:test";
import { StalwartClient } from "../src/stalwart/client.ts";

async function server(handler: RequestListener) {
  const http = createServer(handler);
  await new Promise<void>(resolve => http.listen(0, "127.0.0.1", resolve));
  const address = http.address();
  assert.ok(address && typeof address !== "string");
  return {
    url: `http://127.0.0.1:${address.port}`,
    close: () => new Promise<void>((resolve, reject) => {
      http.close(error => error ? reject(error) : resolve());
      http.closeAllConnections();
    }),
  };
}

test("rejects unsafe endpoint configuration before sending credentials", () => {
  for (const url of ["http://example.com", "https://user:secret@example.com", "https://example.com?token=secret", "https://example.com/#fragment", "file:///tmp/mail"])
    assert.throws(() => new StalwartClient({ url, authorization: "Bearer secret" }));
});

test("validates the JMAP response identity and never follows redirects with credentials", async () => {
  let calls = 0;
  const fixture = await server(async (req, res) => {
    calls++;
    assert.equal(req.url, "/jmap");
    assert.equal(req.headers.authorization, "Bearer fixture-only");
    let raw = ""; for await (const chunk of req) raw += chunk;
    const request = JSON.parse(raw);
    assert.equal(request.methodCalls[0][0], "x:Domain/get");
    if (calls === 1) res.end(JSON.stringify({ methodResponses: [["x:Domain/get", { list: [{ id: "12" }] }, "webdock"]] }));
    else if (calls === 2) res.end(JSON.stringify({ methodResponses: [["x:Account/get", { list: [] }, "webdock"]] }));
    else { res.writeHead(307, { Location: `${fixture.url}/stolen` }); res.end(); }
  });
  try {
    const client = new StalwartClient({ url: fixture.url, authorization: "Bearer fixture-only", allowInsecureHttp: true });
    assert.deepEqual(await client.call("x:Domain/get", { ids: null }), { list: [{ id: "12" }] });
    await assert.rejects(client.call("x:Domain/get", { ids: null }), /invalid response/i);
    await assert.rejects(client.call("x:Domain/get", { ids: null }), /request failed/i);
    assert.equal(calls, 3);
  } finally { await fixture.close(); }
});

test("a per-object rejection is not success and provider details stay private", async () => {
  const fixture = await server((_req, res) => res.end(JSON.stringify({ methodResponses: [["x:Account/set", { notCreated: { one: { type: "invalidProperties", description: "sensitive upstream contents" } } }, "webdock"]] })));
  try {
    const client = new StalwartClient({ url: fixture.url, authorization: "Bearer fixture-only", allowInsecureHttp: true });
    await assert.rejects(client.call("x:Account/set", { create: { one: {} } }), (error: Error) => {
      assert.match(error.message, /object change rejected/i);
      assert.ok(!error.message.includes("sensitive"));
      return true;
    });
  } finally { await fixture.close(); }
});

test("a timed-out mutation is attempted once and remains uncertain", async () => {
  let calls = 0;
  const fixture = await server((_req, _res) => { calls++; });
  try {
    const client = new StalwartClient({ url: fixture.url, authorization: "Bearer fixture-only", allowInsecureHttp: true, timeoutMs: 80 });
    await assert.rejects(client.call("x:Account/set", { create: { one: {} } }), /outcome unknown/i);
    assert.equal(calls, 1);
  } finally { await fixture.close(); }
});

test("bounds streamed upstream responses, not only content-length", async () => {
  const fixture = await server((_req, res) => { res.writeHead(200); res.write(" ".repeat(524_289)); res.end(" ".repeat(524_288)); /* no length: chunked */ });
  try {
    const client = new StalwartClient({ url: fixture.url, authorization: "Bearer fixture-only", allowInsecureHttp: true });
    await assert.rejects(client.call("x:Domain/get", { ids: null }), /invalid response/i);
  } finally { await fixture.close(); }
});
