import { test, expect } from "bun:test";
import { tenantPreview } from "../src/modules/portal/preview";
const origin = "https://studio.example.test";
const request = (
  body = "customerID=123",
  headers: Record<string, string> = {},
) =>
  new Request(origin + "/api/tenant-preview/start", {
    method: "POST",
    headers: {
      origin,
      "content-type": "application/x-www-form-urlencoded",
      ...headers,
    },
    body,
  });
test("preview alias rejects foreign origin, duplicate target and oversized body before dispatch", async () => {
  let calls = 0;
  const call = async <T>() => {
    calls++;
    return {} as T;
  };
  const foreign = await tenantPreview(
    request("", { origin: "https://foreign.invalid", "accept-language": "de" }),
    "start",
    origin,
    call,
  );
  expect(foreign.status).toBe(403);
  expect(await foreign.text()).toBe("Ungültiger Ursprung.");
  expect(
    (
      await tenantPreview(
        request("customerID=123&customerID=456"),
        "start",
        origin,
        call,
      )
    ).status,
  ).toBe(400);
  expect(
    (await tenantPreview(request("x".repeat(4097)), "start", origin, call))
      .status,
  ).toBe(413);
  expect(calls).toBe(0);
});
test("preview alias retains operation arguments and same-origin redirects", async () => {
  const calls: unknown[] = [];
  const call = async <T>(operation: string, args?: unknown[]) => {
    calls.push([operation, args]);
    return { customerID: "123" } as T;
  };
  const start = await tenantPreview(request(), "start", origin, call);
  expect(start.status).toBe(303);
  expect(start.headers.get("location")).toBe(origin + "/tenants/123");
  expect(calls).toEqual([["startTenantPreview", ["123"]]]);
  const exit = await tenantPreview(request(""), "exit", origin, call);
  expect(exit.headers.get("location")).toBe(origin + "/tenants/123");
  const failure = await tenantPreview(request(), "start", origin, async () => {
    throw Error("private-provider-secret");
  });
  expect(failure.status).toBe(403);
  expect(await failure.text()).not.toContain("private-provider-secret");
});
