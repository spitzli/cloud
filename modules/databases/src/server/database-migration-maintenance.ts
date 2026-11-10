import type { DatabaseRuntimePorts } from "./ports";
import { createHash, randomBytes } from "node:crypto";
import {
  HostingError,
  type HostingActor,
  type DatabaseCommand,
} from "@spitzli/hosting-contracts";
import type { Connection } from "@spitzli/hosting-contracts/runtime-ports";
export type Dependencies = {
  authorizeHosting: DatabaseRuntimePorts["authorizeHosting"];
  databaseAccess: ReturnType<
    typeof import("./databases").createHostingDatabases
  >["databaseAccess"];
  audit: DatabaseRuntimePorts["audit"];
  clusterLock: DatabaseRuntimePorts["clusterLock"];
  replay: DatabaseRuntimePorts["replay"];
  recordResult: DatabaseRuntimePorts["recordResult"];
  enqueueDatabase: ReturnType<
    typeof import("./database-orders").createHostingDatabaseOrders
  >["enqueueDatabase"];
};

export function createHostingDatabaseMigrationMaintenance(
  dependencies: Dependencies,
) {
  async function migrationMaintenance(
    db: Connection,
    actor: HostingActor,
    cmd: DatabaseCommand,
  ) {
    if (cmd.action === "databases.migrations.list") {
      await dependencies.databaseAccess(db, actor, cmd.databaseID, false, true);
      return (
        await db.query(
          'SELECT id,source_backend_id AS "sourceBackendID",target_backend_id AS "targetBackendID",state,copy_sha256 AS "copySHA256",retained_until AS "retainedUntil" FROM webdock_auth.hosting_database_migration WHERE database_id=$1 ORDER BY created_at DESC',
          [cmd.databaseID],
        )
      ).rows;
    }
    if (
      cmd.action !== "databases.migrationPurgePreview" &&
      cmd.action !== "databases.purgeMigrationSource"
    )
      throw new HostingError(400, "Unknown database migration action.");
    await dependencies.authorizeHosting(
      actor,
      {
        permission: "org:databases:manage",
        operator: true,
        write: cmd.action === "databases.purgeMigrationSource",
      },
      db,
    );
    const m = (
      await db.query(
        "SELECT m.*,b.cluster_id,b.cnpg_uid,b.kind FROM webdock_auth.hosting_database_migration m JOIN webdock_auth.hosting_database_backend b ON b.id=m.source_backend_id WHERE m.id=$1",
        [cmd.migrationID],
      )
    ).rows[0];
    if (!m)
      throw new HostingError(404, "Retained migration source is unavailable.");
    const d = await dependencies.databaseAccess(
      db,
      actor,
      m.database_id,
      cmd.action === "databases.purgeMigrationSource",
      true,
    );
    await dependencies.clusterLock(db, m.cluster_id);
    const kind = "migration:" + m.id;
    if (cmd.action === "databases.migrationPurgePreview") {
      const plan = {
        migrationID: m.id,
        databaseID: d.id,
        name: d.name,
        revision: d.revision,
        sourceBackendID: m.source_backend_id,
        targetBackendID: m.target_backend_id,
        copySHA256: m.copy_sha256,
        retainedUntil: m.retained_until?.toISOString() ?? null,
      };
      const hash = createHash("sha256")
        .update(JSON.stringify(plan) + randomBytes(24).toString("hex"))
        .digest("hex");
      await db.query(
        `INSERT INTO webdock_auth.hosting_database_preview(database_id,session_id,kind,plan_hash,revision,expires_at) VALUES($1,$2,$3,$4,$5,now()+interval '5 minutes')
   ON CONFLICT(database_id,session_id,kind) DO UPDATE SET plan_hash=excluded.plan_hash,revision=excluded.revision,expires_at=excluded.expires_at`,
        [d.id, actor.sessionID, kind, hash, d.revision],
      );
      return { ...plan, planHash: hash };
    }
    const prior = await dependencies.replay(db, actor, cmd.idempotencyKey, cmd);
    if (prior.existing) return prior.existing.result;
    const preview = (
      await db.query(
        "SELECT * FROM webdock_auth.hosting_database_preview WHERE database_id=$1 AND session_id=$2 AND kind=$3 FOR UPDATE",
        [d.id, actor.sessionID, kind],
      )
    ).rows[0];
    if (
      !preview ||
      preview.plan_hash !== cmd.planHash ||
      preview.revision !== d.revision ||
      cmd.revision !== d.revision ||
      cmd.confirmName !== d.name ||
      new Date(preview.expires_at).getTime() <= Date.now()
    )
      throw new HostingError(
        409,
        "Refresh and confirm the retained-source deletion preview.",
      );
    if (
      m.state !== "retained" ||
      !m.copy_sha256 ||
      !m.retained_until ||
      new Date(m.retained_until).getTime() > Date.now() ||
      d.backend_id === m.source_backend_id ||
      d.pending_backend_id === m.source_backend_id
    )
      throw new HostingError(
        409,
        "The retained source is not eligible for deletion.",
      );
    const busy = (
      await db.query(
        "SELECT 1 FROM webdock_auth.hosting_database_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id WHERE (j.database_id=$1 OR j.backend_id=$2 OR j.source_backend_id=$2) AND o.status IN ('queued','running','needs-reconciliation')",
        [d.id, m.source_backend_id],
      )
    ).rowCount;
    if (busy)
      throw new HostingError(
        409,
        "Finish or reconcile the current database operation first.",
      );
    const operationID = await dependencies.enqueueDatabase(
      db,
      actor,
      m.source_backend_id,
      d.id,
      "purgeSource",
      d.revision,
      false,
    );
    await db.query(
      "UPDATE webdock_auth.hosting_database_job SET migration_id=$2 WHERE operation_id=$1",
      [operationID, m.id],
    );
    await db.query(
      "DELETE FROM webdock_auth.hosting_database_preview WHERE database_id=$1 AND session_id=$2 AND kind=$3",
      [d.id, actor.sessionID, kind],
    );
    const result = { migrationID: m.id, operationID };
    await dependencies.recordResult(
      db,
      actor,
      cmd.idempotencyKey,
      prior.hash,
      cmd.action,
      result,
      {
        customerID: d.customer_id,
        projectID: d.project_id,
        clusterID: m.cluster_id,
      },
    );
    await dependencies.audit(db, actor, cmd.action, m.id);
    return result;
  }
  return { migrationMaintenance };
}
