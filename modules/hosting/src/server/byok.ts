import type { HostingPorts } from "./ports";
import { filterRows } from "@spitzli/search";
import type { VercelRuntime } from "./byok-vercel";
import {
  HostingError,
  byokPolicySchema,
  inventorySchema,
  type HostingActor,
  type HostingCommand,
  type Inventory,
} from "@spitzli/hosting-contracts";
import type { Connection } from "./authorization";
import type { Agent } from "./clusters";
export type Dependencies = {
  executeByokVercel: ReturnType<
    typeof import("./byok-vercel").createHostingByokVercel
  >["executeByokVercel"];
  executeByokMail: ReturnType<
    typeof import("./byok-mail").createHostingByokMail
  >["executeByokMail"];
  authorizeHosting: ReturnType<
    typeof import("./authorization").createHostingAuthorization
  >["authorizeHosting"];
  transaction: ReturnType<typeof import("./db").createHostingDb>["transaction"];
  audit: ReturnType<typeof import("./db").createHostingDb>["audit"];
  planLock: ReturnType<typeof import("./db").createHostingDb>["planLock"];
  clusterLock: ReturnType<typeof import("./db").createHostingDb>["clusterLock"];
  clusterView: ReturnType<
    typeof import("./clusters").createHostingClusters
  >["clusterView"];
  verifyAgent: ReturnType<
    typeof import("./clusters").createHostingClusters
  >["verifyAgent"];
  replay: ReturnType<
    typeof import("./operations").createHostingOperations
  >["replay"];
  recordResult: ReturnType<
    typeof import("./operations").createHostingOperations
  >["recordResult"];
};

export function createHostingByok(dependencies: Dependencies) {
  const disabled = {
    kubernetes: false,
    vercel: false,
    maxClusters: 0,
    manageExisting: false,
    namespaces: [] as string[],
  };
  async function byokPolicy(db: Connection, customerID: string) {
    const row = (
      await db.query(
        "SELECT * FROM webdock_auth.hosting_byok_policy WHERE customer_id=$1 FOR SHARE",
        [customerID],
      )
    ).rows[0];
    return {
      ...byokPolicySchema.parse(row?.policy ?? disabled),
      revision: row?.revision ?? 0,
    };
  }
  async function byokAccess(
    db: Connection,
    actor: HostingActor,
    customerID: string,
    write = false,
    provider: "kubernetes" | "vercel" | "turbosmtp" = "kubernetes",
  ) {
    const access = await dependencies.authorizeHosting(
      actor,
      {
        permission: "org:projects:manage",
        customerID,
        write,
        tenantAdmin: true,
      },
      db,
    );
    const policy = await byokPolicy(db, customerID);
    if (!policy[provider])
      throw new HostingError(
        403,
        "Own infrastructure is not enabled for this customer.",
      );
    return { ...access, policy };
  }
  async function byokCluster(
    db: Connection,
    actor: HostingActor,
    clusterID: string,
    write = false,
  ) {
    const row = (
      await db.query(
        "SELECT * FROM webdock_auth.hosting_cluster WHERE id=$1 AND ownership='tenant'",
        [clusterID],
      )
    ).rows[0];
    if (!row) throw new HostingError(404, "Cluster is unavailable.");
    const access = await byokAccess(
      db,
      actor,
      row.dedicated_customer_id,
      write,
    );
    return { row, ...access };
  }
  function filtered(inventory: Inventory | null, allowed: string[]) {
    return (
      inventory?.resources.filter((r) =>
        r.namespace
          ? allowed.includes(r.namespace)
          : r.kind !== "Namespace" || allowed.includes(r.name),
      ) ?? []
    );
  }
  async function executeByok(
    actor: HostingActor,
    cmd: HostingCommand,
    runtime?: VercelRuntime,
  ) {
    if (cmd.action.startsWith("byok.mail.")) {
      return dependencies.executeByokMail(actor, cmd);
    }
    if (cmd.action.startsWith("byok.vercel.")) {
      return dependencies.executeByokVercel(actor, cmd, runtime);
    }
    return dependencies.transaction(async (db) => {
      if (cmd.action === "byok.reconcile") {
        await dependencies.authorizeHosting(
          actor,
          { permission: "org:projects:manage", operator: true, write: true },
          db,
        );
        const op = (
          await db.query(
            "SELECT * FROM webdock_auth.hosting_operation WHERE id=$1 AND action='byok.workload' FOR UPDATE",
            [cmd.operationID],
          )
        ).rows[0];
        if (!op || op.status !== "needs-reconciliation")
          throw new HostingError(
            409,
            "This operation does not need reconciliation.",
          );
        const { row, policy } = await byokCluster(
          db,
          actor,
          op.cluster_id,
          true,
        );
        const inventory = row.inventory as Inventory | null;
        const resource = filtered(inventory, policy.namespaces).find(
          (r) => r.uid === op.desired.resource.uid,
        );
        if (
          !inventory ||
          Date.now() - Date.parse(inventory.observedAt) > 180000 ||
          !resource ||
          resource.resourceVersion !== cmd.resourceVersion
        )
          throw new HostingError(
            409,
            "Refresh the cluster inventory before reviewing this operation.",
          );
        await db.query(
          "UPDATE webdock_auth.hosting_operation SET status='failed',result='{\"error\":\"Current resource state reviewed. A new action can be requested.\"}' WHERE id=$1",
          [op.id],
        );
        await dependencies.audit(db, actor, cmd.action, op.id);
        return { reviewed: true };
      }
      if (
        cmd.action === "byok.policy.get" ||
        cmd.action === "byok.policy.set" ||
        cmd.action === "byok.list" ||
        cmd.action === "byok.register"
      ) {
        const access = await dependencies.authorizeHosting(
          actor,
          {
            permission: "org:projects:manage",
            customerID: cmd.customerID,
            write:
              cmd.action === "byok.policy.set" ||
              cmd.action === "byok.register",
            tenantAdmin: true,
            operator: cmd.action === "byok.policy.set",
          },
          db,
        );
        if (cmd.action === "byok.policy.set" || cmd.action === "byok.register")
          await dependencies.planLock(db, cmd.customerID);
        const policy = await byokPolicy(db, cmd.customerID);
        if (cmd.action === "byok.policy.get")
          return {
            ...policy,
            operator: access.operator,
            canWrite: access.canWrite && access.liveReads,
          };
        if (cmd.action === "byok.policy.set") {
          if (policy.revision !== cmd.revision)
            throw new HostingError(
              409,
              "Settings changed. Refresh and try again.",
            );
          const { action, customerID, ...input } = cmd;
          Reflect.deleteProperty(input, "revision");
          const value = {
            ...input,
            turbosmtp: input.turbosmtp ?? policy.turbosmtp,
            maxMailDomains: input.maxMailDomains ?? policy.maxMailDomains,
          };
          const result = (
            await db.query(
              "INSERT INTO webdock_auth.hosting_byok_policy(customer_id,policy) VALUES($1,$2) ON CONFLICT(customer_id) DO UPDATE SET policy=$2,revision=hosting_byok_policy.revision+1 RETURNING revision",
              [customerID, JSON.stringify(value)],
            )
          ).rows[0];
          await dependencies.audit(db, actor, action, customerID);
          return { ...value, revision: result.revision };
        }
        if (cmd.action === "byok.list") {
          const rows = policy.kubernetes
            ? (
                await db.query(
                  "SELECT id FROM webdock_auth.hosting_cluster WHERE ownership='tenant' AND dedicated_customer_id=$1 ORDER BY name LIMIT 100",
                  [cmd.customerID],
                )
              ).rows
            : [];
          const vercel = (
            await db.query(
              "SELECT team_id,revision,projects,connected_at FROM webdock_auth.hosting_vercel WHERE customer_id=$1",
              [cmd.customerID],
            )
          ).rows[0];
          return {
            policy,
            operator: access.operator,
            canWrite: access.canWrite && access.liveReads,
            clusters: await Promise.all(
              rows.map((r) => dependencies.clusterView(db, r.id)),
            ),
            vercel: policy.vercel ? (vercel ?? null) : null,
          };
        }
        if (!policy.kubernetes)
          throw new HostingError(
            403,
            "Own infrastructure is not enabled for this customer.",
          );
        const prior = await dependencies.replay(
          db,
          actor,
          cmd.idempotencyKey,
          cmd,
        );
        if (prior.existing) return prior.existing.result;
        const count = (
          await db.query(
            "SELECT count(*)::int AS n FROM webdock_auth.hosting_cluster WHERE ownership='tenant' AND dedicated_customer_id=$1",
            [cmd.customerID],
          )
        ).rows[0].n;
        if (count >= policy.maxClusters)
          throw new HostingError(
            409,
            "The customer cluster limit has been reached.",
          );
        const row = (
          await db.query(
            "INSERT INTO webdock_auth.hosting_cluster(name,provider,region,location_evidence,dedicated_customer_id,ownership) VALUES($1,$2,$3,$4,$5,'tenant') RETURNING id",
            [
              cmd.name,
              cmd.provider,
              cmd.region,
              cmd.locationEvidence,
              cmd.customerID,
            ],
          )
        ).rows[0];
        const result = await dependencies.clusterView(db, row.id);
        await dependencies.recordResult(
          db,
          actor,
          cmd.idempotencyKey,
          prior.hash,
          cmd.action,
          result,
          { customerID: cmd.customerID, clusterID: row.id },
        );
        await dependencies.audit(db, actor, cmd.action, row.id);
        return result;
      }
      if (!("clusterID" in cmd) || !cmd.clusterID)
        throw new HostingError(400, "Invalid cluster request.");
      await dependencies.clusterLock(db, cmd.clusterID);
      const write = !["byok.cluster", "byok.inventory"].includes(cmd.action);
      const { row, policy, canWrite, liveReads } = await byokCluster(
        db,
        actor,
        cmd.clusterID,
        write,
      );
      if (cmd.action === "byok.cluster")
        return {
          ...(await dependencies.clusterView(db, row.id)),
          policy,
          canWrite: canWrite && liveReads,
        };
      if (cmd.action === "byok.inventory") {
        const inventory = row.inventory as Inventory | null;
        const candidates = filtered(inventory, policy.namespaces).filter(
          (r) =>
            (cmd.view !== "applications" ||
              ["Deployment", "StatefulSet", "DaemonSet", "CronJob"].includes(
                r.kind,
              )) &&
            (!cmd.namespace || r.namespace === cmd.namespace) &&
            (!cmd.kind || r.kind === cmd.kind),
        );
        const resources = filterRows(candidates, cmd.search, (r) =>
          [r.name, r.namespace, r.kind].join(" "),
        );
        return {
          observedAt: inventory?.observedAt ?? null,
          complete: inventory?.complete ?? false,
          unavailable: inventory?.unavailable ?? [],
          stale:
            !inventory ||
            Date.now() - Date.parse(inventory.observedAt) > 180000,
          resources: resources.slice((cmd.page - 1) * 50, cmd.page * 50),
          total: resources.length,
          page: cmd.page,
          canWrite: canWrite && liveReads && policy.manageExisting,
        };
      }
      if (cmd.action === "byok.enrollment") {
        const n = (
          await db.query(
            "SELECT count(*)::int AS n FROM webdock_auth.hosting_enrollment WHERE cluster_id=$1 AND created_at>now()-interval '5 minutes'",
            [row.id],
          )
        ).rows[0].n;
        if (n >= 3)
          throw new HostingError(
            429,
            "Wait before creating another enrollment.",
          );
        const enrollment = (
          await db.query(
            "INSERT INTO webdock_auth.hosting_enrollment(cluster_id,actor_id,expires_at) VALUES($1,$2,now()+interval '5 minutes') RETURNING id",
            [row.id, actor.subject],
          )
        ).rows[0];
        await dependencies.audit(db, actor, cmd.action, row.id);
        return {
          enrollmentID: enrollment.id,
          setupPath: `/hosting/clusters/${row.id}/enroll/${enrollment.id}`,
        };
      }
      if (cmd.action === "byok.revoke") {
        if (row.revision !== cmd.revision)
          throw new HostingError(
            409,
            "Settings changed. Refresh and try again.",
          );
        await db.query(
          "UPDATE webdock_auth.hosting_agent SET revoked=true WHERE cluster_id=$1",
          [row.id],
        );
        await db.query(
          "UPDATE webdock_auth.hosting_cluster SET verified=false,revision=revision+1 WHERE id=$1",
          [row.id],
        );
        await db.query(
          "UPDATE webdock_auth.hosting_enrollment SET consumed_at=now() WHERE cluster_id=$1 AND consumed_at IS NULL",
          [row.id],
        );
        await dependencies.audit(db, actor, cmd.action, row.id);
        return { revoked: true };
      }
      if (cmd.action === "byok.scan") {
        await db.query(
          "UPDATE webdock_auth.hosting_cluster SET scan_requested=true WHERE id=$1",
          [row.id],
        );
        await dependencies.audit(db, actor, cmd.action, row.id);
        return { status: "queued" };
      }
      if (cmd.action === "byok.workload") {
        if (!policy.manageExisting)
          throw new HostingError(
            403,
            "Existing application management is disabled.",
          );
        const inventory = row.inventory as Inventory | null;
        const target = filtered(inventory, policy.namespaces).find(
          (r) => r.uid === cmd.uid && r.resourceVersion === cmd.resourceVersion,
        );
        const cluster = await dependencies.clusterView(db, row.id);
        if (
          !target ||
          !inventory ||
          Date.now() - Date.parse(inventory.observedAt) > 180000 ||
          cluster.state !== "connected"
        )
          throw new HostingError(
            409,
            "Refresh the cluster inventory before changing this application.",
          );
        if (!target.namespace)
          throw new HostingError(403, "System resources cannot be managed.");
        const allowed =
          cmd.operation === "logs"
            ? ["Deployment", "StatefulSet", "DaemonSet", "Pod", "Job"]
            : ["suspend", "resume"].includes(cmd.operation)
              ? ["CronJob"]
              : cmd.operation === "restart"
                ? ["Deployment", "StatefulSet", "DaemonSet"]
                : ["Deployment", "StatefulSet"];
        if (
          !allowed.includes(target.kind) ||
          (cmd.operation !== "logs" && target.controlled)
        )
          throw new HostingError(
            409,
            "This action is controlled by another controller or is unsupported.",
          );
        if (cmd.operation === "scale" && cmd.replicas === undefined)
          throw new HostingError(400, "Choose the number of instances.");
        const prior = await dependencies.replay(
          db,
          actor,
          cmd.idempotencyKey,
          cmd,
        );
        if (prior.existing)
          return {
            operationID: prior.existing.id,
            status: prior.existing.status,
          };
        const busy = (
          await db.query(
            "SELECT 1 FROM webdock_auth.hosting_operation WHERE cluster_id=$1 AND desired->'resource'->>'uid'=$2 AND status IN ('queued','running','needs-reconciliation')",
            [row.id, target.uid],
          )
        ).rowCount;
        if (busy)
          throw new HostingError(
            409,
            "An operation is already in progress for this application.",
          );
        let replicas = cmd.replicas;
        if (cmd.operation === "stop") {
          if (!target.replicas)
            throw new HostingError(409, "The application is already stopped.");
          await db.query(
            "INSERT INTO webdock_auth.hosting_external_state(cluster_id,uid,previous_replicas) VALUES($1,$2,$3) ON CONFLICT(cluster_id,uid) DO UPDATE SET previous_replicas=$3",
            [row.id, target.uid, target.replicas],
          );
          replicas = 0;
        }
        if (cmd.operation === "start") {
          const old = (
            await db.query(
              "SELECT previous_replicas FROM webdock_auth.hosting_external_state WHERE cluster_id=$1 AND uid=$2",
              [row.id, target.uid],
            )
          ).rows[0];
          if (!old || target.replicas !== 0)
            throw new HostingError(
              409,
              "Use Scale to choose the desired number of instances.",
            );
          replicas = old.previous_replicas;
        }
        const desired = {
          protocol: "byok-v1",
          clusterID: row.id,
          clusterUID: row.cluster_uid,
          resource: target,
          action: cmd.operation,
          replicas,
        };
        const operation = (
          await db.query(
            "INSERT INTO webdock_auth.hosting_operation(subject,actor_session,idempotency_key,payload_hash,action,status,customer_id,cluster_id,desired) VALUES($1,$2,$3,$4,'byok.workload','queued',$5,$6,$7) RETURNING id",
            [
              actor.subject,
              actor.sessionID,
              cmd.idempotencyKey,
              prior.hash,
              row.dedicated_customer_id,
              row.id,
              JSON.stringify(desired),
            ],
          )
        ).rows[0];
        await dependencies.audit(db, actor, cmd.action, row.id);
        return { operationID: operation.id, status: "queued" };
      }
      throw new HostingError(400, "Invalid BYOK request.");
    });
  }
  async function saveInventory(agent: Agent, input: unknown) {
    const inventory = inventorySchema.parse(input);
    if (Math.abs(Date.now() - Date.parse(inventory.observedAt)) > 90000)
      throw new HostingError(409, "Inventory is stale.");
    return dependencies.transaction(async (db) => {
      await dependencies.verifyAgent(db, agent);
      await dependencies.clusterLock(db, agent.clusterID);
      const row = (
        await db.query(
          "SELECT * FROM webdock_auth.hosting_cluster WHERE id=$1 AND ownership='tenant'",
          [agent.clusterID],
        )
      ).rows[0];
      if (!row)
        throw new HostingError(
          403,
          "Inventory is not enabled for this cluster.",
        );
      const policy = await byokPolicy(db, row.dedicated_customer_id);
      if (!policy.kubernetes)
        throw new HostingError(403, "Own infrastructure is disabled.");
      if (row.cluster_uid && row.cluster_uid !== inventory.clusterUID)
        throw new HostingError(409, "Cluster identity changed.");
      if (
        row.inventory &&
        Date.parse(row.inventory.observedAt) >= Date.parse(inventory.observedAt)
      )
        throw new HostingError(409, "Inventory is stale.");
      const safe = {
        ...inventory,
        resources: filtered(inventory, policy.namespaces),
      };
      await db.query(
        "UPDATE webdock_auth.hosting_cluster SET cluster_uid=$2,inventory=$3,scan_requested=false WHERE id=$1",
        [row.id, inventory.clusterUID, JSON.stringify(safe)],
      );
      return { accepted: true };
    });
  }
  async function byokAgentConfig(agent: Agent) {
    return dependencies.transaction(async (db) => {
      await dependencies.verifyAgent(db, agent);
      const c = (
        await db.query(
          "SELECT * FROM webdock_auth.hosting_cluster WHERE id=$1 AND ownership='tenant'",
          [agent.clusterID],
        )
      ).rows[0];
      if (!c)
        throw new HostingError(403, "This cluster is not customer owned.");
      const policy = await byokPolicy(db, c.dedicated_customer_id);
      if (!policy.kubernetes)
        throw new HostingError(403, "Own infrastructure is disabled.");
      return {
        namespaces: policy.namespaces,
        clusterUID: c.cluster_uid,
        scanRequested: c.scan_requested,
      };
    });
  }
  async function claimByokOperation(agent: Agent) {
    return dependencies.transaction(async (db) => {
      await dependencies.verifyAgent(db, agent);
      await dependencies.clusterLock(db, agent.clusterID);
      await db.query(
        "UPDATE webdock_auth.hosting_operation SET status='needs-reconciliation',result='{\"error\":\"Operation timed out. Refresh inventory before retrying.\"}' WHERE cluster_id=$1 AND action='byok.workload' AND status='running' AND lease_until<now()",
        [agent.clusterID],
      );
      await db.query(
        "UPDATE webdock_auth.hosting_operation SET status='failed',result='{\"error\":\"Operation expired before execution. Refresh inventory and try again.\"}' WHERE cluster_id=$1 AND action='byok.workload' AND status='queued' AND created_at<now()-interval '3 minutes'",
        [agent.clusterID],
      );
      const row = (
        await db.query(
          "SELECT * FROM webdock_auth.hosting_operation WHERE cluster_id=$1 AND action='byok.workload' AND status='queued' ORDER BY created_at,id FOR UPDATE SKIP LOCKED LIMIT 1",
          [agent.clusterID],
        )
      ).rows[0];
      if (!row) return null;
      const actor: HostingActor = {
        subject: row.subject,
        sessionID: row.actor_session,
        source: "studio",
        scopes: ["hosting:read", "hosting:write"],
      };
      try {
        const access = await byokCluster(db, actor, agent.clusterID, true);
        if (
          !access.policy.manageExisting ||
          !access.policy.namespaces.includes(row.desired.resource.namespace) ||
          access.row.cluster_uid !== row.desired.clusterUID
        )
          throw new HostingError(403, "Management permission changed.");
      } catch (e) {
        if (!(e instanceof HostingError)) throw e;
        await db.query(
          "UPDATE webdock_auth.hosting_operation SET status='failed',result='{\"error\":\"Management permission changed.\"}' WHERE id=$1",
          [row.id],
        );
        return null;
      }
      const op = (
        await db.query(
          "UPDATE webdock_auth.hosting_operation SET status='running',generation=generation+1,lease_until=now()+interval '180 seconds' WHERE id=$1 RETURNING id,generation,lease_until",
          [row.id],
        )
      ).rows[0];
      return {
        ...op,
        desired: {
          ...row.desired,
          operationID: op.id,
          generation: op.generation,
          leaseUntil: op.lease_until.toISOString(),
        },
      };
    });
  }
  async function completeByokOperation(
    agent: Agent,
    input: {
      id: string;
      generation: number;
      outcome: "succeeded" | "failed";
      proof?: unknown;
    },
  ) {
    return dependencies.transaction(async (db) => {
      await dependencies.verifyAgent(db, agent);
      await dependencies.clusterLock(db, agent.clusterID);
      const op = (
        await db.query(
          "SELECT * FROM webdock_auth.hosting_operation WHERE id=$1 AND cluster_id=$2 AND action='byok.workload' FOR UPDATE",
          [input.id, agent.clusterID],
        )
      ).rows[0];
      if (
        !op ||
        op.generation !== input.generation ||
        op.status !== "running" ||
        Date.parse(op.lease_until) <= Date.now()
      )
        throw new HostingError(409, "Operation lease changed.");
      const proof = input.proof as
        | {
            uid?: unknown;
            action?: unknown;
            observed?: unknown;
            logs?: unknown;
          }
        | undefined;
      if (
        input.outcome === "succeeded" &&
        (!proof ||
          proof.uid !== op.desired.resource.uid ||
          proof.action !== op.desired.action ||
          proof.observed !== true ||
          (proof.logs !== undefined &&
            (typeof proof.logs !== "string" || proof.logs.length > 16000)))
      )
        throw new HostingError(
          409,
          "Resource observation does not match the requested operation.",
        );
      const status =
        input.outcome === "succeeded"
          ? "succeeded"
          : op.desired.action === "logs"
            ? "failed"
            : "needs-reconciliation";
      const result =
        input.outcome === "succeeded"
          ? {
              uid: proof!.uid,
              observed: true,
              ...(op.desired.action === "logs"
                ? { logs: proof!.logs ?? "" }
                : {}),
            }
          : {
              error:
                "The operation did not finish. Refresh inventory and review the resource before retrying.",
            };
      await db.query(
        "UPDATE webdock_auth.hosting_operation SET status=$2,result=$3 WHERE id=$1",
        [op.id, status, JSON.stringify(result)],
      );
      await db.query(
        "INSERT INTO webdock_auth.hosting_audit(subject,action,target_id,outcome) VALUES($1,'byok.complete',$2,$3)",
        [op.subject, op.id, status],
      );
      return { id: op.id, status };
    });
  }
  return {
    byokPolicy,
    byokAccess,
    byokCluster,
    executeByok,
    saveInventory,
    byokAgentConfig,
    claimByokOperation,
    completeByokOperation,
  };
}
