import { requireOAuthCapability } from "@webdock/auth-configuration/protocols";
import {
  currentMCPClaims,
  currentHostingMCPClaims,
  mcpResource,
} from "../../../../../modules/auth/mcp";
import { authDatabase as database } from "../../../../../db/runtime";

export async function POST(request: Request) {
  requireOAuthCapability();
  const { auth } = await import("../../../../../modules/auth/auth");
  // Native OAuth introspection authenticates the resource-server client and
  // verifies token signature, audience linkage, expiration and session revocation.
  const result = await auth.handler(
    new Request(new URL("/api/auth/oauth2/introspect", request.url), request),
  );
  if (!result.ok) return result;
  const token = await result.json();
  const audiences = Array.isArray(token.aud) ? token.aud : [token.aud];
  if (
    !token.active ||
    typeof token.sid !== "string" ||
    !token.sid ||
    !audiences.includes(mcpResource) ||
    typeof token.sub !== "string"
  )
    return Response.json(
      { active: false },
      { headers: { "Cache-Control": "no-store" } },
    );
  const { rows } = await database.query(
    'SELECT metadata FROM webdock_auth."oauthClient" WHERE "clientId"=$1 AND NOT disabled',
    [token.client_id],
  );
  const metadata =
    typeof rows[0]?.metadata === "string"
      ? JSON.parse(rows[0].metadata)
      : rows[0]?.metadata;
  const claims =
    metadata?.webdock_hosting === true
      ? await currentHostingMCPClaims(token.sub)
      : metadata?.webdock_mcp === true
        ? await currentMCPClaims(token.sub)
        : { disabled: true };
  return Response.json(
    claims.disabled ? { active: false } : { ...token, ...claims },
    { headers: { "Cache-Control": "no-store" } },
  );
}
