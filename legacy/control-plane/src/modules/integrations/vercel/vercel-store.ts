import { createHash } from "node:crypto";
import { sealData, unsealData } from "iron-session";
import type { PoolClient } from "pg";
import type { RegistryActor } from "../../registry/compat";
import { recordID } from "../../registry/compat";
import { nextSnowflake } from "../../registry/snowflake";
export const vercelSchemaSQL = `CREATE TABLE IF NOT EXISTS webdock_admin.vercel_connection (
 team_id text PRIMARY KEY, configuration_id text NOT NULL, encrypted_token text NOT NULL,
 connected_by text NOT NULL, connected_at timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS webdock_admin.vercel_project_link (
 project_id varchar PRIMARY KEY REFERENCES webdock_admin.projects(id) ON DELETE CASCADE,
 team_id text NOT NULL, vercel_project_id text NOT NULL, updated_by text NOT NULL,
 updated_at timestamptz NOT NULL DEFAULT now()
);`;
export type VercelCredential = {
  accessToken: string;
  teamID: string;
  configurationID: string;
};
function operator(actor: RegistryActor) {
  if (actor.user?.collection !== "users" || actor.user.role !== "operator")
    throw Error("Operator access required.");
}
function team(value: string) {
  if (!/^team_[a-zA-Z0-9]+$/.test(value)) throw Error("Invalid Vercel team.");
  return value;
}
function key(secret: string) {
  if (secret.length < 32) throw Error("Vercel encryption is not configured.");
  return createHash("sha256")
    .update("webdock:vercel:stored-credential:" + secret)
    .digest("hex");
}
async function audit(
  client: PoolClient,
  actor: RegistryActor,
  auditID: string,
  targetCollection: string,
  targetID: string,
  summary: string,
) {
  await client.query(
    "INSERT INTO webdock_admin.audit_events(id,actor_id,action,target_collection,target_i_d,summary,changed_fields) VALUES($1,$2,'update',$3,$4,$5,'vercelConnection')",
    [auditID, actor.user.id, targetCollection, targetID, summary],
  );
}
export async function saveVercelConnection(
  actor: RegistryActor,
  connection: VercelCredential,
  secret: string,
) {
  operator(actor);
  team(connection.teamID);
  if (
    !/^icfg_[a-zA-Z0-9]+$/.test(connection.configurationID) ||
    !connection.accessToken ||
    connection.accessToken.length > 16384
  )
    throw Error("Invalid Vercel connection.");
  const encrypted = await sealData(
    { kind: "vercel-credential", ...connection },
    { password: key(secret), ttl: 0 },
  );
  const auditID = await nextSnowflake(),
    client = await actor.payload.db.pool.connect();
  try {
    await client.query("BEGIN");
    await client.query(
      "INSERT INTO webdock_admin.vercel_connection(team_id,configuration_id,encrypted_token,connected_by) VALUES($1,$2,$3,$4) ON CONFLICT(team_id) DO UPDATE SET configuration_id=$2,encrypted_token=$3,connected_by=$4,connected_at=now()",
      [connection.teamID, connection.configurationID, encrypted, actor.user.id],
    );
    await audit(
      client,
      actor,
      auditID,
      "integrations",
      connection.teamID,
      "Connected Vercel integration",
    );
    await client.query("COMMIT");
  } catch (error) {
    await client.query("ROLLBACK");
    throw error;
  } finally {
    client.release();
  }
}
export async function loadVercelConnection(
  actor: RegistryActor,
  teamID: string,
  secret: string,
): Promise<VercelCredential | null> {
  operator(actor);
  const row = (
    await actor.payload.db.pool.query(
      "SELECT encrypted_token FROM webdock_admin.vercel_connection WHERE team_id=$1",
      [team(teamID)],
    )
  ).rows[0];
  if (!row) return null;
  return decodeVercelCredential(row.encrypted_token, teamID, secret);
}
export async function decodeVercelCredential(
  encrypted: string,
  teamID: string,
  secret: string,
): Promise<VercelCredential> {
  const value = await unsealData<Partial<VercelCredential> & { kind?: string }>(
    encrypted,
    { password: key(secret), ttl: 0 },
  );
  if (
    value.kind !== "vercel-credential" ||
    value.teamID !== teamID ||
    typeof value.accessToken !== "string" ||
    !value.accessToken ||
    typeof value.configurationID !== "string"
  )
    throw Error("Reconnect Vercel to refresh its stored credentials.");
  return {
    accessToken: value.accessToken,
    teamID,
    configurationID: value.configurationID,
  };
}

export async function removeVercelConnection(
  actor: RegistryActor,
  teamID: string,
) {
  operator(actor);
  team(teamID);
  const auditID = await nextSnowflake(),
    client = await actor.payload.db.pool.connect();
  try {
    await client.query("BEGIN");
    await client.query(
      "DELETE FROM webdock_admin.vercel_connection WHERE team_id=$1",
      [teamID],
    );
    await audit(
      client,
      actor,
      auditID,
      "integrations",
      teamID,
      "Disconnected Vercel from Studio",
    );
    await client.query("COMMIT");
  } catch (error) {
    await client.query("ROLLBACK");
    throw error;
  } finally {
    client.release();
  }
}
export async function loadVercelLink(
  actor: RegistryActor,
  projectID: string,
  teamID: string,
): Promise<string | null> {
  operator(actor);
  return (
    (
      await actor.payload.db.pool.query(
        "SELECT vercel_project_id FROM webdock_admin.vercel_project_link WHERE project_id=$1 AND team_id=$2",
        [recordID.parse(projectID), team(teamID)],
      )
    ).rows[0]?.vercel_project_id || null
  );
}
export async function saveVercelLink(
  actor: RegistryActor,
  projectID: string,
  teamID: string,
  vercelProjectID: string,
) {
  operator(actor);
  recordID.parse(projectID);
  team(teamID);
  if (!/^prj_[a-zA-Z0-9]+$/.test(vercelProjectID))
    throw Error("Invalid Vercel project.");
  const auditID = await nextSnowflake(),
    client = await actor.payload.db.pool.connect();
  try {
    await client.query("BEGIN");
    const project = (
      await client.query(
        "SELECT status FROM webdock_admin.projects WHERE id=$1 FOR UPDATE",
        [projectID],
      )
    ).rows[0];
    if (!project || project.status !== "active")
      throw Error("Choose an active Studio project.");
    await client.query(
      "INSERT INTO webdock_admin.vercel_project_link(project_id,team_id,vercel_project_id,updated_by) VALUES($1,$2,$3,$4) ON CONFLICT(project_id) DO UPDATE SET team_id=$2,vercel_project_id=$3,updated_by=$4,updated_at=now()",
      [projectID, teamID, vercelProjectID, actor.user.id],
    );
    await audit(
      client,
      actor,
      auditID,
      "projects",
      projectID,
      "Linked Vercel hosting project",
    );
    await client.query("COMMIT");
  } catch (error) {
    await client.query("ROLLBACK");
    throw error;
  } finally {
    client.release();
  }
}
