import type {
  DatabaseCommand as ManagedCommand,
  DatabaseView,
} from "@webdock/hosting-contracts";
import type {
  ManagedDatabaseOffer,
  ManagedDatabasePlanEntry,
} from "@webdock/database-contracts/runtime-ports";
import type { DatabaseRuntimePorts } from "./ports";
import {
  resourceID,
  databaseCommandSchema,
  databasePlanSchema,
  databasePlanAvailableTo,
  databaseViewSchema,
  HostingError,
  type HostingActor,
} from "@webdock/hosting-contracts";
import type { Connection } from "@webdock/hosting-contracts/runtime-ports";
export type Dependencies = {
  withDatabaseScope: DatabaseRuntimePorts["withDatabaseScope"];
  assertDatabaseMaintenanceAllowed: ReturnType<
    typeof import("./database-protection").createHostingDatabaseProtection
  >["assertDatabaseMaintenanceAllowed"];
  migrationMaintenance: ReturnType<
    typeof import("./database-migration-maintenance").createHostingDatabaseMigrationMaintenance
  >["migrationMaintenance"];
  databaseMaintenance: ReturnType<
    typeof import("./database-maintenance").createHostingDatabaseMaintenance
  >["databaseMaintenance"];
  acceptDatabaseOrder: ReturnType<
    typeof import("./database-orders").createHostingDatabaseOrders
  >["acceptDatabaseOrder"];
  createDatabaseBackend: ReturnType<
    typeof import("./database-orders").createHostingDatabaseOrders
  >["createDatabaseBackend"];
  enqueueDatabase: ReturnType<
    typeof import("./database-orders").createHostingDatabaseOrders
  >["enqueueDatabase"];
  freshDatabaseCluster: ReturnType<
    typeof import("./database-orders").createHostingDatabaseOrders
  >["freshDatabaseCluster"];
  authorizeHosting: DatabaseRuntimePorts["authorizeHosting"];
  transaction: DatabaseRuntimePorts["transaction"];
  audit: DatabaseRuntimePorts["audit"];
  clusterLock: DatabaseRuntimePorts["clusterLock"];
  replay: DatabaseRuntimePorts["replay"];
  recordResult: DatabaseRuntimePorts["recordResult"];
  env: DatabaseRuntimePorts["env"];
};

export function createHostingDatabases(dependencies: Dependencies) {
  async function databaseView(db: Connection, id: string) {
    const row = (
      await db.query(
        `SELECT d.*,p.plan,b.host,b.direct_port,b.pooled_port,b.endpoint_ready,o.cancellation_requested_at
 FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_plan p ON p.id=d.plan_revision_id
 JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id JOIN webdock_auth.hosting_database_order o ON o.id=d.order_id WHERE d.id=$1`,
        [id],
      )
    ).rows[0];
    if (!row) throw new HostingError(404, "Database is unavailable.");
    const p = databasePlanSchema.parse(row.plan);
    return databaseViewSchema.parse({
      id: row.id,
      projectID: row.project_id,
      name: row.name,
      revision: row.revision,
      observedRevision: row.observed_revision,
      state: row.state,
      planRevisionID: row.plan_revision_id,
      kind: row.kind,
      postgresMajor: p.postgresMajor,
      cpuMillicores: p.cpuMillicores,
      memoryBytes: p.memoryBytes,
      volumeBytes: p.volumeBytes,
      logicalStorageAllowanceBytes: p.logicalStorageAllowanceBytes,
      storageEnforcement:
        row.kind === "shared" ? "measured-logical" : "bounded-filesystem",
      databaseName: row.database_name,
      endpoint:
        row.endpoint_ready ||
        (row.state === "paused" && row.confirmed_credential_revision > 0)
          ? {
              host: row.host,
              directPort: row.direct_port,
              pooledPort: row.pooled_port,
            }
          : null,
      backup: {
        status: "not-configured",
        observedControls: null,
        publicRecoveryAvailable: false,
        disabledReason: "production-qualification-required",
        rpoSeconds: null,
        rtoSeconds: null,
        lastSuccessAt: null,
        recoverableUntil: null,
        lastRestoreTestAt: null,
      },
      operationID: row.operation_id,
      observedAt: row.observed_at?.toISOString() ?? null,
      usage: row.usage,
      cancellationRequestedAt:
        row.cancellation_requested_at?.toISOString() ?? null,
    });
  }
  async function databaseAccess(
    db: Connection,
    actor: HostingActor,
    id: string,
    write = false,
    operator = false,
  ) {
    const row = (
      await db.query(
        "SELECT * FROM webdock_auth.hosting_database WHERE id=$1",
        [id],
      )
    ).rows[0];
    if (!row) throw new HostingError(404, "Database is unavailable.");
    await dependencies.authorizeHosting(
      actor,
      {
        permission: "org:databases:manage",
        projectID: row.project_id,
        write,
        tenantAdmin: true,
        operator,
      },
      db,
    );
    return row;
  }
  function executeDatabaseCommand(
    actor: HostingActor,
    command: Extract<ManagedCommand, { action: "databases.offers.create" }>,
  ): Promise<ManagedDatabaseOffer>;
  function executeDatabaseCommand(
    actor: HostingActor,
    command: Extract<ManagedCommand, { action: "databases.list" }>,
  ): Promise<DatabaseView[]>;
  function executeDatabaseCommand(
    actor: HostingActor,
    command: Extract<ManagedCommand, { action: "databases.plans.list" }>,
  ): Promise<ManagedDatabasePlanEntry[]>;
  function executeDatabaseCommand(
    actor: HostingActor,
    command: Extract<ManagedCommand, { action: "databases.get" }>,
  ): Promise<DatabaseView>;
  function executeDatabaseCommand(
    actor: HostingActor,
    command: Extract<ManagedCommand, { action: "databases.acceptOrder" }>,
  ): Promise<{ orderID: string; databaseID: string; operationID: string }>;
  function executeDatabaseCommand(
    actor: HostingActor,
    command: unknown,
  ): Promise<unknown>;
  async function executeDatabaseCommand(actor: HostingActor, input: unknown) {
    const cmd = databaseCommandSchema.parse(input);
    if (actor.source === "git-policy")
      throw new HostingError(
        403,
        "Database orders require a customer administrator.",
      );
    if (
      dependencies.env.DATABASE_RLS_SECRET &&
      cmd.action === "databases.acceptOrder"
    ) {
      // Admission derives both identifiers from authorized server data. The worker
      // transaction below revalidates the actor and immutable offer before writes.
      const admission = await dependencies.transaction(async (db) => {
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
        if (!access.customerID)
          throw new HostingError(403, "Choose an authorized hosting customer.");
        const offer = (
          await db.query(
            "SELECT snapshot FROM webdock_auth.hosting_database_offer WHERE id=$1 AND project_id=$2 AND customer_id=$3",
            [cmd.offerID, cmd.projectID, access.customerID],
          )
        ).rows[0];
        if (!offer)
          throw new HostingError(404, "Database offer is unavailable.");
        const source = offer.snapshot.sourceDatabaseID
          ? (
              await db.query(
                "SELECT b.cluster_id FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id WHERE d.id=$1 AND d.project_id=$2 AND d.customer_id=$3",
                [
                  offer.snapshot.sourceDatabaseID,
                  cmd.projectID,
                  access.customerID,
                ],
              )
            ).rows[0]
          : null;
        if (offer.snapshot.sourceDatabaseID && !source)
          throw new HostingError(404, "Database source is unavailable.");
        return {
          customerID: access.customerID,
          clusterID:
            source?.cluster_id ??
            resourceID.parse(dependencies.env.WEBDOCK_DATABASE_CLUSTER_ID),
        };
      });
      return dependencies.withDatabaseScope(
        {
          kind: "system",
          purpose: "database-order",
          ...admission,
          subject: actor.subject,
          sessionID: actor.sessionID,
        },
        () =>
          dependencies.transaction((db) =>
            dependencies.acceptDatabaseOrder(db, actor, cmd),
          ),
      );
    }
    return dependencies.transaction(async (db) => {
      if (
        [
          "databases.migrations.list",
          "databases.migrationPurgePreview",
          "databases.purgeMigrationSource",
        ].includes(cmd.action)
      )
        return dependencies.migrationMaintenance(db, actor, cmd);
      if (cmd.action === "databases.projects.list") {
        await dependencies.authorizeHosting(
          actor,
          { permission: "org:databases:manage", customerID: cmd.customerID },
          db,
        );
        return (
          await db.query(
            "SELECT p.id,p.name,(SELECT count(*)::int FROM webdock_auth.hosting_database d WHERE d.customer_id=$1 AND d.project_id=p.id AND d.state<>'purged') AS count FROM webdock_admin.projects p WHERE p.customer_id=$1 AND p.status='active' ORDER BY p.name,p.id LIMIT 200",
            [cmd.customerID],
          )
        ).rows;
      }
      if (cmd.action === "databases.permissions") {
        const a = await dependencies.authorizeHosting(
          actor,
          { permission: "org:databases:manage", projectID: cmd.projectID },
          db,
        );
        return { canWrite: a.canWrite, operator: a.operator };
      }
      if (cmd.action === "databases.list") {
        await dependencies.authorizeHosting(
          actor,
          { permission: "org:databases:manage", projectID: cmd.projectID },
          db,
        );
        const rows = (
          await db.query(
            "SELECT id FROM webdock_auth.hosting_database WHERE project_id=$1 ORDER BY created_at,id",
            [cmd.projectID],
          )
        ).rows;
        return Promise.all(rows.map((r) => databaseView(db, r.id)));
      }
      if (cmd.action === "databases.get") {
        await databaseAccess(db, actor, cmd.databaseID);
        return databaseView(db, cmd.databaseID);
      }
      if (cmd.action === "databases.plans.list") {
        const access = await dependencies.authorizeHosting(
          actor,
          cmd.projectID ? { projectID: cmd.projectID } : { operator: true },
          db,
        );
        return (
          await db.query(
            "SELECT id,plan,availability,revision FROM webdock_auth.hosting_database_plan WHERE $1 OR (availability='public' AND NOT (plan ? 'sponsoredCustomerID')) ORDER BY created_at,id",
            [access.operator],
          )
        ).rows;
      }
      if (
        cmd.action === "databases.plans.create" ||
        cmd.action === "databases.plans.setAvailability"
      ) {
        await dependencies.authorizeHosting(
          actor,
          { permission: "org:databases:manage", operator: true, write: true },
          db,
        );
        const prior = await dependencies.replay(
          db,
          actor,
          cmd.idempotencyKey,
          cmd,
        );
        if (prior.existing) return prior.existing.result;
        let result;
        if (cmd.action === "databases.plans.create")
          result = (
            await db.query(
              "INSERT INTO webdock_auth.hosting_database_plan(plan) VALUES($1) RETURNING id,plan,availability,revision",
              [JSON.stringify(cmd.plan)],
            )
          ).rows[0];
        else {
          if (
            cmd.availability === "public" &&
            dependencies.env.WEBDOCK_DATABASE_PUBLIC_ENABLED !== "true"
          )
            throw new HostingError(
              409,
              "Managed PostgreSQL is not approved for public ordering.",
            );
          result = (
            await db.query(
              "UPDATE webdock_auth.hosting_database_plan SET availability=$2,revision=revision+1 WHERE id=$1 AND revision=$3 AND NOT ($2='public' AND plan ? 'sponsoredCustomerID') RETURNING id,plan,availability,revision",
              [cmd.planRevisionID, cmd.availability, cmd.revision],
            )
          ).rows[0];
          if (!result)
            throw new HostingError(
              409,
              "Database plan changed. Refresh and try again.",
            );
        }
        await dependencies.recordResult(
          db,
          actor,
          cmd.idempotencyKey,
          prior.hash,
          cmd.action,
          result,
        );
        await dependencies.audit(db, actor, cmd.action, result.id);
        return result;
      }
      if (cmd.action === "databases.offers.create") {
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
        const prior = await dependencies.replay(
          db,
          actor,
          cmd.idempotencyKey,
          cmd,
        );
        if (prior.existing) return prior.existing.result;
        const plan = (
          await db.query(
            "SELECT * FROM webdock_auth.hosting_database_plan WHERE id=$1 FOR SHARE",
            [cmd.planRevisionID],
          )
        ).rows[0];
        if (
          !plan ||
          plan.availability === "disabled" ||
          (plan.availability === "pilot" && !access.operator)
        )
          throw new HostingError(409, "This database plan is not available.");
        if (
          plan.availability === "public" &&
          dependencies.env.WEBDOCK_DATABASE_PUBLIC_ENABLED !== "true"
        )
          throw new HostingError(
            409,
            "Managed PostgreSQL ordering is not enabled.",
          );
        const parsedPlan = databasePlanSchema.parse(plan.plan);
        if (
          !databasePlanAvailableTo(
            parsedPlan,
            resourceID.parse(access.customerID),
            plan.availability,
          )
        )
          throw new HostingError(
            403,
            "This database sponsorship belongs to another customer or is not private.",
          );
        let source;
        if (cmd.databaseID) {
          source = await databaseAccess(db, actor, cmd.databaseID, true);
          dependencies.assertDatabaseMaintenanceAllowed(source.id, "upgrade");
          if (
            source.project_id !== cmd.projectID ||
            source.state !== "ready" ||
            source.pending_order_id
          )
            throw new HostingError(
              409,
              "Only a ready database can accept an upgrade offer.",
            );
        }
        const snapshot = {
          ...parsedPlan,
          backupStatus: "not-configured",
          billing: parsedPlan.sponsoredCustomerID
            ? "sponsored"
            : "monthly-net-fixed",
          billingStarts: "observed-ready",
          ...(source
            ? {
                sourceDatabaseID: source.id,
                sourceRevision: source.revision,
                requiresInterruption: true,
              }
            : {}),
        };
        const result = (
          await db.query(
            'INSERT INTO webdock_auth.hosting_database_offer(customer_id,project_id,plan_revision_id,name,snapshot) VALUES($1,$2,$3,$4,$5) RETURNING id,name,snapshot,expires_at AS "expiresAt"',
            [
              access.customerID,
              cmd.projectID,
              cmd.planRevisionID,
              cmd.name,
              JSON.stringify(snapshot),
            ],
          )
        ).rows[0];
        const output = { ...result, expiresAt: result.expiresAt.toISOString() };
        await dependencies.recordResult(
          db,
          actor,
          cmd.idempotencyKey,
          prior.hash,
          cmd.action,
          output,
          { customerID: access.customerID, projectID: cmd.projectID },
        );
        return output;
      }
      if (cmd.action === "databases.acceptOrder")
        return dependencies.acceptDatabaseOrder(db, actor, cmd);
      if (cmd.action === "databases.create") {
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
        const prior = await dependencies.replay(
          db,
          actor,
          cmd.idempotencyKey,
          cmd,
        );
        if (prior.existing) return prior.existing.result;
        const row = (
          await db.query(
            `SELECT d.id,d.name FROM webdock_auth.hosting_database_order o JOIN webdock_auth.hosting_database d ON d.id=o.database_id WHERE o.id=$1 AND o.project_id=$2 AND o.plan_revision_id=$3 AND o.snapshot->>'kind'=$4`,
            [cmd.orderID, cmd.projectID, cmd.planRevisionID, cmd.kind],
          )
        ).rows[0];
        if (!row || row.name !== cmd.name)
          throw new HostingError(
            409,
            "An accepted matching database order is required.",
          );
        const result = await databaseView(db, row.id);
        await dependencies.recordResult(
          db,
          actor,
          cmd.idempotencyKey,
          prior.hash,
          cmd.action,
          result,
          { customerID: access.customerID, projectID: cmd.projectID },
        );
        return result;
      }
      if (cmd.action === "databases.resize") {
        const d = await databaseAccess(db, actor, cmd.databaseID, true);
        const prior = await dependencies.replay(
          db,
          actor,
          cmd.idempotencyKey,
          cmd,
        );
        if (prior.existing) return prior.existing.result;
        const order = (
          await db.query(
            "SELECT snapshot FROM webdock_auth.hosting_database_order WHERE id=$1 AND database_id=$2 AND plan_revision_id=$3",
            [cmd.targetOrderID, d.id, cmd.targetPlanRevisionID],
          )
        ).rows[0];
        if (
          !order ||
          order.snapshot.sourceRevision !== cmd.revision ||
          ![d.order_id, d.pending_order_id].includes(cmd.targetOrderID)
        )
          throw new HostingError(
            409,
            "An accepted matching database upgrade order is required.",
          );
        const result = await databaseView(db, d.id);
        await dependencies.recordResult(
          db,
          actor,
          cmd.idempotencyKey,
          prior.hash,
          cmd.action,
          result,
          { customerID: d.customer_id, projectID: d.project_id },
        );
        return result;
      }
      if (
        cmd.action === "databases.backup" ||
        cmd.action === "databases.restore" ||
        cmd.action === "databases.clone"
      ) {
        await databaseAccess(
          db,
          actor,
          "databaseID" in cmd ? cmd.databaseID : cmd.sourceDatabaseID,
          true,
        );
        throw new HostingError(409, "Backups and recovery are not configured.");
      }
      if (cmd.action === "databases.pools.register") {
        await dependencies.authorizeHosting(
          actor,
          { permission: "org:databases:manage", operator: true, write: true },
          db,
        );
        const prior = await dependencies.replay(
          db,
          actor,
          cmd.idempotencyKey,
          cmd,
        );
        if (prior.existing) return prior.existing.result;
        await dependencies.clusterLock(db, cmd.clusterID);
        const c = await dependencies.freshDatabaseCluster(db, cmd.clusterID);
        const pool = c.observation.capabilities.databasePools?.find(
          (p) => p.node === cmd.node && p.poolID === cmd.poolID && p.ready,
        );
        if (c.revision !== cmd.revision || !pool)
          throw new HostingError(
            409,
            "Refresh the qualified database pool observation.",
          );
        const existing = (
          await db.query(
            "SELECT * FROM webdock_auth.hosting_database_pool WHERE cluster_id=$1 AND pool_id=$2",
            [cmd.clusterID, cmd.poolID],
          )
        ).rows[0];
        if (
          existing &&
          (existing.node !== cmd.node ||
            Number(existing.capacity_bytes) !== pool.capacityBytes)
        )
          throw new HostingError(
            409,
            "Changing a registered storage partition requires a separate capacity review.",
          );
        const others = (
          await db.query(
            "SELECT COALESCE(sum(capacity_bytes),0)::text AS bytes FROM webdock_auth.hosting_database_pool WHERE cluster_id=$1 AND pool_id<>$2",
            [cmd.clusterID, cmd.poolID],
          )
        ).rows[0].bytes;
        const appBytes = c.observation.capacity?.volumeBytes;
        if (
          typeof appBytes !== "number" ||
          !Number.isSafeInteger(appBytes) ||
          !Number.isSafeInteger(c.capacity?.volumeBytes) ||
          BigInt(appBytes) + BigInt(others) + BigInt(pool.capacityBytes) >
            BigInt(c.capacity.volumeBytes!)
        )
          throw new HostingError(
            409,
            "Carve the PostgreSQL pool out of the existing shared storage budget first.",
          );
        await db.query(
          "INSERT INTO webdock_auth.hosting_database_pool(cluster_id,node,pool_id,capacity_bytes) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING",
          [cmd.clusterID, cmd.node, cmd.poolID, pool.capacityBytes],
        );
        const result = {
          poolID: cmd.poolID,
          node: cmd.node,
          capacityBytes: pool.capacityBytes,
        };
        await dependencies.recordResult(
          db,
          actor,
          cmd.idempotencyKey,
          prior.hash,
          cmd.action,
          result,
          { clusterID: cmd.clusterID },
        );
        await dependencies.audit(db, actor, cmd.action, cmd.clusterID);
        return result;
      }
      if (cmd.action === "databases.installations.list") {
        await dependencies.authorizeHosting(
          actor,
          { permission: "org:databases:manage", operator: true },
          db,
        );
        return (
          await db.query(
            'SELECT id,cluster_id AS "clusterID",kind,node,pool_id AS "poolID",revision,observed_revision AS "observedRevision",state,cnpg_uid AS "cnpgUID",demand,max_databases AS "maxDatabases",logical_capacity_bytes::text AS "logicalCapacityBytes",host,direct_port AS "directPort",pooled_port AS "pooledPort",observed_at AS "observedAt" FROM webdock_auth.hosting_database_backend WHERE cluster_id=$1 ORDER BY id',
            [cmd.clusterID],
          )
        ).rows;
      }
      if (cmd.action === "databases.installations.create") {
        await dependencies.authorizeHosting(
          actor,
          { permission: "org:databases:manage", operator: true, write: true },
          db,
        );
        const prior = await dependencies.replay(
          db,
          actor,
          cmd.idempotencyKey,
          cmd,
        );
        if (prior.existing) return prior.existing.result;
        const backend = await dependencies.createDatabaseBackend(
          db,
          cmd.clusterID,
          "shared-pool",
          undefined,
          cmd,
        );
        const operationID = await dependencies.enqueueDatabase(
          db,
          actor,
          backend.id,
          null,
          "create",
          1,
        );
        const result = {
          installationID: backend.id,
          operationID,
          state: "provisioning",
        };
        await dependencies.recordResult(
          db,
          actor,
          cmd.idempotencyKey,
          prior.hash,
          cmd.action,
          result,
          { clusterID: cmd.clusterID },
        );
        await dependencies.audit(db, actor, cmd.action, backend.id);
        return result;
      }
      return dependencies.databaseMaintenance(db, actor, cmd);
    });
  }
  return { databaseView, databaseAccess, executeDatabaseCommand };
}
