import { expect, test } from "bun:test";
import { portalRequest } from "../src/modules/portal/transport";
const origin = "https://studio.webdock.dev";
test("portal rejects missing session and cross-origin calls before dispatch", async () => {
  const config = { origin, clientID: "studio", clientSecret: "fixture-secret" };
  const missing = await portalRequest(
    new Request(origin + "/api/portal/studio", {
      method: "POST",
      headers: { origin, "content-type": "application/json" },
      body: '{"operation":"session","args":[]}',
    }),
    config,
    async () => null,
    async () => {
      throw Error("unauthorized dispatch");
    },
  );
  expect(missing.status).toBe(401);
  const foreign = await portalRequest(
    new Request(origin + "/api/portal/studio", {
      method: "POST",
      headers: {
        origin: "https://evil.invalid",
        "content-type": "application/json",
      },
      body: "{}",
    }),
    config,
    async () => {
      throw Error("untrusted origin");
    },
    async () => {
      throw Error("dispatch");
    },
  );
  expect(foreign.status).toBe(403);
});
test("portal supplies server-owned delegation and does not trust browser access tokens", async () => {
  const config = { origin, clientID: "studio", clientSecret: "fixture-secret" };
  const request = (body: string) =>
    new Request(origin + "/api/portal/studio", {
      method: "POST",
      headers: { origin, "content-type": "application/json" },
      body,
    });
  const session = async () => ({ accessToken: "verified-token" });
  const dispatch = async (req: Request) => {
    const body = await req.json();
    if (
      body.accessToken !== "verified-token" ||
      req.headers.get("authorization") !==
        "Basic " + btoa("studio:fixture-secret")
    )
      return new Response(null, { status: 403 });
    return Response.json({ data: { operation: body.operation } });
  };
  const result = await portalRequest(
    request('{"operation":"session","args":[]}'),
    config,
    session,
    dispatch,
  );
  expect(await result.json()).toEqual({ data: { operation: "session" } });
  const forged = await portalRequest(
    request('{"operation":"session","args":[],"accessToken":"forged"}'),
    config,
    session,
    dispatch,
  );
  expect(forged.status).toBe(400);
});
