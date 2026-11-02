import {
  currentTrustedBrowserReference,
  currentConsolePrincipal,
  storedConsoleIdentity,
  resolveSessionReference,
} from "../auth/principal";
import { currentHostingMCPClaims, hostingScopes } from "../auth/mcp";
import { requireInstalledOperation } from "../../installation";
import { withHostingDatabase } from "../../db/database-context";
import { transaction } from "../hosting/db";
import { executeDatabase } from "../databases/service";
import { HostingError, type HostingActor } from "@webdock/hosting-contracts";
import { hostingBridge, executeHostingBridgeCommand } from "../hosting/bridge";
import { databaseBridge } from "../databases/http";
import { delegatedToken, nativeHeaders } from "./request-context";
export async function hostingCall(
  command: unknown,
  kind: "hosting" | "database" = "hosting",
) {
  requireInstalledOperation(
    kind === "database" ? "database.command" : "hosting.command",
    [command],
  );
  const principal = currentConsolePrincipal();
  if (principal) {
    const actor: HostingActor = {
      consoleIdentity: storedConsoleIdentity()!,
      subject: principal.userID,
      sessionID: principal.sessionID,
      source: "studio",
      scopes: hostingScopes,
    };
    return kind === "hosting"
      ? executeHostingBridgeCommand(actor, command)
      : withHostingDatabase(actor, () =>
          transaction((db) => executeDatabase(db, actor, command)),
        );
  }
  const trusted = currentTrustedBrowserReference();
  if (trusted) {
    const session = await resolveSessionReference(nativeHeaders());
    if (
      !session ||
      session.subject !== trusted.subject ||
      session.sessionID !== trusted.sessionID ||
      (await currentHostingMCPClaims(session.subject)).disabled
    )
      throw new HostingError(401, "Verified Studio session is unavailable.");
    requireInstalledOperation(
      kind === "database" ? "database.command" : "hosting.command",
      [command],
    );
    const actor: HostingActor = {
      ...session,
      source: "studio",
      scopes: hostingScopes,
    };
    return kind === "database"
      ? withHostingDatabase(actor, () =>
          transaction((db) => executeDatabase(db, actor, command)),
        )
      : executeHostingBridgeCommand(actor, command);
  }
  const id = process.env.WEBDOCK_SSO_CLIENT_ID,
    secret = process.env.WEBDOCK_SSO_CLIENT_SECRET;
  if (!id || !secret)
    throw new HostingError(503, "Hosting authentication is unavailable.");
  const request = new Request(
    new URL(
      kind === "hosting" ? "/api/hosting/bridge" : "/api/databases/bridge",
      process.env.BETTER_AUTH_URL || "http://localhost:3125",
    ),
    {
      method: "POST",
      headers: {
        "content-type": "application/json",
        authorization:
          "Basic " + Buffer.from(id + ":" + secret).toString("base64"),
      },
      body: JSON.stringify({ accessToken: delegatedToken(), command }),
    },
  );
  const response = await (kind === "hosting"
    ? hostingBridge(request)
    : databaseBridge(request));
  const result = (await response.json()) as { data?: unknown; error?: string };
  if (!response.ok)
    throw new HostingError(
      response.status,
      result.error || "Hosting is unavailable.",
    );
  return result.data;
}
