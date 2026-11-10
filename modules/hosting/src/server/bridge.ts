import type { DatabaseServices } from "@spitzli/database-contracts/runtime-ports";
import type { HostingPorts } from "./ports";
import { z } from "zod";
import { gitOAuthCommandSchema } from "@spitzli/hosting-contracts/git-oauth";
import {
  commandSchema,
  resourceID,
  HostingError,
  hostingError,
  type HostingActor,
} from "@spitzli/hosting-contracts";
export type Dependencies = {
  introspectToken(
    accessToken: string,
    clientID: string,
    secret: string,
  ): Promise<Response>;
  requireHostingCommand: HostingPorts["requireHostingCommand"];
  vercelSettings: HostingPorts["vercelSettings"];
  displayDatabaseCredentials: DatabaseServices["displayDatabaseCredentials"];
  executeGitOAuth: ReturnType<
    typeof import("./git-oauth").createHostingGitOauth
  >["executeGitOAuth"];
  database: HostingPorts["database"];
  mcpResource: HostingPorts["mcpResource"];
  currentHostingMCPClaims: HostingPorts["currentHostingMCPClaims"];
  executeHosting: ReturnType<
    typeof import("./service").createHostingService
  >["executeHosting"];
  issueEnrollment: ReturnType<
    typeof import("./clusters").createHostingClusters
  >["issueEnrollment"];
  readHostingJSON: ReturnType<
    typeof import("./http").createHostingHttp
  >["readHostingJSON"];
  requireHostingEnvironment: ReturnType<
    typeof import("./http").createHostingHttp
  >["requireHostingEnvironment"];
  env: HostingPorts["env"];
};

export function createHostingBridge(dependencies: Dependencies) {
  const bodySchema = z
    .object({
      accessToken: z.string().min(1).max(16384),
      // Confidential Studio server context, never accepted by public command schemas.
      vercelRuntime: z
        .object({
          clientID: z.string().min(1).max(256),
          clientSecret: z.string().min(1).max(8192),
          slug: z.string().min(1).max(100),
          origin: z.string().url().max(300),
          platformTeam: z.string().max(160),
        })
        .strict()
        .optional(),
      command: z.union([
        commandSchema,
        gitOAuthCommandSchema,
        z
          .object({
            action: z.literal("databases.credentials.display"),
            databaseID: resourceID,
          })
          .strict(),
        z
          .object({
            action: z.literal("enrollment.display"),
            enrollmentID: resourceID,
          })
          .strict(),
      ]),
    })
    .strict();
  async function hostingIdentity(
    request: Request,
    accessToken: string,
  ): Promise<HostingActor> {
    const clientID = dependencies.env.WEBDOCK_STUDIO_CLIENT_ID,
      basic = request.headers.get("authorization") || "";
    if (
      !clientID ||
      basic.length > 16384 ||
      !/^Basic [A-Za-z0-9+/]+=*$/i.test(basic)
    )
      throw new HostingError(401, "Studio authentication required.");
    const credentials = Buffer.from(basic.slice(6), "base64").toString("utf8"),
      split = credentials.indexOf(":");
    if (
      credentials.slice(0, split) !== clientID ||
      !credentials.slice(split + 1)
    )
      throw new HostingError(401, "Studio authentication required.");
    const response = await dependencies.introspectToken(
      accessToken,
      clientID,
      credentials.slice(split + 1),
    );
    if (!response.ok) {
      if ([400, 401, 403].includes(response.status))
        throw new HostingError(401, "Sign in again.");
      throw new HostingError(
        response.status === 429 ? 429 : 503,
        "Hosting is unavailable. Check the current state before retrying.",
      );
    }
    const token = await response.json();
    if (
      token.active !== true ||
      token.disabled === true ||
      typeof token.sub !== "string" ||
      typeof token.sid !== "string" ||
      !token.sid ||
      typeof token.exp !== "number" ||
      !Number.isFinite(token.exp) ||
      token.exp * 1000 <= Date.now()
    )
      throw new HostingError(401, "Sign in again.");
    if (token.client_id === clientID)
      return {
        subject: token.sub,
        sessionID: token.sid,
        source: "studio",
        scopes: ["hosting:read", "hosting:write"],
      };
    const audiences = Array.isArray(token.aud) ? token.aud : [token.aud];
    const client = (
      await dependencies.database.query(
        'SELECT metadata FROM webdock_auth."oauthClient" WHERE "clientId"=$1 AND NOT disabled',
        [token.client_id],
      )
    ).rows[0];
    let metadata;
    try {
      metadata =
        typeof client?.metadata === "string"
          ? JSON.parse(client.metadata)
          : client?.metadata;
    } catch {}
    const scopes =
      typeof token.scope === "string" ? token.scope.split(" ") : [];
    if (
      metadata?.webdock_hosting !== true ||
      metadata?.webdock_mcp === true ||
      !audiences.includes(dependencies.mcpResource) ||
      !scopes.includes("hosting:read") ||
      scopes.some((s: string) => s.startsWith("webdock:")) ||
      (await dependencies.currentHostingMCPClaims(token.sub)).disabled
    )
      throw new HostingError(403, "Hosting scope is required.");
    return {
      subject: token.sub,
      sessionID: token.sid,
      source: "oauth",
      scopes,
    };
  }
  /** Shared authenticated dispatch; only HTTP/Studio composition supplies the actor. */
  async function executeHostingBridgeCommand(
    actor: HostingActor,
    raw: unknown,
  ) {
    const command = bodySchema.shape.command.parse(raw);
    dependencies.requireHostingCommand(command.action);
    const settings = dependencies.vercelSettings();
    const runtime = settings
      ? {
          clientID: settings.clientID,
          clientSecret: settings.clientSecret,
          slug: settings.slug,
          origin: settings.origin,
          platformTeam: settings.teamID,
        }
      : undefined;
    return command.action === "databases.credentials.display"
      ? await dependencies.displayDatabaseCredentials(actor, command.databaseID)
      : command.action === "enrollment.display"
        ? await dependencies.issueEnrollment(actor, command.enrollmentID)
        : command.action.startsWith("git.oauth.")
          ? await dependencies.executeGitOAuth(
              actor,
              gitOAuthCommandSchema.parse(command),
            )
          : await dependencies.executeHosting(actor, command, runtime);
  }
  async function hostingBridge(request: Request) {
    try {
      dependencies.requireHostingEnvironment();
      if (request.method !== "POST") throw new HostingError(405, "Use POST.");
      const input = bodySchema.parse(
        await dependencies.readHostingJSON(request),
      );
      const actor = await hostingIdentity(request, input.accessToken);
      const data = await executeHostingBridgeCommand(actor, input.command);
      return Response.json(
        { data },
        { headers: { "Cache-Control": "no-store" } },
      );
    } catch (error) {
      const safe = hostingError(error);
      return Response.json(
        { error: safe.message },
        { status: safe.status, headers: { "Cache-Control": "no-store" } },
      );
    }
  }

  return { hostingIdentity, executeHostingBridgeCommand, hostingBridge };
}
