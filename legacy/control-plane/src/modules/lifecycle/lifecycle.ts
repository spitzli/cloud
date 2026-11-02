import { requireOAuthCapability } from "@webdock/auth-configuration/protocols";
import { activeIdentity } from "@webdock/core/identity";
import {currentConsolePrincipal, selectedIdentityDirectory} from "../auth/principal";
import { withDatabaseScope } from "@webdock/database-runtime";
import { createHash } from "node:crypto";
import { database, authDatabase } from "../../db/runtime";
import { currentMCPClaims, mcpResource } from "../auth/mcp";

class LifecycleError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}
const fail = (status: number, message: string): never => {
  throw new LifecycleError(status, message);
};
const digest = (value: unknown) =>
  createHash("sha256").update(JSON.stringify(value)).digest("hex");
type Input = {
  projectID?: string;
  accessToken: string;
  operation: "preview" | "delete";
  bindingID: string;
  customerID: string;
  origin: string;
  planHash?: string;
  requireWrite?: boolean;
};

function input(value: unknown, consoleRequest=false): Input {
  if (!value || typeof value !== "object" || Array.isArray(value))
    fail(400, "Invalid lifecycle request.");
  const v = value as Input;
  if (
    !["preview", "delete"].includes(v.operation) ||
    (!consoleRequest && (typeof v.accessToken !== "string" || !v.accessToken || v.accessToken.length > 16384)) ||
    ![v.bindingID, v.customerID].every(
      (id) => typeof id === "string" && /^[1-9][0-9]{0,18}$/.test(id),
    )
  )
    fail(400, "Invalid lifecycle request.");
  try {
    const url = new URL(v.origin);
    if (
      url.origin !== v.origin ||
      url.protocol !== "https:" ||
      url.username ||
      url.password
    )
      fail(400, "Use an exact HTTPS origin.");
  } catch {
    fail(400, "Use an exact HTTPS origin.");
  }
  if (
    v.projectID !== undefined &&
    (typeof v.projectID !== "string" || !/^[1-9][0-9]{0,18}$/.test(v.projectID))
  )
    fail(400, "Invalid hosting project identity.");
  if (v.requireWrite !== undefined && typeof v.requireWrite !== "boolean")
    fail(400, "Invalid write requirement.");
  if (
    (v.planHash !== undefined && !/^[a-f0-9]{64}$/.test(v.planHash)) ||
    (v.operation === "delete" && !v.planHash)
  )
    fail(400, "A current preview hash is required.");
  return v;
}

async function operator(request: Request, value: Input) {
  const clientID = process.env.WEBDOCK_STUDIO_CLIENT_ID;
  const basic = request.headers.get("authorization") || "";
  if (
    !clientID ||
    basic.length > 16384 ||
    !/^Basic [A-Za-z0-9+/]+=*$/i.test(basic)
  )
    fail(401, "Studio authentication required.");
  const credentials = Buffer.from(basic.slice(6), "base64").toString("utf8");
  const separator = credentials.indexOf(":");
  if (
    credentials.slice(0, separator) !== clientID ||
    !credentials.slice(separator + 1)
  )
    fail(401, "Studio authentication required.");
  const origin = new URL(process.env.BETTER_AUTH_URL || "http://localhost:3125")
    .origin;
  requireOAuthCapability();
  const { auth } = await import("../auth/auth");
  const response = await auth.handler(
    new Request(origin + "/api/auth/oauth2/introspect", {
      method: "POST",
      headers: {
        Origin: origin,
        "Content-Type": "application/x-www-form-urlencoded",
      },
      body: new URLSearchParams({
        token: value.accessToken,
        token_type_hint: "access_token",
        client_id: clientID!,
        client_secret: credentials.slice(separator + 1),
      }),
    }),
  );
  if (!response.ok) fail(401, "Invalid delegated authentication.");
  const token = await response.json();
  if (
    !token ||
    token.active !== true ||
    token.disabled === true ||
    typeof token.sub !== "string" ||
    !token.sub ||
    typeof token.sid !== "string" ||
    !token.sid ||
    typeof token.exp !== "number" ||
    !Number.isFinite(token.exp) ||
    token.exp * 1000 <= Date.now()
  )
    fail(401, "Invalid delegated authentication.");
  if (token.client_id !== clientID) {
    const audiences = Array.isArray(token.aud) ? token.aud : [token.aud];
    const scopes =
      typeof token.scope === "string" ? token.scope.split(" ") : [];
    const client = (
      await authDatabase.query(
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
    } catch {
      /* Invalid metadata fails closed. */
    }
    if (
      !audiences.includes(mcpResource) ||
      metadata?.webdock_mcp !== true ||
      !scopes.includes("webdock:read") ||
      ((value.operation === "delete" || value.requireWrite) &&
        !scopes.includes("webdock:write"))
    )
      fail(403, "Operator MCP scope required.");
  }
  if ((await currentMCPClaims(token.sub)).disabled)
    fail(403, "Platform operator required.");
  return { userID: token.sub as string, sessionID: token.sid as string };
}

export function lifecycle(request:Request):Promise<Response>{return handleLifecycle(request,false);}
export function consoleLifecycle(request:Request):Promise<Response>{return handleLifecycle(request,true);}
async function handleLifecycle(request: Request, consoleRequest:boolean): Promise<Response> {
  try {
    const text = await request.text();
    if (text.length > 32768) fail(413, "Request too large.");
    let raw;
    try {
      raw = JSON.parse(text);
    } catch {
      fail(400, "Invalid JSON.");
    }
    const principal=consoleRequest?currentConsolePrincipal():null;
    if(consoleRequest&&!principal?.operator)fail(403,"Active operator session required.");
    const value = input(raw,consoleRequest);
    const actor = principal ? {userID:principal.userID,sessionID:principal.sessionID} : await operator(request, value);
    return await withDatabaseScope(
      { kind: "session", subject: actor.userID, sessionID: actor.sessionID },
      async () => {
        const connection = await database.connect();
        try {
          await connection.query("BEGIN");
          // Serialize destructive work against preview entry and local suspension.
          const live = (
            await connection.query(
              principal ? `SELECT u.id FROM webdock_auth."user" u WHERE $1::text IS NOT NULL AND u.id=$2 AND NOT coalesce(u.banned,false) FOR UPDATE OF u` : `SELECT s.id FROM webdock_auth.session s JOIN webdock_auth."user" u ON u.id=s."userId" WHERE s.id=$1 AND u.id=$2 AND s."expiresAt">now() AND NOT coalesce(u.banned,false) FOR UPDATE OF s FOR SHARE OF u`,
              [actor.sessionID, actor.userID],
            )
          ).rows[0];
          const identity = live ? await selectedIdentityDirectory.session(
            connection, { subject: actor.userID, sessionID: actor.sessionID }, { lock: true },
          ) : null;
          if (!activeIdentity(identity) || identity.role !== "operator")
            fail(403, "Active operator session required.");
          const preview = (
            await connection.query(
              principal ? "SELECT session_id FROM webdock_auth.console_tenant_preview WHERE session_id=$1 AND issuer=$2" : "SELECT session_id FROM webdock_auth.studio_tenant_preview WHERE session_id=$1 AND expires_at>now() AND expired_at IS NULL",
              [actor.sessionID,...(principal?[principal.issuer]:[])],
            )
          ).rows[0];
          if (preview)
            fail(403, "Leave customer preview before lifecycle operations.");
          if (value.projectID) {
            const project = (
              await connection.query(
                "SELECT customer_id,url FROM webdock_admin.projects WHERE id=$1",
                [value.projectID],
              )
            ).rows[0];
            let projectOrigin;
            try {
              projectOrigin = new URL(project?.url).origin;
            } catch {}
            if (
              !project ||
              project.customer_id !== value.customerID ||
              projectOrigin !== value.origin
            )
              fail(
                409,
                "Hosting project identity does not match this website.",
              );
          }
          const managed = (
            await connection.query(
              "SELECT project_id FROM webdock_auth.hosting_project WHERE customer_id=$1 AND ($2::varchar IS NULL OR project_id=$2) LIMIT 1",
              [value.customerID, value.projectID ?? null],
            )
          ).rows[0];
          if (managed)
            fail(
              409,
              "Managed container hosting must be retired before project deletion.",
            );
          const postgres = (
            await connection.query(
              "SELECT project_id FROM webdock_auth.hosting_database_offer WHERE customer_id=$1 AND ($2::varchar IS NULL OR project_id=$2) LIMIT 1",
              [value.customerID, value.projectID ?? null],
            )
          ).rows[0];
          if (postgres)
            fail(
              409,
              "Managed PostgreSQL resources or order history belong to this project. Archive the project instead of deleting it.",
            );

          const row = (
            await connection.query(
              `SELECT b.id,b.client_id,b.organization_id,b.label,c."redirectUris",c.metadata,t.customer_id FROM webdock_auth.app_binding b JOIN webdock_auth."oauthClient" c ON c."clientId"=b.client_id JOIN webdock_auth.tenant_customer t ON t.organization_id=b.organization_id WHERE b.id=$1 FOR UPDATE OF b,c`,
              [value.bindingID],
            )
          ).rows[0];
          const target = digest([
            value.bindingID,
            value.customerID,
            value.origin,
            value.planHash,
          ]);
          let data;
          if (!row) {
            const proof =
              value.planHash &&
              (
                await connection.query(
                  "SELECT id FROM webdock_auth.access_event WHERE action='lifecycle-delete' AND target_id=$1 AND outcome='succeeded' LIMIT 1",
                  [target],
                )
              ).rows[0];
            if (!proof) fail(404, "Binding not found or not eligible.");
            data = {
              bindingID: value.bindingID,
              customerID: value.customerID,
              origin: value.origin,
              planHash: value.planHash,
              alreadyRemoved: true,
            };
          } else {
            const reserved = new Set([
              "studio.webdock.dev",
              "admin.webdock.dev",
              "auth.webdock.dev",
              "webdock.dev",
              "www.webdock.dev",
              new URL(process.env.BETTER_AUTH_URL || "http://localhost:3125")
                .hostname,
              new URL(mcpResource).hostname,
            ]);
            let metadata;
            try {
              metadata =
                typeof row.metadata === "string"
                  ? JSON.parse(row.metadata)
                  : row.metadata;
            } catch {
              fail(409, "Invalid website metadata.");
            }
            if (
              metadata?.webdock_mcp === true ||
              metadata?.webdock_binding !== value.bindingID
            )
              fail(409, "Binding metadata is protected or changed.");
            let redirects: URL[] = [];
            try {
              redirects = row.redirectUris.map((uri: string) => new URL(uri));
            } catch {
              fail(409, "Invalid website redirects.");
            }
            if (
              row.customer_id !== value.customerID ||
              row.client_id === process.env.WEBDOCK_STUDIO_CLIENT_ID ||
              !redirects.length ||
              redirects.some(
                (url) =>
                  reserved.has(url.hostname) ||
                  url.origin !== value.origin ||
                  url.pathname !== "/api/sso/callback" ||
                  url.username ||
                  url.password,
              )
            )
              fail(409, "Binding mapping is shared, protected or changed.");
            const plan = {
              bindingID: row.id,
              customerID: row.customer_id,
              origin: value.origin,
              clientID: row.client_id,
              organizationID: row.organization_id,
              label: row.label,
            };
            const planHash = digest([
              {
                bindingID: plan.bindingID,
                customerID: plan.customerID,
                origin: plan.origin,
                clientID: plan.clientID,
                organizationID: plan.organizationID,
              },
              row.redirectUris,
            ]);
            data = { ...plan, planHash, alreadyRemoved: false };
            if (value.operation === "delete") {
              if (value.planHash !== planHash)
                fail(409, "Binding changed. Preview again.");
              await connection.query(
                "DELETE FROM webdock_auth.project_grant WHERE binding_id=$1",
                [value.bindingID],
              );
              await connection.query(
                "DELETE FROM webdock_auth.app_binding WHERE id=$1",
                [value.bindingID],
              );
              for (const table of [
                "oauthAccessToken",
                "oauthRefreshToken",
                "oauthConsent",
              ] as const)
                await connection.query(
                  `DELETE FROM webdock_auth."${table}" WHERE "clientId"=$1`,
                  [row.client_id],
                );
              await connection.query(
                'DELETE FROM webdock_auth."oauthClient" WHERE "clientId"=$1',
                [row.client_id],
              );
              await connection.query(
                "INSERT INTO webdock_auth.access_event(actor_id,action,target_id,outcome) VALUES($1,'lifecycle-delete',$2,'succeeded')",
                [actor.userID, target],
              );
            }
          }
          await connection.query("COMMIT");
          return Response.json(data, {
            headers: { "Cache-Control": "no-store" },
          });
        } catch (error) {
          await connection.query("ROLLBACK");
          throw error;
        } finally {
          connection.release();
        }
      },
    );
  } catch (error) {
    return Response.json(
      {
        error:
          error instanceof LifecycleError
            ? error.message
            : "Identity lifecycle operation failed.",
      },
      {
        status: error instanceof LifecycleError ? error.status : 500,
        headers: { "Cache-Control": "no-store" },
      },
    );
  }
}
