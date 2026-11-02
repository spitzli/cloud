import { requireOAuthCapability } from "@webdock/auth-configuration/protocols";
import { businessOperator, getBusinessIdentity } from "./business-identity";
import { withAuthenticatedDatabase } from "../../db/database-context";

import { database } from "../../db/runtime";
import { offlineProvisioning } from "./offline";
import { currentMCPClaims, mcpResource } from "./mcp";
async function operator(headers: Headers) {
  const session = await getBusinessIdentity(headers);
  if (!session || (await businessOperator(session.user.id)).disabled)
    throw Error("Operator access required");
  return session;
}
export async function getConnections(headers: Headers) {
  return withAuthenticatedDatabase(headers, async () => {
    await operator(headers);
    requireOAuthCapability();
    return (
      await database.query(
        `SELECT "clientId",name,disabled,scopes FROM webdock_auth."oauthClient" WHERE (metadata::jsonb->>'webdock_mcp'='true' OR metadata::jsonb->>'webdock_hosting'='true') ORDER BY "createdAt" DESC LIMIT 100`,
      )
    ).rows;
  });
}
export async function registerConnection(
  headers: Headers,
  input: Record<string, string>,
) {
  return withAuthenticatedDatabase(headers, async () => {
    await operator(headers);
    requireOAuthCapability();
    const name = (input.name || "").trim(),
      uri = new URL(input.redirectURI || "");
    const loopback =
      uri.protocol === "http:" &&
      ["127.0.0.1", "localhost", "[::1]"].includes(uri.hostname);
    if (
      !name ||
      name.length > 100 ||
      (uri.protocol !== "https:" && !loopback) ||
      uri.username ||
      uri.password ||
      uri.hash
    )
      throw Error("Invalid client callback");
    const id = (
      await database.query("SELECT webdock_auth.next_snowflake() AS id")
    ).rows[0].id;
    const confidential = input.confidential === "yes",
      offline = input.offline === "yes",
      hosting = input.capability === "hosting",
      prefix = hosting ? "hosting" : "webdock";
    const scope =
      (input.write === "yes"
        ? `${prefix}:read ${prefix}:write`
        : `${prefix}:read`) + (offline ? " offline_access" : "");
    const { auth } = await import("./auth");
    const client = await offlineProvisioning.run({ clientID: id }, () =>
      auth.api.adminCreateOAuthClient({
        headers,
        body: {
          client_name: name,
          redirect_uris: [uri.toString()],
          application_type: loopback ? "native" : "web",
          scope,
          grant_types: offline
            ? ["authorization_code", "refresh_token"]
            : ["authorization_code"],
          response_types: ["code"],
          token_endpoint_auth_method: confidential
            ? "client_secret_post"
            : "none",
          require_pkce: true,
          skip_consent: false,
          metadata: hosting ? { webdock_hosting: true } : { webdock_mcp: true },
        },
      }),
    );
    try {
      await auth.api.adminLinkClientResource({
        headers,
        params: { identifier: mcpResource, client_id: client.client_id },
      });
    } catch (error) {
      await database.query(
        'UPDATE webdock_auth."oauthClient" SET disabled=true WHERE "clientId"=$1',
        [client.client_id],
      );
      throw error;
    }
    return { clientID: client.client_id, clientSecret: client.client_secret };
  });
}
export async function disableConnection(headers: Headers, id: string) {
  return withAuthenticatedDatabase(headers, async () => {
    await operator(headers);
    requireOAuthCapability();
    if (!/^[1-9][0-9]{0,18}$/.test(id)) throw Error("Invalid client");
    await database.query(
      `UPDATE webdock_auth."oauthClient" SET disabled=true WHERE "clientId"=$1 AND (metadata::jsonb->>'webdock_mcp'='true' OR metadata::jsonb->>'webdock_hosting'='true')`,
      [id],
    );
  });
}
