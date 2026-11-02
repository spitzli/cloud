import {registerPluginTools} from '../modules/plugins/exposure';
import { withDatabaseScope } from "@webdock/database-runtime";
import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import {
  introspectMCP,
  validateMCPToken,
  mcpOrigin,
  mcpIssuer,
  mcpResource,
} from "../modules/mcp/mcp-auth";
import { WebStandardStreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/webStandardStreamableHttp.js";
import { challenge, authorizeRegistry } from "../modules/mcp/mcp-auth";
import { createRegistryMCP, mutationTools } from "../modules/mcp/mcp-server";
export async function POST(request: Request) {
  const origin = request.headers.get("origin");
  if (origin && origin !== mcpOrigin())
    return new Response(null, { status: 403 });
  let token;
  try {
    token = await introspectMCP(request);
  } catch {
    return Response.json(
      { error: "Authorization unavailable" },
      { status: 503 },
    );
  }
  const registry = validateMCPToken(token, mcpResource(), mcpIssuer());
  if (!registry) return challenge(401);
  const access = registry
    ? await authorizeRegistry(request, false, {
        authenticate: async () => registry,
      })
    : null;
  if (access instanceof Response) return access;
  if (Number(request.headers.get("content-length")) > 65536)
    return new Response(null, { status: 413 });
  const reader = request.body?.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  if (reader)
    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        size += value.byteLength;
        if (size > 65536) {
          await reader.cancel();
          return new Response(null, { status: 413 });
        }
        chunks.push(value);
      }
    } finally {
      reader.releaseLock();
    }
  const raw = Buffer.concat(chunks).toString("utf8");
  let body;
  try {
    body = JSON.parse(raw);
  } catch {
    return Response.json({ error: "Invalid JSON" }, { status: 400 });
  }
  if (Array.isArray(body))
    return Response.json(
      { error: "Batch requests are not supported" },
      { status: 400 },
    );
  const identity = registry!;
  return withDatabaseScope(
    {
      kind: "session",
      subject: identity.subject,
      sessionID: identity.sessionID,
    },
    async () => {
      const canWrite = access?.canWrite ?? false;
      if (
        body?.method === "tools/call" &&
        mutationTools.has(body?.params?.name) &&
        !canWrite
      )
        return challenge(403, "webdock:read webdock:write");
      const server = access
        ? createRegistryMCP(access.actor, canWrite)
        : new McpServer({ name: "webdock-hosting", version: "1.0.0" });
      if (access) await registerPluginTools(server, canWrite);
      const transport = new WebStandardStreamableHTTPServerTransport({
        sessionIdGenerator: undefined,
        enableJsonResponse: true,
      });
      try {
        await server.connect(transport);
        const response = await transport.handleRequest(request, {
          parsedBody: body,
        });
        const text = await response.text();
        const headers = new Headers(response.headers);
        headers.set("Cache-Control", "no-store");
        return new Response(text || null, { status: response.status, headers });
      } finally {
        await server.close();
      }
    },
  );
}
export async function GET(request: Request) {
  try {
    const token = await introspectMCP(request);
    if (
      !validateMCPToken(token, mcpResource(), mcpIssuer())
    )
      return challenge(401);
    return new Response(null, {
      status: 405,
      headers: { Allow: "POST", "Cache-Control": "no-store" },
    });
  } catch {
    return Response.json(
      { error: "Authorization unavailable" },
      { status: 503 },
    );
  }
}
