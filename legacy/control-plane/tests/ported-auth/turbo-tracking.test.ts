import test from "node:test";
import assert from "node:assert/strict";
import {
  TrackingClient,
  TrackingProviderError,
} from "../../src/modules/mail/turbo-tracking";
const domain = {
  id: 12,
  domain_name: "links.example.com",
  verification_domain: "verify.example.com",
  verified: false,
  ssl: false,
  enabled: false,
  default: false,
};
test("Tracking adapter is fixed-host child-token-only and uses confirmed endpoint methods", async () => {
  const calls: { path: string; method: string; body?: string }[] = [];
  const client = new TrackingClient("child-token", async (url, init) => {
    const parsed = new URL(String(url));
    assert.equal(parsed.origin, "https://pro.api.serversmtp.com");
    assert.equal(init?.redirect, "error");
    const headers = new Headers(init?.headers);
    assert.equal(headers.get("Authorization"), "child-token");
    assert.equal(headers.has("consumerKey"), false);
    const path = parsed.pathname.replace("/api/v2", ""),
      method = init?.method || "GET";
    calls.push({ path, method, body: init?.body as string | undefined });
    if (path === "/tools/link_branding" && method === "POST")
      return Response.json({ id: 12 });
    if (path === "/tools/link_branding/verify/12")
      return Response.json({ message: "success" });
    if (method !== "GET") return new Response(null, { status: 204 });
    if (path === "/tools/link_branding")
      return Response.json({ count: 1, results: [domain] });
    if (path === "/tools/link_branding/12") return Response.json(domain);
    if (path === "/tools/link_branding/domains_info")
      return Response.json({
        all_domains_disabled: true,
        match_sender: false,
        no_default_domain: true,
      });
    if (path === "/tools")
      return Response.json([
        { id: "link_branding", enabled: false },
        { id: "clickTracking", enabled: false },
        { id: "openingTracking", enabled: false },
      ]);
    return Response.json({
      id: path.split("/").at(-1),
      forced: false,
      enabled: false,
      settings: [],
    });
  });
  assert.equal(await client.create("example.com", "links.example.com"), "12");
  assert.equal(calls.length, 1, "Creating does not enable tracking");
  assert.equal(JSON.parse(calls[0].body!).subdomain, "links.example.com");
  assert.equal((await client.list())[0].id, "12");
  assert.equal((await client.get("12")).verified, false);
  await client.verify("12");
  await client.domainSetting("12", "default", false);
  await client.setting("opening", true);
  await client.setting("click", false);
  await client.remove("12");
  const settings = await client.settings();
  assert.equal(settings.custom, false);
  assert.equal(settings.click.enabled, false);
  assert.equal(settings.click.forced, null);
  assert.ok(
    !calls.some(
      (call) =>
        call.method === "GET" &&
        ["/tools/clickTracking", "/tools/openingTracking"].includes(call.path),
    ),
  );
  assert.ok(
    calls.some(
      (c) =>
        c.path === "/tools/link_branding/default/12/disable" &&
        c.method === "PUT",
    ),
  );
  assert.ok(
    calls.some(
      (c) => c.path === "/tools/openingTracking/enable" && c.method === "PUT",
    ),
  );
  assert.ok(
    calls.some(
      (c) => c.path === "/tools/clickTracking/disable" && c.method === "PUT",
    ),
  );
});
test("Tracking rejects malformed, oversized and error responses without leaking response text or retrying writes", async () => {
  for (const response of [
    Response.json({ ...domain, verified: "true" }),
    Response.json({ ...domain, id: "../x" }),
    Response.json({ ...domain, domain_name: "https://bad.com" }),
    new Response("secret upstream", { status: 401 }),
    new Response(" ".repeat(1048577)),
  ]) {
    let calls = 0;
    const client = new TrackingClient("child", async () => {
      calls++;
      return response;
    });
    await assert.rejects(
      client.get("12"),
      (error) =>
        error instanceof TrackingProviderError &&
        !error.message.includes("secret upstream"),
    );
    assert.equal(calls, 1);
  }
  const client = new TrackingClient("child", async () =>
    Response.json({ message: "pending" }),
  );
  await assert.rejects(client.verify("12"));
  await assert.rejects(client.get("../../admin"));
  await assert.rejects(client.create("example.com", "links.evil.com"));
  assert.throws(() => new TrackingClient("bad\nheader"));
});

test("Aggregate tracking state fails closed on malformed flags and preserves missing custom state as unknown", async () => {
  const info = {
    all_domains_disabled: true,
    match_sender: false,
    no_default_domain: true,
  };
  for (const tools of [
    [{ id: "clickTracking", enabled: "true" }],
    [{ id: "clickTracking", enabled: false }],
    { results: [] },
  ]) {
    const client = new TrackingClient("child", async (url) =>
      Response.json(String(url).endsWith("domains_info") ? info : tools),
    );
    await assert.rejects(client.settings());
  }
  const client = new TrackingClient("child", async (url) =>
    Response.json(
      String(url).endsWith("domains_info")
        ? info
        : [
            { id: "clickTracking", enabled: false },
            { id: "openingTracking", enabled: false },
          ],
    ),
  );
  assert.equal((await client.settings()).custom, null);
});

test("Verification DNS names accept underscores while web hostnames do not", async () => {
  const client = new TrackingClient("child", async () =>
    Response.json({
      ...domain,
      verification_domain: "_verify._domainkey.example.com",
    }),
  );
  assert.equal(
    (await client.get("12")).verification_domain,
    "_verify._domainkey.example.com",
  );
  const invalid = new TrackingClient("child", async () =>
    Response.json({ ...domain, domain_name: "_links.example.com" }),
  );
  await assert.rejects(invalid.get("12"));
});
test("Successful HTTP status cannot hide provider error bodies on write endpoints", async () => {
  for (const status of ["error", "ERROR"]) {
    let calls = 0;
    const client = new TrackingClient("child", async () => {
      calls++;
      return Response.json({ status, message: "secret upstream detail" });
    });
    await assert.rejects(
      client.setting("custom", true),
      (error) =>
        error instanceof TrackingProviderError &&
        !error.message.includes("secret upstream detail"),
    );
    assert.equal(calls, 1);
  }
});

test("Tracking list paginates completely and rejects incomplete or changing collections", async () => {
  const domains = Array.from({ length: 101 }, (_, i) => ({
    ...domain,
    id: i + 1,
    domain_name: `links${i}.example.com`,
  }));
  const pages: number[] = [];
  const client = new TrackingClient("child", async (url) => {
    const parsed = new URL(String(url)),
      page = Number(parsed.searchParams.get("page"));
    assert.equal(parsed.searchParams.get("limit"), "100");
    pages.push(page);
    return Response.json({
      count: domains.length,
      results: domains.slice((page - 1) * 100, page * 100),
    });
  });
  assert.equal((await client.list()).length, 101);
  assert.deepEqual(pages, [1, 2]);
  for (const mode of ["incomplete", "changed", "duplicates"]) {
    const invalid = new TrackingClient("child", async (url) => {
      const page = Number(new URL(String(url)).searchParams.get("page"));
      return Response.json({
        count: page === 2 && mode === "changed" ? 102 : 101,
        results:
          page === 1
            ? domains.slice(0, 100)
            : mode === "duplicates"
              ? [domains[0]]
              : [],
      });
    });
    await assert.rejects(invalid.list());
  }
});
