import type { DatabaseRuntimePorts } from "./ports";
import { randomBytes, createHash } from "node:crypto";
import {
  databasePlanSchema,
  databasePlanAvailableTo,
  HostingError,
  resourceID,
  type HostingActor,
  type DatabaseCommand,
  type DatabasePlan,
  type DatabaseView,
  type HostingDemand,
  type HostingAllowances,
  type ClusterObservation,
  hostingDimensions,
  normalizeHostingAllowances,
} from "@spitzli/hosting-contracts";
import type { Connection } from "@spitzli/hosting-contracts/runtime-ports";
import { allocationTotals as totals } from "@spitzli/hosting-contracts/capacity-totals";
export type Dependencies = {
  nativeClusterDemand: DatabaseRuntimePorts["nativeClusterDemand"];
  clusterCapacityDemand: DatabaseRuntimePorts["clusterCapacityDemand"];
  databasePoolCapacity: DatabaseRuntimePorts["databasePoolCapacity"];

  assertDatabaseMaintenanceAllowed: ReturnType<
    typeof import("./database-protection").createHostingDatabaseProtection
  >["assertDatabaseMaintenanceAllowed"];
  authorizeHosting: DatabaseRuntimePorts["authorizeHosting"];
  clusterLock: DatabaseRuntimePorts["clusterLock"];
  audit: DatabaseRuntimePorts["audit"];
  replay: DatabaseRuntimePorts["replay"];
  recordResult: DatabaseRuntimePorts["recordResult"];
  encryptSecret: DatabaseRuntimePorts["encryptSecret"];
  env: DatabaseRuntimePorts["env"];
};
type ClusterRecord = {
  id: string;
  revision: number;
  ownership: string;
  verified: boolean;
  revoked: boolean;
  last_seen: Date | null;
  capacity: HostingAllowances | null;
  observation: ClusterObservation | null;
};
type OfferRecord = {
  id: string;
  project_id: string;
  customer_id: string;
  plan_revision_id: string;
  snapshot: DatabasePlan & {
    sourceDatabaseID?: string;
    sourceRevision?: number;
  };
};
type BackendRecord = {
  id: string;
  cluster_id: string;
  node: string;
  pool_id: string;
  kind: "shared-pool" | "dedicated";
  cnpg_uid: string | null;
  demand: HostingDemand;
  logical_capacity_bytes: string;
};
type SourceRecord = {
  id: string;
  name: string;
  customer_id: string;
  project_id: string;
  backend_id: string;
  kind: "shared" | "dedicated";
  revision: number;
  pending_order_id: string | null;
  plan: DatabasePlan;
  order_id: string;
  plan_revision_id: string;
  credential_revision: number;
  credentials_encrypted: string;
  usage: DatabaseView["usage"];
  state: string;
};
export function createHostingDatabaseOrders(dependencies: Dependencies) {
  async function freshDatabaseCluster(db: Connection, clusterID: string) {
    const c: ClusterRecord | undefined = dependencies.env.DATABASE_RLS_SECRET
      ? (
          await db.query("SELECT webdock_rls.cluster_snapshot($1) AS cluster", [
            resourceID.parse(clusterID),
          ])
        ).rows[0]?.cluster
      : (
          await db.query<ClusterRecord>(
            "SELECT c.*,a.last_seen,a.revoked,a.observation FROM webdock_auth.hosting_cluster c JOIN webdock_auth.hosting_agent a ON a.cluster_id=c.id WHERE c.id=$1",
            [resourceID.parse(clusterID)],
          )
        ).rows[0];
    if (
      !c ||
      c.ownership !== "platform" ||
      !c.verified ||
      c.revoked ||
      !c.last_seen ||
      Date.now() - new Date(c.last_seen).getTime() >= 90000 ||
      c.observation?.capabilities?.databaseVersion !== 1
    )
      throw new HostingError(
        409,
        "A fresh qualified PostgreSQL agent is required.",
      );
    return {
      ...c,
      capacity: normalizeHostingAllowances(c.capacity ?? undefined),
      observation: c.observation!,
    };
  }
  function databaseCredentials() {
    return {
      migrationPassword: randomBytes(32).toString("base64url"),
      applicationPassword: randomBytes(32).toString("base64url"),
    };
  }
  async function enqueueDatabase(
    db: Connection,
    actor: HostingActor,
    backendID: string,
    databaseID: string | null,
    action: string,
    revision: number,
    setCurrentOperation = true,
  ) {
    const backend = (
      await db.query(
        "SELECT cluster_id FROM webdock_auth.hosting_database_backend WHERE id=$1",
        [backendID],
      )
    ).rows[0];
    const target = databaseID
      ? (
          await db.query(
            "SELECT customer_id,project_id FROM webdock_auth.hosting_database WHERE id=$1",
            [databaseID],
          )
        ).rows[0]
      : {};
    const token = randomBytes(18).toString("hex");
    const operation = (
      await db.query(
        `INSERT INTO webdock_auth.hosting_operation(subject,customer_id,project_id,cluster_id,action,idempotency_key,payload_hash,status,desired,target_revision,actor_session)
 VALUES($1,$2,$3,$4,$5,$6,$7,'queued',$8,$9,$10) RETURNING id`,
        [
          actor.subject,
          target.customer_id ?? null,
          target.project_id ?? null,
          backend.cluster_id,
          "databases." + action,
          "database-job-" + token,
          createHash("sha256").update(token).digest("hex"),
          JSON.stringify({
            databaseVersion: 1,
            backendID,
            databaseID,
            action,
            revision,
            ...(actor.consoleIdentity
              ? { consoleIdentity: actor.consoleIdentity }
              : {}),
          }),
          revision,
          actor.sessionID,
        ],
      )
    ).rows[0];
    await db.query(
      "INSERT INTO webdock_auth.hosting_database_job(operation_id,backend_id,database_id,action,revision) VALUES($1,$2,$3,$4,$5)",
      [operation.id, backendID, databaseID, action, revision],
    );
    if (databaseID && setCurrentOperation)
      await db.query(
        "UPDATE webdock_auth.hosting_database SET operation_id=$2 WHERE id=$1",
        [databaseID, operation.id],
      );
    return operation.id as string;
  }
  async function createDatabaseBackend(
    db: Connection,
    clusterID: string,
    kind: "shared-pool" | "dedicated",
    plan?: DatabasePlan,
    selected?: {
      node: string;
      poolID: string;
    },
  ) {
    await dependencies.clusterLock(db, clusterID);
    const c = await freshDatabaseCluster(db, clusterID);
    const shared = kind === "shared-pool";
    const pg = {
      cpuMillicores: shared ? 1000 : plan!.cpuMillicores!,
      memoryBytes: shared ? 2147483648 : plan!.memoryBytes!,
      volumeBytes: shared ? 32212254720 : plan!.volumeBytes!,
    };
    // Pooler and job headroom are physical obligations, including while PostgreSQL is paused.
    const demand = {
      apps: 2,
      cpuMillicores: pg.cpuMillicores + 200,
      memoryBytes: pg.memoryBytes + 536870912,
      volumeBytes: pg.volumeBytes,
      ephemeralBytes: 536870912,
      replicasPerApp: 1,
      concurrentDeployments: 1,
    };
    const apps = dependencies.env.DATABASE_RLS_SECRET
      ? await dependencies.clusterCapacityDemand(db, clusterID, "apps")
      : totals(
          (
            await db.query(
              "SELECT demand,status FROM webdock_auth.hosting_reservation WHERE cluster_id=$1 AND status<>'released'",
              [clusterID],
            )
          ).rows,
        );
    const native = await dependencies.nativeClusterDemand(db, clusterID);
    const cap = normalizeHostingAllowances(c.capacity);
    for (const k of hostingDimensions) {
      const used =
        k === "replicasPerApp"
          ? Math.max(apps[k], native[k], demand[k])
          : apps[k] + native[k] + (k === "volumeBytes" ? 0 : demand[k]);
      if (cap[k] === null || used > cap[k]!)
        throw new HostingError(
          409,
          "Cluster capacity is insufficient for this database.",
        );
    }
    const physical = dependencies.env.DATABASE_RLS_SECRET
      ? []
      : (
          await db.query(
            "SELECT id,node,pool_id,demand FROM webdock_auth.hosting_database_backend WHERE cluster_id=$1 AND state<>'purged'",
            [clusterID],
          )
        ).rows;
    const aggregatePools = dependencies.env.DATABASE_RLS_SECRET
      ? await dependencies.databasePoolCapacity(db, clusterID)
      : null;
    const registered =
      aggregatePools ??
      (
        await db.query(
          "SELECT node,pool_id,capacity_bytes FROM webdock_auth.hosting_database_pool WHERE cluster_id=$1",
          [clusterID],
        )
      ).rows;
    const pools = c.observation.capabilities.databasePools ?? [];
    const pool = pools.find((p) => {
      if (
        !p.ready ||
        !registered.some(
          (r) =>
            r.node === p.node &&
            r.pool_id === p.poolID &&
            Number(r.capacity_bytes) === p.capacityBytes,
        ) ||
        (selected && (p.node !== selected.node || p.poolID !== selected.poolID))
      )
        return false;
      const aggregated = aggregatePools?.find(
        (r) => r.node === p.node && r.pool_id === p.poolID,
      );
      const held = aggregated
        ? {
            cpu: aggregated.held_cpu,
            memory: aggregated.held_memory,
            volume: aggregated.held_volume,
          }
        : physical
            .filter((r) => r.node === p.node)
            .reduce(
              (n, r) => ({
                cpu: n.cpu + r.demand.cpuMillicores,
                memory: n.memory + r.demand.memoryBytes,
                volume:
                  n.volume +
                  (r.pool_id === p.poolID ? r.demand.volumeBytes : 0),
              }),
              { cpu: 0, memory: 0, volume: 0 },
            );
      const node = c.observation.nodes.find((n) => n.id === p.node);
      if (!node) return false;
      const known = new Set(
        physical.filter((r) => r.node === p.node).map((r) => r.id),
      );
      const actual = (p.observedDatabases ?? []).filter((r) =>
        known.has(r.installationID),
      );
      const knownCPU =
          aggregated?.known_cpu ??
          actual.reduce((sum, r) => sum + r.cpuMillicores, 0),
        knownMemory =
          aggregated?.known_memory ??
          actual.reduce((sum, r) => sum + r.memoryBytes, 0);
      const allocated =
        aggregated?.allocated ??
        (p.allocations ?? [])
          .filter((r) => known.has(r.installationID))
          .reduce((sum, r) => sum + r.volumeBytes, 0);
      const unknownVolume = Math.max(0, (p.allocatedBytes ?? 0) - allocated),
        pending = Math.max(0, held.volume - allocated);
      return (
        Math.max(0, p.requestedCpuMillicores - knownCPU) +
          held.cpu +
          apps.cpuMillicores +
          demand.cpuMillicores <=
          Math.floor(node.cpuMillicores * 0.8) &&
        Math.max(0, p.requestedMemoryBytes - knownMemory) +
          held.memory +
          apps.memoryBytes +
          demand.memoryBytes <=
          Math.floor(node.memoryBytes * 0.8) &&
        held.volume + unknownVolume + demand.volumeBytes <= p.capacityBytes &&
        p.availableBytes >= pending + demand.volumeBytes
      );
    });
    if (!pool)
      throw new HostingError(
        409,
        "No qualified database node and volume pool has sufficient capacity.",
      );
    const suffix = dependencies.env.SPITZLI_DATABASE_DOMAIN;
    if (
      !suffix ||
      suffix.length > 220 ||
      !/^([a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,}$/.test(suffix)
    )
      throw new HostingError(
        503,
        "Database endpoint domain is not configured.",
      );
    const used = new Set<number>([
      ...(c.observation.capabilities.databaseNodePorts ?? []),
      ...(
        await db.query(
          "SELECT port FROM webdock_auth.hosting_database_port WHERE cluster_id=$1",
          [clusterID],
        )
      ).rows.map((r) => r.port),
    ]);
    const ports = [];
    for (let port = 32000; port <= 32199 && ports.length < 2; port++)
      if (!used.has(port)) ports.push(port);
    if (ports.length !== 2)
      throw new HostingError(409, "Database endpoint capacity is unavailable.");
    const id = (await db.query("SELECT webdock_auth.next_snowflake() AS id"))
      .rows[0].id;
    const backend = (
      await db.query(
        `INSERT INTO webdock_auth.hosting_database_backend(id,cluster_id,kind,node,pool_id,demand,profile,max_databases,logical_capacity_bytes,host,direct_port,pooled_port)
 VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) RETURNING *`,
        [
          id,
          clusterID,
          kind,
          pool.node,
          pool.poolID,
          JSON.stringify(demand),
          JSON.stringify(pg),
          shared ? 8 : 1,
          shared ? 8589934592 : 0,
          `db-${id}.${suffix}`,
          ports[0],
          ports[1],
        ],
      )
    ).rows[0];
    for (const port of ports)
      await db.query(
        "INSERT INTO webdock_auth.hosting_database_port(cluster_id,port,backend_id) VALUES($1,$2,$3)",
        [clusterID, port, id],
      );
    return backend;
  }
  async function acceptDatabaseOrder(
    db: Connection,
    actor: HostingActor,
    cmd: Extract<
      DatabaseCommand,
      {
        action: "databases.acceptOrder";
      }
    >,
  ) {
    const access = await dependencies.authorizeHosting(
      actor,
      {
        permission: "org:databases:manage",
        projectID: cmd.projectID,
        write: true,
        tenantAdmin: true,
      },
      db,
    );
    const prior = await dependencies.replay(db, actor, cmd.idempotencyKey, cmd);
    if (prior.existing) return prior.existing.result;
    const offer = (
      await db.query(
        "SELECT * FROM webdock_auth.hosting_database_offer WHERE id=$1 AND project_id=$2 AND customer_id=$3 FOR UPDATE",
        [cmd.offerID, cmd.projectID, access.customerID],
      )
    ).rows[0];
    if (!offer) throw new HostingError(404, "Database offer is unavailable.");
    const previous = (
      await db.query(
        'SELECT o.id AS "orderID",d.id AS "databaseID",d.operation_id AS "operationID" FROM webdock_auth.hosting_database_order o JOIN webdock_auth.hosting_database d ON d.id=o.database_id WHERE o.offer_id=$1',
        [offer.id],
      )
    ).rows[0];
    if (previous) {
      await dependencies.recordResult(
        db,
        actor,
        cmd.idempotencyKey,
        prior.hash,
        cmd.action,
        previous,
        { customerID: access.customerID, projectID: cmd.projectID },
      );
      return previous;
    }
    const planRow = (
      await db.query(
        "SELECT * FROM webdock_auth.hosting_database_plan WHERE id=$1 FOR SHARE",
        [offer.plan_revision_id],
      )
    ).rows[0];
    if (
      !planRow ||
      planRow.availability === "disabled" ||
      (planRow.availability === "pilot" && !access.operator) ||
      new Date(offer.expires_at).getTime() <= Date.now()
    )
      throw new HostingError(
        409,
        "Database offer expired or is no longer available.",
      );
    if (
      planRow.availability === "public" &&
      dependencies.env.SPITZLI_DATABASE_PUBLIC_ENABLED !== "true"
    )
      throw new HostingError(409, "Database ordering is not enabled.");
    const plan = databasePlanSchema.parse(planRow.plan);
    if (
      !databasePlanAvailableTo(
        plan,
        resourceID.parse(access.customerID),
        planRow.availability,
      )
    )
      throw new HostingError(
        403,
        "This database sponsorship belongs to another customer or is not private.",
      );
    const source = offer.snapshot.sourceDatabaseID
      ? (
          await db.query(
            "SELECT b.cluster_id FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id WHERE d.id=$1 AND d.project_id=$2",
            [offer.snapshot.sourceDatabaseID, cmd.projectID],
          )
        ).rows[0]
      : null;
    const clusterID =
      source?.cluster_id ??
      resourceID.parse(dependencies.env.SPITZLI_DATABASE_CLUSTER_ID);
    await dependencies.clusterLock(db, clusterID);
    await freshDatabaseCluster(db, clusterID);
    if (offer.snapshot.sourceDatabaseID)
      return acceptUpgrade(db, actor, cmd, offer, plan, clusterID, prior.hash);
    // The cluster lock serializes placement and shared-slot allocation across customer projects.
    let backend;
    if (plan.kind === "shared") {
      const candidates = (
        await db.query(
          "SELECT * FROM webdock_auth.hosting_database_backend WHERE cluster_id=$1 AND kind='shared-pool' AND state='ready' AND observed_revision=revision ORDER BY id FOR UPDATE",
          [clusterID],
        )
      ).rows;
      for (const candidate of candidates) {
        const use = dependencies.env.DATABASE_RLS_SECRET
          ? (
              await db.query("SELECT * FROM webdock_rls.capacity_slots($1)", [
                candidate.id,
              ])
            ).rows[0]
          : (
              await db.query(
                "SELECT count(*)::int AS count,COALESCE(sum((p.plan->>'logicalStorageAllowanceBytes')::bigint),0)::text AS bytes FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_plan p ON p.id=COALESCE(d.pending_plan_revision_id,d.plan_revision_id) WHERE d.backend_id=$1 AND d.state<>'purged'",
                [candidate.id],
              )
            ).rows[0];
        const retained = dependencies.env.DATABASE_RLS_SECRET
          ? { count: 0, bytes: "0" }
          : (
              await db.query(
                "SELECT count(*)::int AS count,COALESCE(sum(source_allowance_bytes),0)::text AS bytes FROM webdock_auth.hosting_database_migration WHERE source_backend_id=$1 AND state='retained'",
                [candidate.id],
              )
            ).rows[0];
        if (
          use.count + retained.count < candidate.max_databases &&
          BigInt(use.bytes) +
            BigInt(retained.bytes) +
            BigInt(plan.logicalStorageAllowanceBytes!) <=
            BigInt(candidate.logical_capacity_bytes)
        ) {
          backend = candidate;
          break;
        }
      }
      if (!backend)
        throw new HostingError(
          409,
          "Shared PostgreSQL capacity is unavailable.",
        );
    } else
      backend = await createDatabaseBackend(db, clusterID, "dedicated", plan);
    const order = (
      await db.query(
        "INSERT INTO webdock_auth.hosting_database_order(offer_id,customer_id,project_id,plan_revision_id,accepted_by,snapshot) VALUES($1,$2,$3,$4,$5,$6) RETURNING id",
        [
          offer.id,
          access.customerID,
          cmd.projectID,
          offer.plan_revision_id,
          actor.subject,
          offer.snapshot,
        ],
      )
    ).rows[0];
    const id = (await db.query("SELECT webdock_auth.next_snowflake() AS id"))
      .rows[0].id;
    const sealed = dependencies.encryptSecret(
      databaseCredentials(),
      `database:${id}:1`,
    );
    await db.query(
      "INSERT INTO webdock_auth.hosting_database(id,customer_id,project_id,order_id,plan_revision_id,backend_id,kind,name,database_name,credentials_encrypted) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)",
      [
        id,
        access.customerID,
        cmd.projectID,
        order.id,
        offer.plan_revision_id,
        backend.id,
        plan.kind,
        offer.name,
        "db_" + id,
        sealed,
      ],
    );
    await db.query(
      "UPDATE webdock_auth.hosting_database_order SET database_id=$2 WHERE id=$1",
      [order.id, id],
    );
    const operationID = await enqueueDatabase(
      db,
      actor,
      backend.id,
      id,
      "create",
      1,
    );
    const result = { orderID: order.id, databaseID: id, operationID };
    await dependencies.recordResult(
      db,
      actor,
      cmd.idempotencyKey,
      prior.hash,
      cmd.action,
      result,
      { customerID: access.customerID, projectID: cmd.projectID, clusterID },
    );
    await dependencies.audit(db, actor, cmd.action, id);
    return result;
  }
  async function acceptUpgrade(
    db: Connection,
    actor: HostingActor,
    cmd: Extract<
      DatabaseCommand,
      {
        action: "databases.acceptOrder";
      }
    >,
    offer: OfferRecord,
    plan: DatabasePlan,
    clusterID: string,
    hash: string,
  ) {
    if (cmd.confirmInterruption !== true)
      throw new HostingError(
        409,
        "Confirm the maintenance interruption before accepting this upgrade.",
      );
    const d = (
      await db.query(
        "SELECT d.*,p.plan FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_plan p ON p.id=d.plan_revision_id WHERE d.id=$1 AND d.project_id=$2 FOR UPDATE OF d",
        [offer.snapshot.sourceDatabaseID, cmd.projectID],
      )
    ).rows[0];
    if (
      !d ||
      d.revision !== offer.snapshot.sourceRevision ||
      d.state !== "ready" ||
      d.pending_order_id
    )
      throw new HostingError(
        409,
        "Database changed. Request a new upgrade offer.",
      );
    if (
      (
        await db.query(
          "SELECT 1 FROM webdock_auth.hosting_database_order WHERE id=$1 AND cancellation_requested_at IS NOT NULL",
          [d.order_id],
        )
      ).rowCount
    )
      throw new HostingError(409, "A cancelled database cannot be upgraded.");
    const backend = (
      await db.query(
        "SELECT * FROM webdock_auth.hosting_database_backend WHERE id=$1 FOR UPDATE",
        [d.backend_id],
      )
    ).rows[0];
    dependencies.assertDatabaseMaintenanceAllowed(d.id, "upgrade");
    const old = databasePlanSchema.parse(d.plan);
    if (plan.kind !== d.kind || plan.volumeBytes !== old.volumeBytes)
      return acceptMigration(
        db,
        actor,
        cmd,
        offer,
        plan,
        clusterID,
        hash,
        d,
        backend,
        old,
      );
    if (plan.kind === "shared") {
      if (
        plan.logicalStorageAllowanceBytes! < old.logicalStorageAllowanceBytes!
      )
        throw new HostingError(409, "Storage allowance cannot shrink.");
      const used = dependencies.env.DATABASE_RLS_SECRET
        ? (
            await db.query("SELECT * FROM webdock_rls.capacity_slots($1,$2)", [
              backend.id,
              d.id,
            ])
          ).rows[0].bytes
        : (
            await db.query(
              "SELECT COALESCE(sum((p.plan->>'logicalStorageAllowanceBytes')::bigint),0)::text AS bytes FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_plan p ON p.id=COALESCE(d.pending_plan_revision_id,d.plan_revision_id) WHERE d.backend_id=$1 AND d.id<>$2 AND d.state<>'purged'",
              [backend.id, d.id],
            )
          ).rows[0].bytes;
      if (
        BigInt(used) + BigInt(plan.logicalStorageAllowanceBytes!) >
        BigInt(backend.logical_capacity_bytes)
      )
        throw new HostingError(
          409,
          "Shared storage allowance capacity is unavailable.",
        );
    } else {
      if (
        plan.cpuMillicores! < old.cpuMillicores! ||
        plan.memoryBytes! < old.memoryBytes!
      )
        throw new HostingError(409, "Instance resources cannot shrink.");
      const delta = {
        cpuMillicores: plan.cpuMillicores! - old.cpuMillicores!,
        memoryBytes: plan.memoryBytes! - old.memoryBytes!,
      };
      const c = await freshDatabaseCluster(db, clusterID),
        cap = normalizeHostingAllowances(c.capacity),
        native = await dependencies.nativeClusterDemand(db, clusterID);
      const apps = dependencies.env.DATABASE_RLS_SECRET
        ? await dependencies.clusterCapacityDemand(db, clusterID, "apps")
        : totals(
            (
              await db.query(
                "SELECT demand,status FROM webdock_auth.hosting_reservation WHERE cluster_id=$1 AND status<>'released'",
                [clusterID],
              )
            ).rows,
          );
      const pool = c.observation.capabilities.databasePools?.find(
        (p) =>
          p.node === backend.node && p.poolID === backend.pool_id && p.ready,
      );
      const node = c.observation.nodes.find((n) => n.id === backend.node);
      if (!pool || !node)
        throw new HostingError(
          409,
          "Database placement requires a fresh qualified storage node.",
        );
      const aggregatePool = dependencies.env.DATABASE_RLS_SECRET
        ? (await dependencies.databasePoolCapacity(db, clusterID)).find(
            (r) => r.node === backend.node && r.pool_id === backend.pool_id,
          )
        : null;
      if (dependencies.env.DATABASE_RLS_SECRET && !aggregatePool)
        throw new HostingError(
          409,
          "Database placement capacity is unavailable.",
        );
      const held = dependencies.env.DATABASE_RLS_SECRET
        ? []
        : (
            await db.query(
              "SELECT id,demand FROM webdock_auth.hosting_database_backend WHERE cluster_id=$1 AND node=$2 AND state<>'purged'",
              [clusterID, backend.node],
            )
          ).rows;
      for (const key of ["cpuMillicores", "memoryBytes"] as const) {
        if (
          cap[key] === null ||
          apps[key] + native[key] + delta[key] > cap[key]!
        )
          throw new HostingError(
            409,
            "Cluster capacity is insufficient for this upgrade.",
          );
        const requested =
          key === "cpuMillicores"
            ? pool.requestedCpuMillicores
            : pool.requestedMemoryBytes;
        const known = new Set(held.map((r) => r.id));
        const observed = aggregatePool
          ? key === "cpuMillicores"
            ? aggregatePool.known_cpu
            : aggregatePool.known_memory
          : (pool.observedDatabases ?? [])
              .filter((r) => known.has(r.installationID))
              .reduce((sum, r) => sum + r[key], 0);
        const reserved = aggregatePool
          ? key === "cpuMillicores"
            ? aggregatePool.held_cpu
            : aggregatePool.held_memory
          : held.reduce((sum, r) => sum + r.demand[key], 0);
        if (
          Math.max(0, requested - observed) +
            reserved +
            apps[key] +
            delta[key] >
          Math.floor(node[key] * 0.8)
        )
          throw new HostingError(
            409,
            "Database node capacity is insufficient for this upgrade.",
          );
      }
      const demand = {
        ...backend.demand,
        cpuMillicores: backend.demand.cpuMillicores + delta.cpuMillicores,
        memoryBytes: backend.demand.memoryBytes + delta.memoryBytes,
      };
      await db.query(
        "UPDATE webdock_auth.hosting_database_backend SET demand=$2,profile=$3,revision=revision+1,state='updating' WHERE id=$1",
        [
          backend.id,
          JSON.stringify(demand),
          JSON.stringify({
            cpuMillicores: plan.cpuMillicores,
            memoryBytes: plan.memoryBytes,
            volumeBytes: plan.volumeBytes,
          }),
        ],
      );
    }
    const order = (
      await db.query(
        "INSERT INTO webdock_auth.hosting_database_order(offer_id,customer_id,project_id,plan_revision_id,accepted_by,snapshot,database_id) VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING id",
        [
          offer.id,
          d.customer_id,
          d.project_id,
          offer.plan_revision_id,
          actor.subject,
          offer.snapshot,
          d.id,
        ],
      )
    ).rows[0];
    await db.query(
      "UPDATE webdock_auth.hosting_database SET pending_plan_revision_id=$2,pending_order_id=$3,revision=revision+1,state='updating' WHERE id=$1",
      [d.id, offer.plan_revision_id, order.id],
    );
    const operationID = await enqueueDatabase(
      db,
      actor,
      backend.id,
      d.id,
      "resize",
      d.revision + 1,
    );
    const result = { orderID: order.id, databaseID: d.id, operationID };
    await dependencies.recordResult(
      db,
      actor,
      cmd.idempotencyKey,
      hash,
      cmd.action,
      result,
      {
        customerID: d.customer_id,
        projectID: d.project_id,
        clusterID,
      },
    );
    await dependencies.audit(db, actor, "databases.resize", d.id);
    return result;
  }
  async function acceptMigration(
    db: Connection,
    actor: HostingActor,
    cmd: Extract<
      DatabaseCommand,
      {
        action: "databases.acceptOrder";
      }
    >,
    offer: OfferRecord,
    plan: DatabasePlan,
    clusterID: string,
    hash: string,
    d: SourceRecord,
    source: BackendRecord,
    old: DatabasePlan,
  ) {
    if (
      plan.kind !== "dedicated" ||
      (old.kind === "dedicated" &&
        (plan.volumeBytes! <= old.volumeBytes! ||
          plan.memoryBytes! < old.memoryBytes! ||
          plan.cpuMillicores! < old.cpuMillicores!))
    )
      throw new HostingError(
        409,
        "Only Shared to Dedicated or a larger dedicated filesystem is supported.",
      );
    if (
      !source.cnpg_uid ||
      !d.usage ||
      Date.now() - Date.parse(d.usage.observedAt) > 90000 ||
      d.usage.logicalBytes > plan.volumeBytes! * 0.7
    )
      throw new HostingError(
        409,
        "A fresh source-size observation and sufficient migration headroom are required.",
      );
    const target = await createDatabaseBackend(
      db,
      clusterID,
      "dedicated",
      plan,
    );
    const order = (
      await db.query(
        "INSERT INTO webdock_auth.hosting_database_order(offer_id,customer_id,project_id,plan_revision_id,accepted_by,snapshot,database_id) VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING id",
        [
          offer.id,
          d.customer_id,
          d.project_id,
          offer.plan_revision_id,
          actor.subject,
          offer.snapshot,
          d.id,
        ],
      )
    ).rows[0];
    const id = (await db.query("SELECT webdock_auth.next_snowflake() AS id"))
      .rows[0].id;
    await db.query(
      `INSERT INTO webdock_auth.hosting_database_migration(id,database_id,source_backend_id,target_backend_id,target_order_id,source_order_id,source_plan_revision_id,source_credential_revision,source_credentials_encrypted,transfer_credentials_encrypted,source_allowance_bytes)
 VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)`,
      [
        id,
        d.id,
        source.id,
        target.id,
        order.id,
        d.order_id,
        d.plan_revision_id,
        d.credential_revision,
        d.credentials_encrypted,
        dependencies.encryptSecret(
          { password: randomBytes(32).toString("base64url") },
          "database-migration:" + id,
        ),
        old.logicalStorageAllowanceBytes ?? 0,
      ],
    );
    const credentialRevision = d.credential_revision + 1;
    await db.query(
      "UPDATE webdock_auth.hosting_database SET pending_plan_revision_id=$2,pending_order_id=$3,pending_backend_id=$4,credential_revision=$5,credentials_encrypted=$6,revision=revision+1,state='updating' WHERE id=$1",
      [
        d.id,
        offer.plan_revision_id,
        order.id,
        target.id,
        credentialRevision,
        dependencies.encryptSecret(
          databaseCredentials(),
          `database:${d.id}:${credentialRevision}`,
        ),
      ],
    );
    const operationID = await enqueueDatabase(
      db,
      actor,
      target.id,
      d.id,
      "prepareMigration",
      d.revision + 1,
    );
    await db.query(
      "UPDATE webdock_auth.hosting_database_job SET source_backend_id=$2 WHERE operation_id=$1",
      [operationID, source.id],
    );
    const result = { orderID: order.id, databaseID: d.id, operationID };
    await dependencies.recordResult(
      db,
      actor,
      cmd.idempotencyKey,
      hash,
      cmd.action,
      result,
      {
        customerID: d.customer_id,
        projectID: d.project_id,
        clusterID,
      },
    );
    await dependencies.audit(db, actor, "databases.migrate", d.id);
    return result;
  }
  return {
    freshDatabaseCluster,
    databaseCredentials,
    enqueueDatabase,
    createDatabaseBackend,
    acceptDatabaseOrder,
  };
}
