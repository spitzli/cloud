import type { HostingPorts } from "./ports";
import {
  HostingError,
  normalizeHostingAllowances,
  hostingDimensions,
  resourceID,
  type HostingActor,
} from "@spitzli/hosting-contracts";
import { limits, totals } from "./allowances";
import { z } from "zod";
export type Dependencies = {
  nativeClusterDemand: HostingPorts["nativeClusterDemand"];
  clusterCapacityDemand: HostingPorts["clusterCapacityDemand"];

  authorizeHosting: ReturnType<
    typeof import("./authorization").createHostingAuthorization
  >["authorizeHosting"];
  transaction: ReturnType<typeof import("./db").createHostingDb>["transaction"];
  audit: ReturnType<typeof import("./db").createHostingDb>["audit"];
  planLock: ReturnType<typeof import("./db").createHostingDb>["planLock"];
  clusterLock: ReturnType<typeof import("./db").createHostingDb>["clusterLock"];
  replay: ReturnType<
    typeof import("./operations").createHostingOperations
  >["replay"];
  env: HostingPorts["env"];
};

export function createHostingReservations(dependencies: Dependencies) {
  const inputSchema = z
    .object({
      projectID: resourceID,
      demand: z.unknown(),
      subscriptionRevision: z.number().int().nonnegative(),
      idempotencyKey: z
        .string()
        .min(16)
        .max(128)
        .regex(/^[A-Za-z0-9_-]+$/),
    })
    .strict();
  async function reserveHosting(actor: HostingActor, input: unknown) {
    const data = inputSchema.parse(input),
      demand = normalizeHostingAllowances(data.demand);
    if (
      hostingDimensions.some((k) => demand[k] === null) ||
      !demand.apps ||
      !demand.replicasPerApp ||
      !demand.concurrentDeployments
    )
      throw new HostingError(
        400,
        "Deployment demand must be finite and include apps, replicas and concurrency.",
      );
    return dependencies.transaction(async (db) => {
      let access = await dependencies.authorizeHosting(
        actor,
        {
          permission: "org:projects:manage",
          projectID: data.projectID,
          write: true,
        },
        db,
      );
      const customerID = access.customerID!;
      await dependencies.planLock(db, customerID);
      access = await dependencies.authorizeHosting(
        actor,
        {
          permission: "org:projects:manage",
          projectID: data.projectID,
          write: true,
        },
        db,
      );
      const clusterID = access.project?.cluster_id;
      if (!clusterID)
        throw new HostingError(409, "A k3s hosting project is required.");
      await dependencies.clusterLock(db, clusterID);
      const prior = await dependencies.replay(
        db,
        actor,
        data.idempotencyKey,
        data,
      );
      if (prior.existing)
        return { id: prior.existing.id, status: prior.existing.status };
      await checkDemand(db, {
        customerID,
        projectID: data.projectID,
        clusterID,
        provider: access.project.provider,
        demand,
        subscriptionRevision: data.subscriptionRevision,
      });
      const op = (
        await db.query(
          "INSERT INTO webdock_auth.hosting_operation(subject,customer_id,project_id,cluster_id,action,idempotency_key,payload_hash,status) VALUES($1,$2,$3,$4,'deploy',$5,$6,'queued') RETURNING id,status",
          [
            actor.subject,
            customerID,
            data.projectID,
            clusterID,
            data.idempotencyKey,
            prior.hash,
          ],
        )
      ).rows[0];
      await db.query(
        "INSERT INTO webdock_auth.hosting_reservation(operation_id,customer_id,project_id,cluster_id,demand,status) VALUES($1,$2,$3,$4,$5,'reserved')",
        [op.id, customerID, data.projectID, clusterID, JSON.stringify(demand)],
      );
      await dependencies.audit(db, actor, "resources.reserve", op.id);
      return op;
    });
  }
  async function checkDemand(
    db: import("./authorization").Connection,
    {
      customerID,
      projectID,
      clusterID,
      provider,
      demand,
      subscriptionRevision,
    }: {
      customerID: string;
      projectID: string;
      clusterID: string;
      provider: string;
      demand: import("@spitzli/hosting-contracts").HostingAllowances;
      subscriptionRevision: number;
    },
  ) {
    const cap = await limits(db, customerID, projectID, provider);
    if (cap.subscriptionRevision !== subscriptionRevision)
      throw new HostingError(
        409,
        "The subscription changed. Refresh and try again.",
      );
    const c = dependencies.env.DATABASE_RLS_SECRET
      ? (
          await db.query("SELECT webdock_rls.cluster_snapshot($1) AS cluster", [
            clusterID,
          ])
        ).rows[0]?.cluster
      : (
          await db.query(
            "SELECT c.verified,c.capacity,a.revoked,a.last_seen,a.observation FROM webdock_auth.hosting_cluster c JOIN webdock_auth.hosting_agent a ON a.cluster_id=c.id WHERE c.id=$1",
            [clusterID],
          )
        ).rows[0];
    if (
      !c?.verified ||
      c.revoked ||
      !c.last_seen ||
      Date.now() - new Date(c.last_seen).getTime() >= 90000
    )
      throw new HostingError(
        409,
        "Cluster readiness must be verified before allocation.",
      );
    const rows = (
      await db.query(
        "SELECT r.*,h.provider FROM webdock_auth.hosting_reservation r JOIN webdock_auth.hosting_project h ON h.project_id=r.project_id WHERE r.status<>'released' AND (r.customer_id=$1 OR r.cluster_id=$2)",
        [customerID, clusterID],
      )
    ).rows;
    const all = totals(rows.filter((r) => r.customer_id === customerID)),
      onCluster = dependencies.env.DATABASE_RLS_SECRET
        ? await dependencies.clusterCapacityDemand(db, clusterID, "apps")
        : totals(rows.filter((r) => r.cluster_id === clusterID));
    const mail = await dependencies.nativeClusterDemand(db, clusterID);
    for (const key of hostingDimensions)
      onCluster[key] =
        key === "replicasPerApp"
          ? Math.max(onCluster[key], mail[key])
          : onCluster[key] + mail[key];
    const clusterCap = normalizeHostingAllowances(
      c.capacity ?? c.observation?.capacity,
    );
    const globalCap = (await limits(db, customerID)).effective;
    for (const k of hostingDimensions) {
      const amount = demand[k]!;
      // The customer cap always counts ALL projects, even when this request has a narrower sublimit.
      const global = globalCap[k];
      if (
        amount > 0 &&
        global !== null &&
        (k === "replicasPerApp" ? amount : all[k] + amount) > global
      )
        throw new HostingError(409, "Customer hosting allowance exceeded.");
      for (const scope of cap.limits) {
        const ceiling = scope.values[k];
        if (ceiling === undefined || ceiling === null) continue;
        const relevant = rows.filter(
          (r) =>
            r.customer_id === customerID &&
            (scope.scope === "customer" ||
              scope.scope === `provider:${r.provider}` ||
              scope.scope === `project:${r.project_id}`),
        );
        if (
          scope.scope !== "customer" &&
          scope.scope !== `provider:${provider}` &&
          scope.scope !== `project:${projectID}`
        )
          continue;
        if (
          amount > 0 &&
          (k === "replicasPerApp" ? amount : totals(relevant)[k] + amount) >
            ceiling
        )
          throw new HostingError(
            409,
            "Project or provider hosting allowance exceeded.",
          );
      }
      if (
        amount > 0 &&
        (clusterCap[k] === null ||
          (k === "replicasPerApp" ? amount : onCluster[k] + amount) >
            clusterCap[k]!)
      )
        throw new HostingError(
          409,
          "Verified cluster capacity is insufficient.",
        );
    }
  }
  return { reserveHosting, checkDemand };
}
