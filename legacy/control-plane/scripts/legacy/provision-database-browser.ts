/** Explicit offline operator provisioning; inventory contains private runtime secrets. */
import { readFileSync } from "node:fs";
import { z } from "zod";
import { database } from "../../src/db/runtime";
import { encryptMailSecret } from "../../src/modules/platform/platform";
import { databaseCommand, databaseEngine } from "@webdock/database-contracts";

const actor = process.env.WEBDOCK_DATABASE_OPERATOR_ID;
const path = process.argv[2];
if (!actor || !path)
  throw Error(
    "Provide WEBDOCK_DATABASE_OPERATOR_ID and the private inventory path",
  );
const rows = z
  .array(
    z.object({
      name: z.string().min(1).max(120),
      projectID: z.string().regex(/^[1-9][0-9]*$/),
      engine: databaseEngine,
      subject: z.string(),
      runtimeOrigin: z.string(),
      connectionID: z.string(),
      proxySecret: z.string(),
    }),
  )
  .min(1)
  .parse(JSON.parse(readFileSync(path, "utf8")));
const db = await database.connect();
try {
  await db.query("BEGIN");
  await db.query(
    "SELECT pg_advisory_xact_lock(hashtext('webdock-database-provision'))",
  );
  const operator = (
    await db.query(
      'SELECT id FROM webdock_auth."user" WHERE id=$1 AND role=\'operator\' AND NOT COALESCE(banned,false) AND "twoFactorEnabled"',
      [actor],
    )
  ).rows[0];
  if (!operator) throw Error("An active MFA-enabled operator is required");
  for (const row of rows) {
    const project = (
      await db.query(
        "SELECT id,customer_id FROM webdock_admin.projects WHERE id=$1 AND status='active'",
        [row.projectID],
      )
    ).rows[0];
    if (!project) throw Error("Active project required");
    const member = (
      await db.query(
        `SELECT u.id FROM webdock_auth."user" u WHERE u.id=$1 AND NOT COALESCE(u.banned,false) AND
      (u.role='operator' OR EXISTS(SELECT 1 FROM webdock_auth.member m JOIN webdock_auth.tenant_customer t ON t.organization_id=m."organizationId"
        WHERE m."userId"=u.id AND t.customer_id=$2))`,
        [row.subject, project.customer_id],
      )
    ).rows[0];
    if (!member)
      throw Error("Subject does not belong to this project customer");
    const existing = (
      await db.query(
        "SELECT id,engine FROM webdock_auth.database_binding WHERE project_id=$1 AND name=$2 AND enabled",
        [row.projectID, row.name],
      )
    ).rows;
    if (
      existing.length > 1 ||
      (existing[0] && existing[0].engine !== row.engine)
    )
      throw Error("Ambiguous or incompatible existing binding");
    const bindingID =
      existing[0]?.id ??
      (
        await db.query(
          "INSERT INTO webdock_auth.database_binding(project_id,name,environment,engine) VALUES($1,$2,'production',$3) RETURNING id",
          [row.projectID, row.name, row.engine],
        )
      ).rows[0].id;
    const grant = databaseCommand.parse({
      action: "grant",
      bindingID,
      subject: row.subject,
      profile: "schema",
      runtimeOrigin: row.runtimeOrigin,
      connectionID: row.connectionID,
      proxySecret: row.proxySecret,
      isolationVerified: true,
      databaseRoleVerified: true,
    });
    if (grant.action !== "grant") throw Error("Invalid grant");
    const reserved = (
      await db.query(
        `INSERT INTO webdock_auth.database_runtime_owner(runtime_origin,binding_id,subject,profile) VALUES($1,$2,$3,'schema')
      ON CONFLICT(runtime_origin) DO UPDATE SET runtime_origin=EXCLUDED.runtime_origin WHERE database_runtime_owner.binding_id=EXCLUDED.binding_id
      AND database_runtime_owner.subject=EXCLUDED.subject AND database_runtime_owner.profile='schema' RETURNING runtime_origin`,
        [grant.runtimeOrigin, bindingID, row.subject],
      )
    ).rows[0];
    if (!reserved) throw Error("Runtime already belongs to another grant");
    const sealed = encryptMailSecret(
      grant.proxySecret,
      `database:${bindingID}:${row.subject}`,
    );
    await db.query(
      `INSERT INTO webdock_auth.database_grant(binding_id,subject,profile,runtime_origin,connection_id,secret_sealed)
      VALUES($1,$2,'schema',$3,$4,$5) ON CONFLICT(binding_id,subject) DO UPDATE SET profile='schema',runtime_origin=EXCLUDED.runtime_origin,
      connection_id=EXCLUDED.connection_id,secret_sealed=EXCLUDED.secret_sealed,revision=database_grant.revision+1,enabled=true`,
      [bindingID, row.subject, grant.runtimeOrigin, row.connectionID, sealed],
    );
    await db.query(
      "INSERT INTO webdock_auth.database_audit(subject,action,binding_id) VALUES($1,'grant.operator-provision',$2)",
      [actor, bindingID],
    );
    console.log(
      JSON.stringify({
        bindingID,
        projectID: row.projectID,
        subject: row.subject,
        engine: row.engine,
        profile: "schema",
      }),
    );
  }
  await db.query("COMMIT");
} catch (error) {
  await db.query("ROLLBACK");
  throw error;
} finally {
  db.release();
  await database.end();
}
