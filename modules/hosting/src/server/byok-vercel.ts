import type { HostingPorts } from "./ports";
import { HostingProviderError } from "@spitzli/module-hosting/providers";
import { createHash, randomBytes } from "node:crypto";
import {
  HostingError,
  type HostingActor,
  type HostingCommand,
} from "@spitzli/hosting-contracts";
export type Dependencies = {
  fetchVercelProject: HostingPorts["fetchVercelProject"];
  listVercelProjects: HostingPorts["listVercelProjects"];

  createVercelJSONTransport: HostingPorts["createVercelJSONTransport"];
  encryptMailSecret: HostingPorts["encryptMailSecret"];
  decryptMailSecret: HostingPorts["decryptMailSecret"];
  transaction: ReturnType<typeof import("./db").createHostingDb>["transaction"];
  audit: ReturnType<typeof import("./db").createHostingDb>["audit"];
  planLock: ReturnType<typeof import("./db").createHostingDb>["planLock"];
  byokAccess: ReturnType<
    typeof import("./byok").createHostingByok
  >["byokAccess"];
  env: HostingPorts["env"];
};
export type VercelRuntime = {
  clientID: string;
  clientSecret: string;
  slug: string;
  origin: string;
  platformTeam: string;
};
export function createHostingByokVercel(dependencies: Dependencies) {
  const hash = (text: string) =>
    createHash("sha256").update(text).digest("hex");
  function settings(runtime?: VercelRuntime) {
    const clientID =
        runtime?.clientID ?? dependencies.env.WEBDOCK_BYOK_VERCEL_CLIENT_ID,
      clientSecret =
        runtime?.clientSecret ??
        dependencies.env.WEBDOCK_BYOK_VERCEL_CLIENT_SECRET,
      slug =
        runtime?.slug ?? dependencies.env.WEBDOCK_BYOK_VERCEL_INTEGRATION_SLUG;
    const origin = runtime?.origin ?? dependencies.env.WEBDOCK_STUDIO_ORIGIN;
    const platformTeam =
      runtime?.platformTeam ?? dependencies.env.WEBDOCK_VERCEL_TEAM_ID;
    if (
      (runtime && runtime.origin !== dependencies.env.WEBDOCK_STUDIO_ORIGIN) ||
      !platformTeam ||
      !/^team_[A-Za-z0-9]+$/.test(platformTeam) ||
      !clientID ||
      !clientSecret ||
      !slug ||
      !origin ||
      !/^https:\/\/[a-z0-9.-]+$/.test(origin) ||
      !/^[a-z0-9-]+$/.test(slug)
    )
      throw new HostingError(
        503,
        "Customer Vercel connections are not configured yet.",
      );
    return {
      clientID,
      clientSecret,
      slug,
      platformTeam,
      callback: origin + "/api/vercel/callback",
    };
  }
  const providerJSON = dependencies.createVercelJSONTransport();
  /** Compatibility boundary keeps the service's existing error type and status contract. */
  async function vercelJSON(
    path: string,
    token?: string,
    teamID?: string,
    init: RequestInit = {},
  ) {
    try {
      return await providerJSON(path, token, teamID, init);
    } catch (error) {
      if (error instanceof HostingProviderError)
        throw new HostingError(error.status, error.message);
      throw error;
    }
  }
  async function executeByokVercel(
    actor: HostingActor,
    cmd: HostingCommand,
    runtime?: VercelRuntime,
  ) {
    return dependencies.transaction(async (db) => {
      if (cmd.action === "byok.vercel.finish") {
        if (actor.source !== "studio")
          throw new HostingError(403, "Connect Vercel in Studio.");
        const flow = (
          await db.query(
            "UPDATE webdock_auth.hosting_vercel_flow SET consumed=true WHERE state_hash=$1 AND subject=$2 AND session_id=$3 AND NOT consumed AND expires_at>now() RETURNING customer_id",
            [hash(cmd.state), actor.subject, actor.sessionID],
          )
        ).rows[0];
        if (!flow)
          throw new HostingError(
            403,
            "This connection attempt expired. Start again.",
          );
        await dependencies.byokAccess(
          db,
          actor,
          flow.customer_id,
          true,
          "vercel",
        );
        await dependencies.planLock(db, flow.customer_id);
        const config = settings(runtime);
        const token = await vercelJSON(
          "/v2/oauth/access_token",
          undefined,
          undefined,
          {
            method: "POST",
            headers: { "Content-Type": "application/x-www-form-urlencoded" },
            body: new URLSearchParams({
              client_id: config.clientID,
              client_secret: config.clientSecret,
              code: cmd.code,
              redirect_uri: config.callback,
            }),
          },
        );
        if (
          token.team_id !== cmd.teamID ||
          token.installation_id !== cmd.configurationID ||
          typeof token.access_token !== "string" ||
          token.access_token.length > 8192 ||
          typeof token.token_type !== "string" ||
          token.token_type.toLowerCase() !== "bearer"
        )
          throw new HostingError(
            403,
            "Vercel connection could not be verified.",
          );
        const installation = await vercelJSON(
          "/v1/integrations/configuration/" + cmd.configurationID,
          token.access_token,
          cmd.teamID,
        );
        if (
          installation.id !== cmd.configurationID ||
          installation.teamId !== cmd.teamID ||
          installation.integrationId !== config.clientID ||
          installation.disabledAt ||
          installation.deletedAt ||
          installation.deleteRequestedAt ||
          !Array.isArray(installation.scopes) ||
          !installation.scopes.includes("read:integration-configuration") ||
          !installation.scopes.some((s: string) =>
            ["read:deployment", "read-write:deployment"].includes(s),
          ) ||
          !installation.scopes.some((s: string) =>
            ["read:domain", "read-write:domain"].includes(s),
          ) ||
          !installation.scopes.some((s: string) =>
            ["read:project", "read-write:project"].includes(s),
          )
        )
          throw new HostingError(
            403,
            "Vercel connection could not be verified.",
          );
        // A Vercel team has exactly one canonical tenant owner in Webdock.
        await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
          "byok-vercel:" + cmd.teamID,
        ]);
        const other = (
          await db.query(
            "SELECT customer_id FROM webdock_auth.hosting_vercel WHERE team_id=$1 AND customer_id<>$2",
            [cmd.teamID, flow.customer_id],
          )
        ).rowCount;
        if (other || cmd.teamID === config.platformTeam)
          throw new HostingError(
            409,
            "This Vercel team is already assigned to another connection.",
          );
        const encrypted = dependencies.encryptMailSecret(
          { token: token.access_token, scopes: installation.scopes },
          "byok-vercel:" + flow.customer_id + ":" + cmd.teamID,
        );
        await db.query(
          "INSERT INTO webdock_auth.hosting_vercel(customer_id,team_id,configuration_id,encrypted_token) VALUES($1,$2,$3,$4) ON CONFLICT(customer_id) DO UPDATE SET team_id=$2,configuration_id=$3,encrypted_token=$4,projects='[]',revision=hosting_vercel.revision+1,connected_at=now()",
          [flow.customer_id, cmd.teamID, cmd.configurationID, encrypted],
        );
        await dependencies.audit(db, actor, cmd.action, flow.customer_id);
        return { customerID: flow.customer_id };
      }
      if (!("customerID" in cmd))
        throw new HostingError(400, "Invalid Vercel request.");
      const read = ["byok.vercel.projects", "byok.vercel.resources"].includes(
        cmd.action,
      );
      const access = await dependencies.byokAccess(
        db,
        actor,
        cmd.customerID,
        !read,
        "vercel",
      );
      if (!access.liveReads)
        throw new HostingError(403, "Customer preview is read-only.");
      if (cmd.action === "byok.vercel.begin") {
        if (actor.source !== "studio")
          throw new HostingError(403, "Connect Vercel in Studio.");
        const config = settings(runtime),
          state = randomBytes(32).toString("base64url");
        await dependencies.planLock(db, cmd.customerID);
        const count = (
          await db.query(
            "SELECT count(*)::int AS n FROM webdock_auth.hosting_vercel_flow WHERE customer_id=$1 AND expires_at>now()",
            [cmd.customerID],
          )
        ).rows[0].n;
        if (count >= 5)
          throw new HostingError(
            429,
            "Wait before starting another connection.",
          );
        await db.query(
          "INSERT INTO webdock_auth.hosting_vercel_flow(state_hash,customer_id,subject,session_id,expires_at) VALUES($1,$2,$3,$4,now()+interval '10 minutes')",
          [hash(state), cmd.customerID, actor.subject, actor.sessionID],
        );
        return {
          url: `https://vercel.com/integrations/${config.slug}/new?state=${state}`,
        };
      }
      await dependencies.planLock(db, cmd.customerID);
      const connection = (
        await db.query(
          "SELECT * FROM webdock_auth.hosting_vercel WHERE customer_id=$1 FOR UPDATE",
          [cmd.customerID],
        )
      ).rows[0];
      if (!connection)
        throw new HostingError(409, "Connect your Vercel team first.");
      const credential = dependencies.decryptMailSecret<{
        token: string;
        scopes: string[];
      }>(
        connection.encrypted_token,
        "byok-vercel:" + cmd.customerID + ":" + connection.team_id,
      );
      const options = { token: credential.token, teamID: connection.team_id };
      if (cmd.action === "byok.vercel.disconnect") {
        if (connection.revision !== cmd.revision)
          throw new HostingError(
            409,
            "Settings changed. Refresh and try again.",
          );
        await db.query(
          "DELETE FROM webdock_auth.hosting_vercel WHERE customer_id=$1",
          [cmd.customerID],
        );
        await dependencies.audit(db, actor, cmd.action, cmd.customerID);
        return { disconnected: true };
      }
      if (
        cmd.action === "byok.vercel.projects" ||
        cmd.action === "byok.vercel.select"
      ) {
        const projects: {
          id: string;
          name: string;
        }[] = [];
        let cursor: string | null = null;
        for (let i = 0; i < 10; i++) {
          const page = await dependencies.listVercelProjects({
            ...options,
            cursor: cursor ?? undefined,
          });
          projects.push(...page.projects);
          cursor = page.nextCursor;
          if (!cursor) break;
        }
        if (cmd.action === "byok.vercel.projects")
          return {
            projects: access.canWrite
              ? projects
              : projects.filter((p) => connection.projects.includes(p.id)),
            selected: connection.projects,
            revision: connection.revision,
            complete: !cursor,
          };
        if (connection.revision !== cmd.revision)
          throw new HostingError(
            409,
            "Settings changed. Refresh and try again.",
          );
        if (cmd.projects.some((id) => !projects.some((p) => p.id === id)))
          throw new HostingError(
            403,
            "Choose projects available to this Vercel connection.",
          );
        await db.query(
          "UPDATE webdock_auth.hosting_vercel SET projects=$2,revision=revision+1 WHERE customer_id=$1",
          [cmd.customerID, JSON.stringify([...new Set(cmd.projects)])],
        );
        await dependencies.audit(db, actor, cmd.action, cmd.customerID);
        return { saved: true };
      }
      if (!("projectID" in cmd) || !connection.projects.includes(cmd.projectID))
        throw new HostingError(404, "Vercel project is unavailable.");
      if (cmd.action === "byok.vercel.resources")
        return {
          ...(await dependencies.fetchVercelProject({
            ...options,
            projectID: cmd.projectID,
          })),
          usage: null,
          canSetRegion: credential.scopes.includes("read-write:project"),
          canCancel: credential.scopes.includes("read-write:deployment"),
        };
      // Verify project membership fresh before any provider mutation.
      const project = await vercelJSON(
        "/v9/projects/" + cmd.projectID,
        credential.token,
        connection.team_id,
      );
      if (
        project.id !== cmd.projectID ||
        project.accountId !== connection.team_id
      )
        throw new HostingError(404, "Vercel project is unavailable.");
      if (cmd.action === "byok.vercel.region") {
        if (!credential.scopes.includes("read-write:project"))
          throw new HostingError(
            403,
            "Reconnect Vercel with project management permission.",
          );
        await vercelJSON(
          "/v9/projects/" + cmd.projectID,
          credential.token,
          connection.team_id,
          {
            method: "PATCH",
            headers: { "Content-Type": "application/json" },
            body: JSON.stringify({
              functionDefaultRegions: ["fra1"],
              functionZeroConfigFailover: false,
            }),
          },
        );
      } else if (cmd.action === "byok.vercel.cancel") {
        if (!credential.scopes.includes("read-write:deployment"))
          throw new HostingError(
            403,
            "Reconnect Vercel with deployment management permission.",
          );
        const deployment = await vercelJSON(
          "/v13/deployments/" + cmd.deploymentID,
          credential.token,
          connection.team_id,
        );
        if (
          deployment.projectId !== cmd.projectID ||
          typeof deployment.readyState !== "string" ||
          !["QUEUED", "BUILDING", "INITIALIZING"].includes(
            deployment.readyState,
          )
        )
          throw new HostingError(409, "This deployment cannot be canceled.");
        await vercelJSON(
          "/v12/deployments/" + cmd.deploymentID + "/cancel",
          credential.token,
          connection.team_id,
          { method: "PATCH" },
        );
      } else throw new HostingError(400, "Invalid Vercel request.");
      await dependencies.audit(db, actor, cmd.action, cmd.projectID);
      return { saved: true };
    });
  }
  return { vercelJSON, executeByokVercel };
}
