import { createServer } from "node:http";
import { createHash, timingSafeEqual } from "node:crypto";
import { createPluginContext } from "@spitzli/plugin-sdk/server";
export function processJob(payload: unknown) {
  if (
    !payload ||
    typeof payload !== "object" ||
    !("message" in payload) ||
    typeof payload.message !== "string" ||
    payload.message.length > 1000
  )
    throw Error("Expected message (max 1000 characters)");
  return {
    message: payload.message.toUpperCase(),
    sha256: createHash("sha256").update(payload.message).digest("hex"),
  };
}
const runtimeToken = process.env.SPITZLI_PLUGIN_RUNTIME_TOKEN;
if (!runtimeToken || runtimeToken.length < 32)
  throw Error("Set SPITZLI_PLUGIN_RUNTIME_TOKEN (32+ characters)");
const hostURL = process.env.SPITZLI_PLUGIN_HOST_URL;
if (!hostURL) throw Error("Set SPITZLI_PLUGIN_HOST_URL");
const server = createServer(async (req, res) => {
  res.setHeader("Content-Type", "application/json");
  if (
    req.method === "GET" &&
    (req.url === "/healthz" || req.url === "/readyz")
  ) {
    res.end('{"ok":true}');
    return;
  }
  const supplied = Buffer.from(req.headers.authorization ?? "");
  const expected = Buffer.from("Bearer " + runtimeToken);
  if (
    supplied.length !== expected.length ||
    !timingSafeEqual(supplied, expected)
  ) {
    res.writeHead(401);
    res.end('{"error":"unauthorized"}');
    return;
  }
  try {
    let size = 0;
    const chunks: Buffer[] = [];
    for await (const chunk of req) {
      size += chunk.length;
      if (size > 32768) {
        res.writeHead(413);
        res.end('{"error":"too_large"}');
        return;
      }
      chunks.push(chunk);
    }
    const body = JSON.parse(Buffer.concat(chunks).toString("utf8"));
    if (req.method !== "POST") throw Error("Unsupported method");
    if (req.url === "/webdock/jobs" && body.name === "demo.process")
      res.end(JSON.stringify(processJob(body.payload)));
    else if (req.url === "/webdock/actions" && body.name === "demo.enqueue") {
      const token = req.headers["x-plugin-invocation"];
      if (typeof token !== "string") throw Error("Missing invocation");
      // The trusted host supplies this invocation token. Redis never enters the plugin.
      const ctx = createPluginContext({ hostURL, token: () => token });
      const result = await ctx.jobs.enqueue({
        name: "demo.process",
        payload: body.payload,
        idempotencyKey: body.idempotencyKey,
      });
      res.end(JSON.stringify(result));
    } else {
      res.writeHead(404);
      res.end('{"error":"unknown_handler"}');
    }
  } catch {
    res.writeHead(400);
    res.end('{"error":"invalid_request"}');
  }
});
server.listen(
  Number(process.env.PORT ?? 8080),
  process.env.HOST ?? "127.0.0.1",
  () =>
    console.log(
      JSON.stringify({ port: (server.address() as { port: number }).port }),
    ),
);
process.on("SIGTERM", () => server.close());
