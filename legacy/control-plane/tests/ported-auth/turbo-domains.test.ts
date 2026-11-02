import test from "node:test";
import assert from "node:assert/strict";
import { TurboDomainsClient } from "../../src/modules/mail/turbo-domains";
import {
  DKIM_VALUE,
  lookupMailTxt,
  planMailRecords,
} from "../../src/modules/mail/mail-dns";
const domain = {
  id: "999999999999999999999",
  domain: "Example.COM",
  spf_verified: true,
  dkim_verified: false,
  dmarc_verified: true,
};
test("Sender-domain adapter uses fixed origin, child token, POST and exact IDs", async () => {
  const calls: string[] = [];
  const client = new TurboDomainsClient(
    { apiKey: "child-token" },
    async (url, init) => {
      assert.equal(
        url,
        `https://pro.api.serversmtp.com/api/v2/sender-domains${init?.method === "GET" ? "?page=1&limit=100" : ""}`,
      );
      assert.equal(init?.redirect, "error");
      assert.equal(
        new Headers(init?.headers).get("Authorization"),
        "child-token",
      );
      assert.equal(new Headers(init?.headers).get("consumerKey"), null);
      calls.push(init!.method!);
      if (init?.method === "POST")
        assert.deepEqual(JSON.parse(String(init.body)), {
          domain: "example.com",
        });
      return Response.json(
        init?.method === "POST" ? domain : { results: [domain] },
      );
    },
  );
  assert.equal(
    (await client.registerSenderDomain("example.com")).id,
    domain.id,
  );
  assert.equal((await client.listSenderDomains())[0].domain, "example.com");
  assert.deepEqual(calls, ["POST", "GET"]);
});
test("Malformed flags, precision loss, oversized responses and raw provider errors fail closed", async () => {
  for (const row of [
    { ...domain, spf_verified: "true" },
    { ...domain, id: 9007199254740992 },
    { ...domain, domain: "https://example.com" },
  ]) {
    await assert.rejects(
      new TurboDomainsClient({ apiKey: "child" }, async () =>
        Response.json({ results: [row] }),
      ).listSenderDomains(),
      /Invalid turboSMTP response/,
    );
  }
  let calls = 0;
  const client = new TurboDomainsClient(
    { consumerKey: "master", consumerSecret: "secret" },
    async () => {
      calls++;
      return new Response("secret raw provider failure", { status: 403 });
    },
  );
  await assert.rejects(
    client.registerSenderDomain("example.com"),
    (e) => e instanceof Error && !e.message.includes("secret"),
  );
  assert.equal(calls, 1);
  await assert.rejects(
    new TurboDomainsClient(
      { apiKey: "child" },
      async () => new Response("x".repeat(1_048_577)),
    ).listSenderDomains(),
  );
});
test("DNS plan preserves SPF authorization and strict DMARC, publishes direct provider TXT and ownership", () => {
  const records = planMailRecords("example.com", "proof", {
    root: ["v=spf1 include:existing.example -all"],
    dkim: [],
    dmarc: ["v=DMARC1; p=reject; rua=mailto:reports@example.com"],
    ownership: [],
  });
  assert.equal(records[0].name, "_webdock-mail.example.com");
  assert.equal(records[0].value, "webdock=proof");
  assert.equal(
    records[1].value,
    "v=spf1 include:spf.turbo-smtp.com include:existing.example -all",
  );
  assert.equal(records[2].type, "TXT");
  assert.equal(records[2].value, DKIM_VALUE);
  assert.equal(records[2].name, "turbo-smtp._domainkey.example.com");
  assert.equal(
    records[3].value,
    "v=DMARC1; p=reject; rua=mailto:reports@example.com",
  );
  assert.equal(records[3].action, "none");
  assert.ok(!JSON.stringify(records).includes("relaykit"));
  assert.equal(
    planMailRecords("example.com", "proof", {
      root: ["v=spf1 -all", "v=spf1 ~all"],
      dkim: [],
      dmarc: [],
      ownership: [],
    })[1].status,
    "conflict",
  );
});
test("DNS resolution uses only public HTTPS DoH", async () => {
  let calls = 0;
  const txt = await lookupMailTxt("example.com", async (url, init) => {
    assert.equal(new URL(String(url)).origin, "https://cloudflare-dns.com");
    assert.equal(init?.redirect, "error");
    calls++;
    return Response.json({
      Status: 0,
      Answer: [{ type: 16, data: '"v=spf1 " "-all"' }],
    });
  });
  assert.equal(calls, 4);
  assert.deepEqual(txt.root, ["v=spf1 -all"]);
});
test("Sender-domain deletion uses only the validated mapped ID and validates success", async () => {
  let calls = 0;
  const client = new TurboDomainsClient(
    { apiKey: "child-token" },
    async (url, init) => {
      calls++;
      assert.equal(
        url,
        "https://pro.api.serversmtp.com/api/v2/sender-domains/domain-id",
      );
      assert.equal(init?.method, "DELETE");
      return new Response(null, { status: 204 });
    },
  );
  await client.deleteSenderDomain("domain-id");
  assert.equal(calls, 1);
  await new TurboDomainsClient({ apiKey: "child" }, async () =>
    Response.json({ message: "domain_deleted" }),
  ).deleteSenderDomain("domain-id");
  await assert.rejects(client.deleteSenderDomain("../other"));
  assert.equal(calls, 1);
  await assert.rejects(
    new TurboDomainsClient({ apiKey: "child" }, async () =>
      Response.json({ error: "not removed" }),
    ).deleteSenderDomain("domain-id"),
  );
});
test("Sender-domain pagination requires a complete bounded list before proving absence", async () => {
  const client = new TurboDomainsClient({ apiKey: "child" }, async (url) => {
    const page = Number(new URL(String(url)).searchParams.get("page"));
    assert.equal(new URL(String(url)).searchParams.get("limit"), "100");
    return Response.json({
      count: 2,
      results: [{ ...domain, id: String(page) }],
    });
  });
  assert.deepEqual(
    (await client.listSenderDomains()).map((row) => row.id),
    ["1", "2"],
  );
  for (const body of [
    { count: -1, results: [] },
    { count: "2x", results: [] },
    { count: "9007199254740992", results: [] },
    { count: "-1", results: [] },
    { count: "1.0", results: [] },
    { count: 2, results: [] },
    { count: 2, results: [domain] },
  ]) {
    await assert.rejects(
      new TurboDomainsClient({ apiKey: "child" }, async () =>
        Response.json(body),
      ).listSenderDomains(),
    );
  }
  let pages = 0;
  await assert.rejects(
    new TurboDomainsClient({ apiKey: "child" }, async () =>
      Response.json({
        count: 11,
        results: [{ ...domain, id: String(++pages) }],
      }),
    ).listSenderDomains(),
  );
  assert.equal(pages, 10);
});

test("Sender-domain pagination accepts the live provider decimal-string count", async () => {
  const client = new TurboDomainsClient({ apiKey: "child" }, async (url) => {
    const page = Number(new URL(String(url)).searchParams.get("page"));
    return Response.json({
      count: "2",
      results: [{ ...domain, id: String(page) }],
    });
  });
  assert.deepEqual(
    (await client.listSenderDomains()).map((row) => row.id),
    ["1", "2"],
  );
  assert.deepEqual(
    await new TurboDomainsClient({ apiKey: "child" }, async () =>
      Response.json({ count: "0", results: [] }),
    ).listSenderDomains(),
    [],
  );
});
