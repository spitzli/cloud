import test from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { ApiClient } from "../src/api";
import { checkedTransfer, MAX_BINARY_BYTES, UploadUncertainError, type TransferDescriptor } from "../src/plugins/storage";
const data = new TextEncoder().encode("owned binary proof");
const hash = createHash("sha256").update(data).digest("hex");
const options = () => ({ signal: new AbortController().signal, isCurrent: () => true });
const config = { apiOrigin: "https://console.example", authOrigin: "https://account.example", enabledFeatures: [] };
function descriptor(method: "GET" | "PUT" = "GET"): TransferDescriptor {
  const id = "af91cd04-c775-42a8-9775-42f2ec158691";
  return { version: 1, id, path: "/api/console/plugins/storage/" + id, method, expiresAt: Math.floor(Date.now() / 1000) + 60, exactBytes: data.length, expectedSha256: hash, attachment: { filename: "proof.txt", contentType: "text/plain" } };
}
function response(body: BodyInit | ReadableStream<Uint8Array> = data, headers: Record<string, string> = {}) { return new Response(body, { headers: { "content-type": "text/plain", ...headers } }); }
await test("descriptor rejects external/query/encoded paths, mismatched ID/method, sizes, expiry, private fields and unsafe attachments before I/O", async () => {
  let calls = 0; const api = new ApiClient(config, () => { calls++; return Promise.resolve(response()); });
  const valid = descriptor();
  const invalid: unknown[] = [
    { ...valid, path: "https://provider.invalid/object" }, { ...valid, path: valid.path + "?url=target" }, { ...valid, path: valid.path + "#x" },
    { ...valid, path: valid.path.replace("/storage/", "/storage/%2e%2e/") }, { ...valid, id: "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb" }, { ...valid, method: "PUT" },
    { ...valid, exactBytes: MAX_BINARY_BYTES + 1 }, { ...valid, exactBytes: -1 }, { ...valid, expiresAt: 1 }, { ...valid, expiresAt: Math.floor(Date.now() / 1000) + 301 },
    { ...valid, url: "private-signed-target" }, { ...valid, headers: { authorization: "private" } },
    { ...valid, attachment: { filename: "../unsafe", contentType: "text/plain" } }, { ...valid, attachment: { filename: "proof", contentType: "text/html" } },
  ];
  for (const value of invalid) await assert.rejects(api.downloadBinary(value as TransferDescriptor, options()));
  assert.equal(calls, 0); assert.throws(() => checkedTransfer({ ...descriptor("PUT"), expectedSha256: undefined }, "PUT"));
});
await test("binary GET uses the configured fetcher detached, credentials, no-store and redirect error; exact known SHA precedes Blob return", async () => {
  const fetcher: typeof fetch = function (this: unknown, url, init) {
    assert.equal(this, undefined); assert.equal(url instanceof Request ? url.url : url.toString(), config.apiOrigin + descriptor().path); assert.equal(init?.method, "GET");
    assert.equal(init.credentials, "include"); assert.equal(init.cache, "no-store"); assert.equal(init.redirect, "error"); assert.equal(init.body, undefined);
    return Promise.resolve(response(data, { "content-length": String(data.length) }));
  };
  const blob = await new ApiClient(config, fetcher).downloadBinary(descriptor(), options());
  assert.equal(blob.type.split(";")[0], "text/plain"); assert.deepEqual(new Uint8Array(await blob.arrayBuffer()), data);
});
await test("download rejects wrong known hash, truncated/oversized streams, length headers and content type", async () => {
  const cases = [response(new Uint8Array(data.length)), response(data.slice(0, -1)), response(new Uint8Array(data.length + 1)), response(data, { "content-length": "999" }), response(data, { "content-type": "text/html" }), response(data, { "content-encoding": "gzip" })];
  for (const reply of cases) await assert.rejects(new ApiClient(config, () => Promise.resolve(reply)).downloadBinary(descriptor(), options()));
  const unknown = descriptor(); delete unknown.expectedSha256;
  assert.equal((await new ApiClient(config, () => Promise.resolve(response())).downloadBinary(unknown, options())).size, data.length);
});
await test("a scope change cancels a stalled response and stale bytes never return", async () => {
  let current = true, cancelled = false;
  const stream = new ReadableStream<Uint8Array>({ start(controller) { controller.enqueue(data.slice(0, 2)); }, cancel() { cancelled = true; } });
  const pending = new ApiClient(config, () => Promise.resolve(response(stream))).downloadBinary(descriptor(), { signal: new AbortController().signal, isCurrent: () => current });
  await new Promise(resolve => setTimeout(resolve, 10)); current = false;
  await assert.rejects(pending, /cancel/i); assert.equal(cancelled, true);
});
await test("AbortSignal and expiry interrupt stalled fetches even when a fetcher ignores the signal", async () => {
  for (const mode of ["abort", "expiry"] as const) {
    const controller = new AbortController(), d = descriptor(); if (mode === "expiry") d.expiresAt = Math.floor(Date.now() / 1000) + 1;
    const pending = new ApiClient(config, () => new Promise<Response>(() => { /* Deliberately never resolves or cooperates with cancellation. */ })).downloadBinary(d, { signal: controller.signal, isCurrent: () => true });
    if (mode === "abort") controller.abort(); await assert.rejects(pending, /cancel|expired/i);
  }
});
await test("already aborted or retired scopes make no binary request", async () => {
  let calls = 0; const api = new ApiClient(config, () => { calls++; return Promise.resolve(response()); });
  const controller = new AbortController(); controller.abort();
  await assert.rejects(api.downloadBinary(descriptor(), { signal: controller.signal, isCurrent: () => true }));
  await assert.rejects(api.downloadBinary(descriptor(), { signal: new AbortController().signal, isCurrent: () => false }));
  assert.equal(calls, 0);
});
await test("upload sends a raw Blob once, validates hash/size and never automatically retries an uncertain outcome", async () => {
  let calls = 0;
  const api = new ApiClient(config, async (_url, init) => { calls++; assert(init?.body instanceof Blob); assert.equal(await init.body.text(), new TextDecoder().decode(data)); throw Error("connection lost after commit"); });
  await assert.rejects(api.uploadBinary(descriptor("PUT"), new Blob([data]), options()), UploadUncertainError); assert.equal(calls, 1);
  await assert.rejects(api.uploadBinary(descriptor("PUT"), new Blob(["bad"]), options())); assert.equal(calls, 1);
  const good = new ApiClient(config, () => Promise.resolve(Response.json({ bytes: data.length, sha256: hash, etag: '"opaque"' })));
  assert.equal((await good.uploadBinary(descriptor("PUT"), new Blob([data]), options())).sha256, hash);
});
