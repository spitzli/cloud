import { RegistryError } from "../registry/service";
import { withDatabaseScope } from "@webdock/database-runtime";
import { currentRegistryActor } from "../integrations/operator";
import { POST as nativeIntrospection } from "../../protocols/legacy-auth/api/mcp/introspect/route";
import type { RegistryActor } from "../registry/compat";
export const mcpOrigin = () =>
  process.env.WEBDOCK_STUDIO_ORIGIN || "http://localhost:3120";
export const mcpResource = () => `${mcpOrigin()}/api/mcp`;
export const mcpIssuer = () =>
  process.env.WEBDOCK_AUTH_ISSUER || "https://auth.webdock.dev/api/auth";
export function challenge(status: 401 | 403, scope = "webdock:read") {
  return Response.json(
    {
      error: status === 401 ? "Authentication required" : "Insufficient scope",
    },
    {
      status,
      headers: {
        "Cache-Control": "no-store",
        "WWW-Authenticate": `Bearer resource_metadata="${mcpOrigin()}/.well-known/oauth-protected-resource/api/mcp", scope="${scope}"${status === 403 ? ', error="insufficient_scope"' : ""}`,
      },
    },
  );
}
export function validateMCPToken(
  value: unknown,
  resource: string,
  issuer: string,
) {
  if (!value || typeof value !== "object") return null;
  const token = value as Record<string, unknown>;
  const aud = Array.isArray(token.aud) ? token.aud : [token.aud];
  if (
    token.active !== true ||
    token.disabled === true ||
    token.webdock_role !== "operator" ||
    token.iss !== issuer ||
    !aud.includes(resource) ||
    typeof token.sub !== "string" ||
    typeof token.sid !== "string" ||
    !token.sid ||
    typeof token.exp !== "number" ||
    !Number.isFinite(token.exp) ||
    token.exp <= Date.now() / 1000 ||
    typeof token.scope !== "string" ||
    token.token_type !== "Bearer"
  )
    return null;
  return {
    subject: token.sub,
    sessionID: token.sid as string,
    scopes: new Set(token.scope.split(" ")),
  };
}
export async function introspectMCP(request: Request) {
  const authorization = request.headers.get("authorization");
  if (!authorization?.startsWith("Bearer ") || authorization.length > 16384)
    return null;
  const clientID = process.env.WEBDOCK_SSO_CLIENT_ID,
    secret = process.env.WEBDOCK_SSO_CLIENT_SECRET;
  if (!clientID || !secret)
    throw Error("MCP authentication configuration unavailable");
  const response = await nativeIntrospection(
    new Request(new URL("/api/mcp/introspect", mcpIssuer()), {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: new URLSearchParams({
        token: authorization.slice(7),
        client_id: clientID,
        client_secret: secret,
      }),
      cache: "no-store",
      signal: AbortSignal.timeout(10000),
    }),
  );
  if (response.status >= 500 || response.status === 429)
    throw Error("MCP introspection unavailable");
  if (!response.ok) return null;
  return response.json();
}

export async function authenticateMCP(request: Request) {
  return validateMCPToken(
    await introspectMCP(request),
    mcpResource(),
    mcpIssuer(),
  );
}

export async function authorizeRegistry(
  request: Request,
  write: boolean,
  dependencies: { authenticate?: typeof authenticateMCP } = {},
) {
  const origin = request.headers.get("origin");
  if (origin && origin !== mcpOrigin())
    return Response.json(
      { error: "Invalid origin" },
      { status: 403, headers: { "Cache-Control": "no-store" } },
    );
  let identity;
  try {
    identity = await (dependencies.authenticate || authenticateMCP)(request);
  } catch {
    return Response.json(
      { error: "Authorization unavailable" },
      { status: 503, headers: { "Cache-Control": "no-store" } },
    );
  }
  if (!identity) return challenge(401);
  if (
    !identity.scopes.has("webdock:read") ||
    (write && !identity.scopes.has("webdock:write"))
  )
    return challenge(
      403,
      write ? "webdock:read webdock:write" : "webdock:read",
    );
  try {
    const actor = await withDatabaseScope(
      {
        kind: "session",
        subject: identity.subject,
        sessionID: identity.sessionID,
      },
      () => currentRegistryActor(),
    );
    actor.accessToken = request.headers.get("authorization")?.slice(7);
    return { actor, canWrite: identity.scopes.has("webdock:write") };
  } catch (error) {
    const denied =
      error instanceof RegistryError && [401, 403].includes(error.status);
    return Response.json(
      {
        error: denied
          ? "Operator access required"
          : "Authorization unavailable",
      },
      { status: denied ? 403 : 503, headers: { "Cache-Control": "no-store" } },
    );
  }
}
