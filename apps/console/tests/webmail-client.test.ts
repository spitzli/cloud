import assert from "node:assert/strict";
import { test } from "node:test";
import { openWebmail, WebmailError } from "../src/webmail-client";
import { foldersSchema, orderFolders } from "../src/webmail-contracts";

const origin = "https://mail.example.test";
const launch = () => Promise.resolve({
  data: {
    code: "a".repeat(43), gatewayOrigin: origin, customerID: "101", mailboxID: "201",
    expiresAt: new Date(Date.now() + 50_000).toISOString(),
  },
});
const href = (url: string | URL | Request) => url instanceof Request ? url.url : url.toString();
function exchanged(customerID = "101") {
  return { scopeID: "301", customerID, mailboxID: "201", expiresAt: new Date(Date.now() + 600_000).toISOString() };
}
await test("Webmail sends one-use code only in an exact-origin POST and exposes no gateway bearer", async () => {
  const calls: { url: string; options: RequestInit | undefined }[] = [];
  const at = (index: number) => { const call = calls[index]; assert.ok(call); return call; };
  const fetcher: typeof fetch = (url, options) => {
    calls.push({ url: url instanceof Request ? url.url : url.toString(), options });
    return Promise.resolve(Response.json(exchanged()));
  };
  const connection = await openWebmail(launch, origin, "101", "201", new AbortController().signal, fetcher);
  assert.equal(at(0).url, `${origin}/session`);
  assert.equal(at(0).options?.method, "POST");
  assert.equal(at(0).options?.credentials, "include");
  assert.equal(at(0).options?.redirect, "error");
  assert.equal(at(0).options?.body, JSON.stringify({ code: "a".repeat(43) }));
  assert.equal(JSON.stringify(connection).includes("a".repeat(43)), false);
  connection.close();
  assert.equal(at(1).url, `${origin}/s/301/logout`);
});
await test("Webmail refuses mismatched or non-HTTPS launch destinations before disclosing the code", async () => {
  for (const destination of ["http://mail.example.test", "https://evil.test", `${origin}/extra`, `${origin}?code=x`]) {
    let requests = 0;
    await assert.rejects(openWebmail(async () => {
      const value = await launch();
      value.data.gatewayOrigin = destination;
      return value;
    }, origin, "101", "201", new AbortController().signal, () => {
      requests++;
      return Promise.resolve(Response.json(exchanged()));
    }));
    assert.equal(requests, 0);
  }
});
await test("cross-mailbox exchange and expired launch are rejected", async () => {
  await assert.rejects(openWebmail(launch, origin, "101", "201", new AbortController().signal, () => Promise.resolve(Response.json(exchanged("102")))));
  let requests = 0;
  await assert.rejects(openWebmail(async () => {
    const value = await launch();
    value.data.expiresAt = new Date(Date.now() - 1).toISOString();
    return value;
  }, origin, "101", "201", new AbortController().signal, () => {
    requests++;
    return Promise.resolve(Response.json(exchanged()));
  }));
  assert.equal(requests, 0);
});
await test("denial retires the connection and prevents any further mailbox request", async () => {
  let requests = 0;
  const connection = await openWebmail(launch, origin, "101", "201", new AbortController().signal, (url) => {
    requests++;
    return Promise.resolve(href(url).endsWith("/session") ? Response.json(exchanged()) : Response.json({}, { status: 403 }));
  });
  let retired = false;
  connection.onDenied(() => { retired = true; });
  await assert.rejects(connection.call({ action: "folders" }, foldersSchema), (error: unknown) => error instanceof WebmailError && error.denied);
  assert.equal(retired, true);
  const after = requests;
  await assert.rejects(connection.call({ action: "folders" }, foldersSchema));
  assert.equal(requests, after);
});
await test("a response arriving after retirement cannot publish mailbox data", async () => {
  let release: ((response: Response) => void) | undefined;
  const connection = await openWebmail(launch, origin, "101", "201", new AbortController().signal, (url) => {
    if (href(url).endsWith("/session")) return Promise.resolve(Response.json(exchanged()));
    if (href(url).endsWith("/logout")) return Promise.resolve(Response.json({ ok: true }));
    return new Promise<Response>((resolve) => { release = resolve; });
  });
  const pending = connection.call({ action: "folders" }, foldersSchema);
  connection.close();
  assert.ok(release);
  release(Response.json({ data: { list: [{ id: "private", name: "Private folder", unreadEmails: 0 }] } }));
  await assert.rejects(pending);
});
await test("browser fetch is invoked without a WebmailConnection receiver for mailbox requests and logout", async () => {
  const receivers: unknown[] = [];
  const fetcher: typeof fetch = function (this: unknown, url) {
    receivers.push(this);
    return Promise.resolve(href(url).endsWith("/session") ? Response.json(exchanged()) : Response.json({ data: { list: [] } }));
  };
  const connection = await openWebmail(launch, origin, "101", "201", new AbortController().signal, fetcher);
  await connection.call({ action: "folders" }, foldersSchema);
  connection.close();
  assert.equal(receivers.length, 3);
  assert.deepEqual(receivers, [undefined, undefined, undefined]);
});
await test("folders follow mailbox roles before names", () => {
  const names = orderFolders([
    { id: "j", name: "Junk Mail", role: "junk", unreadEmails: 0 },
    { id: "z", name: "Zeta", role: null, unreadEmails: 0 },
    { id: "s", name: "Sent Items", role: "sent", unreadEmails: 0 },
    { id: "a", name: "Alpha", unreadEmails: 0 },
    { id: "t", name: "Deleted Items", role: "trash", unreadEmails: 0 },
    { id: "i", name: "Inbox", role: "inbox", unreadEmails: 0 },
    { id: "d", name: "Drafts", role: "drafts", unreadEmails: 0 },
  ]).map((folder) => folder.name);
  assert.deepEqual(names, ["Inbox", "Drafts", "Sent Items", "Junk Mail", "Deleted Items", "Alpha", "Zeta"]);
});
