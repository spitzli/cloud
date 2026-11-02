import { withTokenIntrospection } from "../auth/introspection";
type PortalTransportConfig = {
  origin: string;
  clientID: string;
  clientSecret: string;
};
const failure = (status: number, message: string, kind = "AccessError") =>
  Response.json(
    { error: { kind, message } },
    { status, headers: { "cache-control": "no-store" } },
  );
/** Opaque browser sessions stay server-side; caller cannot supply delegation credentials. */
export async function portalRequest(
  request: Request,
  config: PortalTransportConfig,
  session: (headers: Headers) => Promise<{ accessToken: string } | null>,
  dispatch: (request: Request) => Promise<Response>,
): Promise<Response> {
  if (request.method !== "POST") return failure(405, "Use POST.");
  if (request.headers.get("origin") !== config.origin)
    return failure(403, "Invalid origin.");
  if (
    request.headers.get("content-type")?.split(";")[0].trim() !==
    "application/json"
  )
    return failure(415, "Use JSON.");
  const reader = request.body?.getReader();
  if (!reader) return failure(400, "Invalid request.");
  let size = 0;
  const chunks: Uint8Array[] = [];
  try {
    for (;;) {
      const item = await reader.read();
      if (item.done) break;
      size += item.value.byteLength;
      if (size > 2_097_152) {
        await reader.cancel();
        return failure(413, "Request is too large.");
      }
      chunks.push(item.value);
    }
  } finally {
    reader.releaseLock();
  }
  let body: unknown;
  try {
    body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    return failure(400, "Invalid request.");
  }
  if (!body || typeof body !== "object" || Array.isArray(body))
    return failure(400, "Invalid request.");
  const value = body as Record<string, unknown>;
  if (
    Object.keys(value).some((key) => !["operation", "args"].includes(key)) ||
    typeof value.operation !== "string" ||
    !Array.isArray(value.args)
  )
    return failure(400, "Invalid request.");
  return withTokenIntrospection(async () => {
    try {
      const identity = await session(request.headers);
      if (!identity)
        return failure(401, "Sign in to Studio again.", "PortalSessionMissing");
      return await dispatch(
        new Request(new URL("/api/studio", request.url), {
          method: "POST",
          headers: {
            "content-type": "application/json",
            authorization:
              "Basic " +
              Buffer.from(config.clientID + ":" + config.clientSecret).toString(
                "base64",
              ),
          },
          body: JSON.stringify({ ...value, accessToken: identity.accessToken }),
          signal: request.signal,
        }),
      );
    } catch {
      return failure(503, "Studio is temporarily unavailable.");
    }
  });
}
