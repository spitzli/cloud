import type { DatabaseRuntimePorts } from "./ports";
import { createHash, randomBytes } from "node:crypto";
import {
  HostingError,
  type HostingActor,
  type DatabaseCommand,
} from "@spitzli/hosting-contracts";
import type { Connection } from "@spitzli/hosting-contracts/runtime-ports";
export type Dependencies = {
  assertDatabaseMaintenanceAllowed: ReturnType<
    typeof import("./database-protection").createHostingDatabaseProtection
  >["assertDatabaseMaintenanceAllowed"];
  databaseAccess: ReturnType<
    typeof import("./databases").createHostingDatabases
  >["databaseAccess"];
  databaseView: ReturnType<
    typeof import("./databases").createHostingDatabases
  >["databaseView"];
  clusterLock: DatabaseRuntimePorts["clusterLock"];
  audit: DatabaseRuntimePorts["audit"];
  enqueueDatabase: ReturnType<
    typeof import("./database-orders").createHostingDatabaseOrders
  >["enqueueDatabase"];
  databaseCredentials: ReturnType<
    typeof import("./database-orders").createHostingDatabaseOrders
  >["databaseCredentials"];
  freshDatabaseCluster: ReturnType<
    typeof import("./database-orders").createHostingDatabaseOrders
  >["freshDatabaseCluster"];
  replay: DatabaseRuntimePorts["replay"];
  recordResult: DatabaseRuntimePorts["recordResult"];
  encryptSecret: DatabaseRuntimePorts["encryptSecret"];
};

export function createHostingDatabaseMaintenance(dependencies: Dependencies) {
  async function databaseMaintenance(
    db: Connection,
    actor: HostingActor,
    cmd: DatabaseCommand,
  ) {
    if (!("databaseID" in cmd) || !cmd.databaseID)
      throw new HostingError(400, "Database target required.");
    const preview =
      cmd.action === "databases.deletionPreview" ||
      cmd.action === "databases.purgePreview";
    const destructive = [
      "databases.delete",
      "databases.purge",
      "databases.reconcile",
    ].includes(cmd.action);
    const row = await dependencies.databaseAccess(
      db,
      actor,
      cmd.databaseID,
      !preview,
      preview || destructive,
    );
    dependencies.assertDatabaseMaintenanceAllowed(row.id, cmd.action);
    const backend = (
      await db.query(
        "SELECT * FROM webdock_auth.hosting_database_backend WHERE id=$1",
        [row.backend_id],
      )
    ).rows[0];
    await dependencies.clusterLock(db, backend.cluster_id);
    const current = (
      await db.query(
        "SELECT * FROM webdock_auth.hosting_database WHERE id=$1 FOR UPDATE",
        [row.id],
      )
    ).rows[0];
    if (preview) {
      const kind = cmd.action === "databases.purgePreview" ? "purge" : "delete";
      const plan = {
        databaseID: row.id,
        name: row.name,
        revision: current.revision,
        kind,
        backendID: backend.id,
        cnpgUID: backend.cnpg_uid,
        scope:
          row.kind === "shared" ? "this-logical-database" : "this-installation",
        backupStatus: "not-configured",
        retainedUntil: current.retained_until?.toISOString() ?? null,
      };
      const hash = createHash("sha256")
        .update(JSON.stringify(plan) + randomBytes(24).toString("hex"))
        .digest("hex");
      const expiresAt = new Date(Date.now() + 300000).toISOString();
      await db.query(
        `INSERT INTO webdock_auth.hosting_database_preview(database_id,session_id,kind,plan_hash,revision,expires_at)
   VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(database_id,session_id,kind) DO UPDATE SET plan_hash=excluded.plan_hash,revision=excluded.revision,expires_at=excluded.expires_at`,
        [row.id, actor.sessionID, kind, hash, current.revision, expiresAt],
      );
      return { ...plan, planHash: hash, expiresAt };
    }
    if (!("idempotencyKey" in cmd) || !("revision" in cmd))
      throw new HostingError(400, "Fenced database command required.");
    const prior = await dependencies.replay(db, actor, cmd.idempotencyKey, cmd);
    if (prior.existing) return prior.existing.result;
    if (current.revision !== cmd.revision)
      throw new HostingError(409, "Database changed. Refresh and try again.");
    if (cmd.action === "databases.requestCancellation") {
      await db.query(
        "UPDATE webdock_auth.hosting_database_order SET cancellation_requested_at=COALESCE(cancellation_requested_at,now()) WHERE id=$1",
        [current.order_id],
      );
    } else if (cmd.action === "databases.reconcile") {
      const pending = (
        await db.query(
          `SELECT o.id,o.generation,o.lease_until,j.backend_id,j.action,j.revision,j.lease_token_hash,j.checkpoint FROM webdock_auth.hosting_database_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id
   WHERE j.database_id=$1 AND o.status='needs-reconciliation' ORDER BY o.created_at DESC LIMIT 1 FOR UPDATE OF o,j`,
          [row.id],
        )
      ).rows[0];
      const actualBackend = pending
        ? (
            await db.query(
              "SELECT cnpg_uid,revision,observed_revision,state FROM webdock_auth.hosting_database_backend WHERE id=$1",
              [pending.backend_id],
            )
          ).rows[0]
        : null;
      const neverClaimed =
        pending &&
        actualBackend &&
        pending.id === current.operation_id &&
        pending.backend_id === row.backend_id &&
        pending.action === "create" &&
        pending.revision === 1 &&
        current.revision === 1 &&
        current.state === "needs-reconciliation" &&
        pending.generation === 0 &&
        pending.lease_until === null &&
        pending.lease_token_hash === null &&
        pending.checkpoint === null &&
        actualBackend.cnpg_uid === null &&
        actualBackend.revision === 1 &&
        actualBackend.observed_revision === 0 &&
        actualBackend.state === "provisioning";
      if (
        !neverClaimed &&
        (!pending || !actualBackend?.cnpg_uid || !pending.checkpoint)
      )
        throw new HostingError(
          409,
          "Operator must inspect the unconfirmed installation before reconciliation.",
        );
      if (neverClaimed) {
        // No lease was ever issued, so no agent could have started this creation.
        await dependencies.freshDatabaseCluster(db, backend.cluster_id);
        await db.query(
          "UPDATE webdock_auth.hosting_database SET state='provisioning' WHERE id=$1",
          [row.id],
        );
      } else {
        // Once a lease exists, reconciliation only observes the previous intent.
        await db.query(
          "UPDATE webdock_auth.hosting_database_job SET checkpoint=$2 WHERE operation_id=$1",
          [
            pending.id,
            JSON.stringify({
              ...pending.checkpoint,
              phase: "observing",
              cnpgUID: actualBackend.cnpg_uid,
            }),
          ],
        );
      }
      await db.query(
        "UPDATE webdock_auth.hosting_operation SET status='queued',actor_session=$2,subject=$3,desired=coalesce(desired,'{}'::jsonb)||jsonb_build_object('consoleIdentity',$4::jsonb) WHERE id=$1",
        [
          pending.id,
          actor.sessionID,
          actor.subject,
          JSON.stringify(actor.consoleIdentity ?? null),
        ],
      );
    } else {
      await dependencies.freshDatabaseCluster(db, backend.cluster_id);
      const busy = (
        await db.query(
          "SELECT 1 FROM webdock_auth.hosting_database_job j JOIN webdock_auth.hosting_operation o ON o.id=j.operation_id WHERE j.database_id=$1 AND o.status IN ('queued','running','needs-reconciliation')",
          [row.id],
        )
      ).rowCount;
      if (busy)
        throw new HostingError(
          409,
          "Finish or reconcile the current database operation first.",
        );
      let action: string, state: string;
      if (cmd.action === "databases.pause") {
        if (!["ready", "degraded"].includes(current.state))
          throw new HostingError(409, "Only a ready database can be paused.");
        action = "pause";
        state = "pausing";
      } else if (cmd.action === "databases.resume") {
        if (current.state !== "paused")
          throw new HostingError(409, "Only a paused database can be resumed.");
        action = "resume";
        state = "resuming";
      } else if (cmd.action === "databases.restart") {
        if (row.kind === "shared")
          throw new HostingError(
            409,
            "Shared databases cannot restart their pool.",
          );
        if (!["ready", "degraded"].includes(current.state))
          throw new HostingError(
            409,
            "Only a ready instance can be restarted.",
          );
        action = "restart";
        state = "updating";
      } else if (cmd.action === "databases.rotateCredentials") {
        if (current.state !== "ready")
          throw new HostingError(
            409,
            "Only a ready database can rotate credentials.",
          );
        const revision = current.credential_revision + 1;
        await db.query(
          "UPDATE webdock_auth.hosting_database SET credential_revision=$2,credentials_encrypted=$3 WHERE id=$1",
          [
            row.id,
            revision,
            dependencies.encryptSecret(
              dependencies.databaseCredentials(),
              `database:${row.id}:${revision}`,
            ),
          ],
        );
        action = "rotateCredentials";
        state = "updating";
      } else if (
        cmd.action === "databases.delete" ||
        cmd.action === "databases.purge"
      ) {
        action = cmd.action === "databases.delete" ? "delete" : "purge";
        state = action === "delete" ? "deleting" : "retained";
        const proof = (
          await db.query(
            "SELECT * FROM webdock_auth.hosting_database_preview WHERE database_id=$1 AND session_id=$2 AND kind=$3 FOR UPDATE",
            [row.id, actor.sessionID, action],
          )
        ).rows[0];
        if (
          !proof ||
          proof.plan_hash !== cmd.planHash ||
          proof.revision !== current.revision ||
          new Date(proof.expires_at).getTime() <= Date.now() ||
          cmd.confirmName !== row.name
        )
          throw new HostingError(
            409,
            "Refresh and confirm the database deletion preview.",
          );
        if (
          action === "purge" &&
          (current.state !== "retained" ||
            !current.retained_until ||
            new Date(current.retained_until).getTime() > Date.now())
        )
          throw new HostingError(409, "Database retention has not expired.");
        if (
          action === "delete" &&
          ["retained", "purged"].includes(current.state)
        )
          throw new HostingError(
            409,
            "Database is already retained or purged.",
          );
        await db.query(
          "DELETE FROM webdock_auth.hosting_database_preview WHERE database_id=$1",
          [row.id],
        );
      } else
        throw new HostingError(
          409,
          "Database maintenance requires an accepted matching upgrade order.",
        );
      await db.query(
        "UPDATE webdock_auth.hosting_database SET revision=revision+1,state=$2 WHERE id=$1",
        [row.id, state],
      );
      await dependencies.enqueueDatabase(
        db,
        actor,
        row.backend_id,
        row.id,
        action,
        current.revision + 1,
      );
    }
    const result = await dependencies.databaseView(db, row.id);
    await dependencies.recordResult(
      db,
      actor,
      cmd.idempotencyKey,
      prior.hash,
      cmd.action,
      result,
      {
        customerID: row.customer_id,
        projectID: row.project_id,
        clusterID: backend.cluster_id,
      },
    );
    await dependencies.audit(db, actor, cmd.action, row.id);
    return result;
  }
  return { databaseMaintenance };
}
