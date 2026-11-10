import type { HostingPorts } from "./ports";
import {
  HostingError,
  appDemand,
  hostingDimensions,
  normalizeHostingAllowances,
  resolveAppSpec,
  type HostingActor,
  type HostingCommand,
} from "@spitzli/hosting-contracts";
import { totals } from "./allowances";
export type Dependencies = {
  authorizeHosting: ReturnType<
    typeof import("./authorization").createHostingAuthorization
  >["authorizeHosting"];
  transaction: ReturnType<typeof import("./db").createHostingDb>["transaction"];
  planLock: ReturnType<typeof import("./db").createHostingDb>["planLock"];
  clusterLock: ReturnType<typeof import("./db").createHostingDb>["clusterLock"];
  audit: ReturnType<typeof import("./db").createHostingDb>["audit"];
  clusterView: ReturnType<
    typeof import("./clusters").createHostingClusters
  >["clusterView"];
  replay: ReturnType<
    typeof import("./operations").createHostingOperations
  >["replay"];
  recordResult: ReturnType<
    typeof import("./operations").createHostingOperations
  >["recordResult"];
};
type Relocation = Extract<
  HostingCommand,
  {
    action: "projects.relocate";
  }
>;
export function createHostingRelocation(dependencies: Dependencies) {
  /** Operator handover after an external, recorded cold restore. Never copies data or starts writers. */
  async function relocateProject(actor: HostingActor, cmd: Relocation) {
    return dependencies.transaction(async (db) => {
      const access = await dependencies.authorizeHosting(
        actor,
        {
          permission: "org:projects:manage",
          projectID: cmd.projectID,
          operator: true,
          write: true,
        },
        db,
      );
      await dependencies.planLock(db, access.customerID!);
      for (const id of [
        ...new Set([cmd.sourceClusterID, cmd.targetClusterID]),
      ].sort())
        await dependencies.clusterLock(db, id);
      const prior = await dependencies.replay(
        db,
        actor,
        cmd.idempotencyKey,
        cmd,
      );
      if (prior.existing) return prior.existing.result;
      const current = await dependencies.authorizeHosting(
        actor,
        {
          permission: "org:projects:manage",
          projectID: cmd.projectID,
          operator: true,
          write: true,
        },
        db,
      );
      if (
        current.project.provider !== "k3s" ||
        current.project.cluster_id !== cmd.sourceClusterID ||
        current.project.revision !== cmd.revision ||
        cmd.sourceClusterID === cmd.targetClusterID
      )
        throw new HostingError(
          409,
          "Hosting project changed. Refresh before relocation.",
        );
      const age = Date.now() - Date.parse(cmd.checkedAt);
      if (age < -30000 || age > 300000)
        throw new HostingError(
          409,
          "Fresh source fencing and target restore evidence are required.",
        );
      const source = await dependencies.clusterView(db, cmd.sourceClusterID),
        target = await dependencies.clusterView(db, cmd.targetClusterID);
      if (
        source.state !== "connected" ||
        !target.workloadReady ||
        target.state !== "connected"
      )
        throw new HostingError(
          409,
          "Both clusters require fresh observations and the target must be activated.",
        );
      if (
        target.ownership !== "platform" ||
        (target.dedicatedCustomerID &&
          target.dedicatedCustomerID !== access.customerID)
      )
        throw new HostingError(
          403,
          "Target cluster is not available to this customer.",
        );
      const busy = await db.query(
        "SELECT id FROM webdock_auth.hosting_operation WHERE project_id=$1 AND status IN ('queued','running','needs-reconciliation')",
        [cmd.projectID],
      );
      if (busy.rowCount)
        throw new HostingError(
          409,
          "Finish or reconcile existing operations before relocation.",
        );
      const apps = (
        await db.query(
          "SELECT * FROM webdock_auth.hosting_app WHERE project_id=$1 AND lifecycle='deployed' AND (status<>'deleted' OR COALESCE((spec->>'volumeBytes')::bigint,0)>0) ORDER BY id FOR UPDATE",
          [cmd.projectID],
        )
      ).rows;
      if (
        !apps.length ||
        apps.length !== cmd.evidence.length ||
        new Set(cmd.evidence.map((e) => e.appID)).size !== apps.length
      )
        throw new HostingError(
          409,
          "Restore evidence must cover exactly the current applications.",
        );
      for (const app of apps) {
        if (
          app.status !== "stopped" ||
          app.spec.replicas !== 0 ||
          app.observed_spec?.replicas !== 0 ||
          app.observed_revision !== app.revision ||
          !app.observed_uid
        )
          throw new HostingError(
            409,
            "Every application must be stopped and observed before relocation.",
          );
        const evidence = cmd.evidence.find((e) => e.appID === app.id);
        if (
          !evidence ||
          evidence.revision !== app.revision ||
          evidence.sourceUID !== app.observed_uid ||
          evidence.backupSHA256 !== evidence.restoredSHA256
        )
          throw new HostingError(
            409,
            "Restore evidence does not match the stopped application.",
          );
        const stopped = (
          await db.query(
            "SELECT result FROM webdock_auth.hosting_operation WHERE app_id=$1 AND cluster_id=$2 AND target_revision=$3 AND status='succeeded' AND desired->>'action'='apply' AND desired->'spec'->>'replicas'='0' ORDER BY created_at DESC LIMIT 1",
            [app.id, cmd.sourceClusterID, app.revision],
          )
        ).rows[0]?.result?.proof;
        if (
          !stopped ||
          stopped.appID !== app.id ||
          stopped.namespace !== "wd-" + cmd.projectID ||
          stopped.uid !== app.observed_uid ||
          stopped.revision !== app.revision ||
          stopped.ready !== true ||
          stopped.replicas !== 0 ||
          stopped.deleted !== false
        )
          throw new HostingError(
            409,
            "A completed source stop proof is required.",
          );
        if (
          app.spec.volumeBytes &&
          (target.observation?.capabilities?.storage !== true ||
            target.observation?.capabilities?.storageVersion !== 1 ||
            stopped.storage?.volumeBytes !== app.spec.volumeBytes ||
            stopped.storage?.retained !== false)
        )
          throw new HostingError(
            409,
            "Verified target storage and source storage proof are required.",
          );
        if (
          app.environment_encrypted &&
          target.observation?.capabilities?.environment !== true
        )
          throw new HostingError(
            409,
            "Target environment support is required.",
          );
      }
      const reservations = (
        await db.query(
          "SELECT * FROM webdock_auth.hosting_reservation WHERE project_id=$1 AND status<>'released' FOR UPDATE",
          [cmd.projectID],
        )
      ).rows;
      if (
        reservations.length !== apps.length ||
        apps.some((a) => {
          const rows = reservations.filter((r) => r.app_id === a.id);
          return (
            rows.length !== 1 ||
            rows[0].status !== "active" ||
            rows[0].cluster_id !== cmd.sourceClusterID ||
            rows[0].customer_id !== access.customerID ||
            rows[0].demand.volumeBytes !== (a.spec.volumeBytes ?? 0)
          );
        })
      )
        throw new HostingError(
          409,
          "Application reservations require reconciliation before relocation.",
        );
      const allocated = (
        await db.query(
          "SELECT demand,status FROM webdock_auth.hosting_reservation WHERE cluster_id=$1 AND status<>'released'",
          [cmd.targetClusterID],
        )
      ).rows;
      // Check room to start one writer per application as well as its retained storage.
      const demand = totals([
        ...allocated,
        ...apps.map((a) => ({
          status: "active",
          demand: appDemand(resolveAppSpec({ ...a.spec, replicas: 1 })),
        })),
      ]);
      const capacity = normalizeHostingAllowances(target.capacity);
      if (
        hostingDimensions.some(
          (k) => capacity[k] === null || demand[k] > capacity[k]!,
        )
      )
        throw new HostingError(
          409,
          "Verified target capacity is insufficient.",
        );
      const retainedBytes = apps.reduce(
        (sum, a) => sum + (a.spec.volumeBytes ?? 0),
        0,
      );
      if (retainedBytes) {
        const sourceCapacity = normalizeHostingAllowances(
          source.capacity ?? source.observation?.capacity,
        );
        sourceCapacity.volumeBytes = Math.max(
          0,
          (sourceCapacity.volumeBytes ?? 0) - retainedBytes,
        );
        // Recovery copies occupy the source pool, but must not charge the customer twice.
        await db.query(
          "UPDATE webdock_auth.hosting_cluster SET capacity=$2,revision=revision+1 WHERE id=$1",
          [cmd.sourceClusterID, JSON.stringify(sourceCapacity)],
        );
      }
      await db.query(
        "UPDATE webdock_auth.hosting_project SET cluster_id=$2,revision=revision+1 WHERE project_id=$1",
        [cmd.projectID, cmd.targetClusterID],
      );
      await db.query(
        "UPDATE webdock_auth.hosting_reservation SET cluster_id=$2 WHERE project_id=$1 AND status<>'released'",
        [cmd.projectID, cmd.targetClusterID],
      );
      await db.query(
        "UPDATE webdock_auth.hosting_app SET revision=revision+1,observed_revision=0,observed_uid=NULL,observed_spec=NULL,observed_environment_encrypted=NULL,previous_spec=NULL,previous_environment_encrypted=NULL,operation_id=NULL,status='pending',last_error=NULL,logs=NULL WHERE id=ANY($1)",
        [apps.map((a) => a.id)],
      );
      const result = {
        projectID: cmd.projectID,
        clusterID: cmd.targetClusterID,
        sourceClusterID: cmd.sourceClusterID,
        revision: cmd.revision + 1,
        apps: apps.map((a) => ({ appID: a.id, revision: a.revision + 1 })),
        writersStarted: false,
        sourceDataRetained: true,
        sourceRetainedStorageBytes: retainedBytes,
        evidence: cmd.evidence,
        checkedAt: cmd.checkedAt,
      };
      await dependencies.recordResult(
        db,
        actor,
        cmd.idempotencyKey,
        prior.hash,
        cmd.action,
        result,
        {
          customerID: access.customerID,
          projectID: cmd.projectID,
          clusterID: cmd.targetClusterID,
        },
      );
      await dependencies.audit(db, actor, cmd.action, cmd.projectID);
      return result;
    });
  }
  return { relocateProject };
}
