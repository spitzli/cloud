import type { DatabaseServices } from "@spitzli/database-contracts/runtime-ports";
import type { HostingPorts } from "./ports";
import { msgid } from "@spitzli/i18n";
import { searchPage } from "@spitzli/search";
import type { VercelRuntime } from "./byok-vercel";
import {
  commandSchema,
  normalizeHostingAllowances,
  hostingDimensions,
  HostingError,
  readActions,
  type HostingActor,
  type ClusterObservation,
} from "@spitzli/hosting-contracts";
import { limits, usage } from "./allowances";
export type Dependencies = {
  readProjectOverview: ReturnType<
    typeof import("./project-overview").createHostingProjectOverview
  >["readProjectOverview"];
  readHostingView: ReturnType<
    typeof import("./view-cache").createHostingViewCache
  >["readHostingView"];
  executeProjectSetup: ReturnType<
    typeof import("./project-setup").createHostingProjectSetup
  >["executeProjectSetup"];
  executeServiceBinding: ReturnType<
    typeof import("./service-bindings").createHostingServiceBindings
  >["executeServiceBinding"];
  relocateProject: ReturnType<
    typeof import("./relocation").createHostingRelocation
  >["relocateProject"];
  withHostingDatabase: HostingPorts["withHostingDatabase"];
  executeDatabaseCommand: DatabaseServices["executeDatabaseCommand"];
  executeGitDeployment: ReturnType<
    typeof import("./git-deployments").createHostingGitDeployments
  >["executeGitDeployment"];
  executeByok: ReturnType<
    typeof import("./byok").createHostingByok
  >["executeByok"];
  byokCluster: ReturnType<
    typeof import("./byok").createHostingByok
  >["byokCluster"];
  executeAppCommand: ReturnType<
    typeof import("./apps").createHostingApps
  >["executeAppCommand"];
  transaction: ReturnType<typeof import("./db").createHostingDb>["transaction"];
  audit: ReturnType<typeof import("./db").createHostingDb>["audit"];
  planLock: ReturnType<typeof import("./db").createHostingDb>["planLock"];
  clusterLock: ReturnType<typeof import("./db").createHostingDb>["clusterLock"];
  authorizeHosting: ReturnType<
    typeof import("./authorization").createHostingAuthorization
  >["authorizeHosting"];
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

export function createHostingService(dependencies: Dependencies) {
  async function executeHosting(
    actor: HostingActor,
    raw: unknown,
    runtime?: VercelRuntime,
  ) {
    return dependencies.withHostingDatabase(actor, async () => {
      const cmd = commandSchema.parse(raw);
      if (cmd.action.startsWith("setup."))
        return dependencies.executeProjectSetup(actor, cmd);
      if (cmd.action.startsWith("services."))
        return dependencies.executeServiceBinding(actor, cmd);
      if (cmd.action === "projects.relocate")
        return dependencies.relocateProject(actor, cmd);
      if (cmd.action.startsWith("databases."))
        return dependencies.executeDatabaseCommand(actor, cmd);
      if (cmd.action.startsWith("git."))
        return dependencies.executeGitDeployment(actor, cmd);
      if (cmd.action.startsWith("byok."))
        return dependencies.executeByok(actor, cmd, runtime);
      if (cmd.action.startsWith("apps."))
        return dependencies.executeAppCommand(actor, cmd);
      return dependencies.transaction(async (db) => {
        const write = !readActions.has(cmd.action);
        if (cmd.action === "projects.overview")
          return dependencies.readProjectOverview(actor, cmd.projectID, db);
        if (cmd.action === "projects.get") {
          const access = await dependencies.authorizeHosting(
            actor,
            { permission: "org:projects:manage", projectID: cmd.projectID },
            db,
          );
          const context = await dependencies.readHostingView(
            db,
            actor,
            {
              kind: "projectsummary",
              scope: {
                customerID: access.customerID!,
                projectID: cmd.projectID,
                operator: access.operator,
                liveReads: access.liveReads,
              },
              query: cmd,
              ttlSeconds: 60,
              tables: ["webdock_admin.projects", "webdock_admin.customers"],
            },
            async () =>
              (
                await db.query(
                  `SELECT p.id AS "projectID",p.name,c.id AS "customerID",c.name AS "customerName"
        FROM webdock_admin.projects p JOIN webdock_admin.customers c ON c.id=p.customer_id WHERE p.id=$1`,
                  [cmd.projectID],
                )
              ).rows[0],
          );
          return {
            ...context,
            operator: access.operator,
            configured: Boolean(access.project?.provider),
            provider: access.project?.provider ?? null,
          };
        }
        if (cmd.action === "operations.overview") {
          await dependencies.authorizeHosting(
            actor,
            { permission: "org:projects:manage", operator: true },
            db,
          );
          return dependencies.readHostingView(
            db,
            actor,
            {
              kind: "operations",
              scope: { operator: true },
              query: cmd,
              ttlSeconds: 15,
              tables: [
                "webdock_auth.hosting_app",
                "webdock_auth.hosting_operation",
                "webdock_admin.projects",
                "webdock_admin.customers",
              ],
            },
            async () => {
              const search = `%${cmd.search.replace(/[\\%_]/g, "\\$&")}%`;
              const from = `FROM webdock_auth.hosting_app a JOIN webdock_admin.projects p ON p.id=a.project_id JOIN webdock_admin.customers c ON c.id=p.customer_id
            WHERE a.status<>'deleted' AND (a.name ILIKE $1 OR p.name ILIKE $1 OR c.name ILIKE $1)`;
              const totalDocs = (
                await db.query(`SELECT count(*)::int AS n ${from}`, [search])
              ).rows[0].n;
              const docs = (
                await db.query(
                  `SELECT a.id,a.name,a.status,a.revision,a.observed_revision AS "observedRevision",p.id AS "projectID",p.name AS "projectName",c.id AS "customerID",c.name AS "customerName"
            ${from} ORDER BY a.name ${cmd.sort === "-name" ? "DESC" : "ASC"},a.id LIMIT $2 OFFSET $3`,
                  [search, cmd.limit, (cmd.page - 1) * cmd.limit],
                )
              ).rows;
              const operations = (
                await db.query(`SELECT o.id,o.action,o.status,o.created_at AS "createdAt",p.id AS "projectID",p.name AS "projectName",c.name AS "customerName"
            FROM webdock_auth.hosting_operation o LEFT JOIN webdock_admin.projects p ON p.id=o.project_id LEFT JOIN webdock_admin.customers c ON c.id=o.customer_id
            ORDER BY CASE WHEN o.status IN ('queued','running','needs-reconciliation') THEN 0 ELSE 1 END,o.created_at DESC,o.id DESC LIMIT 30`)
              ).rows;
              return {
                docs,
                operations,
                page: cmd.page,
                limit: cmd.limit,
                totalDocs,
                totalPages: Math.ceil(totalDocs / cmd.limit),
                readAt: new Date().toISOString(),
              };
            },
          );
        }
        if (cmd.action === "clusters.activate") {
          await dependencies.authorizeHosting(
            actor,
            { permission: "org:projects:manage", operator: true, write: true },
            db,
          );
          await dependencies.clusterLock(db, cmd.clusterID);
          const owned = (
            await db.query(
              "SELECT ownership FROM webdock_auth.hosting_cluster WHERE id=$1",
              [cmd.clusterID],
            )
          ).rows[0];
          if (owned?.ownership === "tenant")
            throw new HostingError(
              409,
              "Customer clusters use their existing workload management.",
            );
          const c = await dependencies.clusterView(db, cmd.clusterID);
          if (
            c.revision !== cmd.revision ||
            c.state !== "connected" ||
            !c.observation
          )
            throw new HostingError(
              409,
              "A fresh connected cluster and current revision are required.",
            );
          const capacity = normalizeHostingAllowances(cmd.capacity);
          const databasePools: NonNullable<
            ClusterObservation["capabilities"]["databasePools"]
          > =
            c.observation.capabilities.databaseVersion === 1
              ? (c.observation.capabilities.databasePools ?? [])
              : [];
          const databaseStorage = databasePools
            .filter(
              (p) =>
                p.ready &&
                c.observation.nodes.some(
                  (n: { id: string }) => n.id === p.node,
                ),
            )
            .reduce((sum, p) => sum + p.capacityBytes, 0);
          const appStorage =
            c.observation.capabilities.storage &&
            c.observation.capabilities.storageVersion === 1
              ? (c.observation.capacity.volumeBytes ?? 0)
              : 0;
          const verifiedStorage = appStorage + databaseStorage;
          if (
            hostingDimensions.some((k) => capacity[k] === null) ||
            !Number.isSafeInteger(verifiedStorage) ||
            capacity.volumeBytes! > verifiedStorage
          )
            throw new HostingError(
              400,
              "Managed capacity must be finite and within verified persistent storage capacity.",
            );
          const cpu = c.observation.nodes.reduce(
              (
                n: number,
                v: {
                  cpuMillicores: number;
                },
              ) => n + v.cpuMillicores,
              0,
            ),
            memory = c.observation.nodes.reduce(
              (
                n: number,
                v: {
                  memoryBytes: number;
                },
              ) => n + v.memoryBytes,
              0,
            );
          if (
            !capacity.apps ||
            !capacity.cpuMillicores ||
            !capacity.memoryBytes ||
            !capacity.ephemeralBytes ||
            capacity.cpuMillicores > Math.floor(cpu * 0.8) ||
            capacity.memoryBytes > Math.floor(memory * 0.8)
          )
            throw new HostingError(
              409,
              "Reserve at least twenty percent of node CPU and memory for the platform.",
            );
          await db.query(
            "UPDATE webdock_auth.hosting_cluster SET verified=true,capacity=$2,verification_evidence=$3,revision=revision+1 WHERE id=$1",
            [cmd.clusterID, JSON.stringify(capacity), cmd.verificationEvidence],
          );
          await dependencies.audit(db, actor, cmd.action, cmd.clusterID);
          return dependencies.clusterView(db, cmd.clusterID);
        }
        if (cmd.action === "clusters.list") {
          await dependencies.authorizeHosting(
            actor,
            { permission: "org:projects:manage", operator: true },
            db,
          );
          return dependencies.readHostingView(
            db,
            actor,
            {
              kind: "projectsummary",
              scope: { operator: true },
              query: cmd,
              ttlSeconds: 5,
              tables: [
                "webdock_auth.hosting_cluster",
                "webdock_auth.hosting_agent",
              ],
            },
            async () => {
              if (cmd.search.trim()) {
                const found = await searchPage(
                  async (page) => {
                    const rows = (
                      await db.query(
                        `SELECT id,name,provider,region FROM webdock_auth.hosting_cluster ORDER BY name ${cmd.sort === "-name" ? "DESC" : "ASC"},id LIMIT 300 OFFSET $1`,
                        [(page - 1) * 300],
                      )
                    ).rows;
                    return { rows, hasMore: rows.length === 300 };
                  },
                  (r) => [r.name, r.provider, r.region].join(" "),
                  cmd.search,
                  cmd.page,
                  cmd.limit,
                );
                return {
                  ...found,
                  docs: await Promise.all(
                    found.docs.map((r) => dependencies.clusterView(db, r.id)),
                  ),
                };
              }
              const filter = `name ILIKE $1`;
              const params = [`%${cmd.search.replace(/[\\%_]/g, "\\$&")}%`];
              const rows = (
                await db.query(
                  `SELECT id FROM webdock_auth.hosting_cluster WHERE ${filter} ORDER BY name ${cmd.sort === "-name" ? "DESC" : "ASC"},id LIMIT $2 OFFSET $3`,
                  [...params, cmd.limit, (cmd.page - 1) * cmd.limit],
                )
              ).rows;
              const total = (
                await db.query(
                  `SELECT count(*)::int AS n FROM webdock_auth.hosting_cluster WHERE ${filter}`,
                  params,
                )
              ).rows[0].n;
              return {
                docs: await Promise.all(
                  rows.map((r) => dependencies.clusterView(db, r.id)),
                ),
                page: cmd.page,
                totalDocs: total,
                totalPages: Math.ceil(total / cmd.limit),
                limit: cmd.limit,
              };
            },
          );
        }
        if (cmd.action === "clusters.get") {
          await dependencies.authorizeHosting(
            actor,
            { permission: "org:projects:manage", operator: true },
            db,
          );
          return dependencies.readHostingView(
            db,
            actor,
            {
              kind: "projectsummary",
              scope: { operator: true },
              query: cmd,
              ttlSeconds: 5,
              tables: [
                "webdock_auth.hosting_cluster",
                "webdock_auth.hosting_agent",
              ],
            },
            () => dependencies.clusterView(db, cmd.clusterID),
          );
        }
        if (cmd.action === "clusters.register") {
          await dependencies.authorizeHosting(
            actor,
            {
              permission: "org:projects:manage",
              operator: true,
              write,
              customerID: cmd.dedicatedCustomerID,
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
              `INSERT INTO webdock_auth.hosting_cluster(name,provider,country,region,location_evidence,dedicated_customer_id) VALUES($1,$2,$3,$4,$5,$6) RETURNING id`,
              [
                cmd.name,
                cmd.provider,
                cmd.country,
                cmd.region,
                cmd.locationEvidence,
                cmd.dedicatedCustomerID,
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
            { clusterID: row.id },
          );
          await dependencies.audit(db, actor, cmd.action, row.id);
          return result;
        }
        if (
          cmd.action === "clusters.enrollment" ||
          cmd.action === "clusters.revoke"
        ) {
          await dependencies.authorizeHosting(
            actor,
            { permission: "org:projects:manage", operator: true, write },
            db,
          );
          await dependencies.clusterLock(db, cmd.clusterID);
          const c = await dependencies.clusterView(db, cmd.clusterID);
          if (cmd.action === "clusters.revoke") {
            if (c.revision !== cmd.revision)
              throw new HostingError(
                409,
                "Cluster changed. Refresh and try again.",
              );
            await db.query(
              "UPDATE webdock_auth.hosting_cluster SET verified=false,revision=revision+1 WHERE id=$1",
              [cmd.clusterID],
            );
            await db.query(
              "UPDATE webdock_auth.hosting_agent SET revoked=true WHERE cluster_id=$1",
              [cmd.clusterID],
            );
            await db.query(
              "UPDATE webdock_auth.hosting_enrollment SET consumed_at=now() WHERE cluster_id=$1 AND consumed_at IS NULL",
              [cmd.clusterID],
            );
            await dependencies.audit(db, actor, cmd.action, cmd.clusterID);
            return dependencies.clusterView(db, cmd.clusterID);
          }
          const recent = (
            await db.query(
              "SELECT count(*)::int AS n FROM webdock_auth.hosting_enrollment WHERE cluster_id=$1 AND created_at>now()-interval '5 minutes'",
              [cmd.clusterID],
            )
          ).rows[0].n;
          if (recent >= 3)
            throw new HostingError(
              429,
              "Wait before creating another enrollment.",
            );
          const r = (
            await db.query(
              "INSERT INTO webdock_auth.hosting_enrollment(cluster_id,actor_id,expires_at) VALUES($1,$2,now()+interval '5 minutes') RETURNING id,expires_at",
              [cmd.clusterID, actor.subject],
            )
          ).rows[0];
          await dependencies.audit(db, actor, cmd.action, cmd.clusterID);
          return {
            enrollmentID: r.id,
            clusterID: cmd.clusterID,
            setupPath: `/infrastructure/${cmd.clusterID}/enroll/${r.id}`,
            expiresAt: r.expires_at.toISOString(),
          };
        }
        if (cmd.action === "limits.get" || cmd.action === "usage.get") {
          await dependencies.authorizeHosting(
            actor,
            { permission: "org:projects:manage", customerID: cmd.customerID },
            db,
          );
          return cmd.action === "limits.get"
            ? limits(db, cmd.customerID)
            : usage(db, cmd.customerID);
        }
        if (cmd.action === "limits.set") {
          await dependencies.authorizeHosting(
            actor,
            {
              permission: "org:projects:manage",
              customerID: cmd.customerID,
              projectID: cmd.projectID,
              operator: true,
              write,
            },
            db,
          );
          await dependencies.planLock(db, cmd.customerID);
          if (cmd.projectID && cmd.provider)
            throw new HostingError(
              400,
              "Choose either a project or a provider scope.",
            );
          const current = await limits(db, cmd.customerID);
          if (current.subscriptionRevision !== cmd.subscriptionRevision)
            throw new HostingError(
              409,
              "Subscription changed. Refresh and try again.",
            );
          const scope = cmd.projectID
            ? `project:${cmd.projectID}`
            : cmd.provider
              ? `provider:${cmd.provider}`
              : "customer";
          const old = current.limits.find((r) => r.scope === scope);
          if ((old?.revision ?? 0) !== cmd.revision)
            throw new HostingError(
              409,
              "Hosting limits changed. Refresh and try again.",
            );
          await db.query(
            "INSERT INTO webdock_auth.hosting_limit(customer_id,scope,values,revision) VALUES($1,$2,$3,1) ON CONFLICT(customer_id,scope) DO UPDATE SET values=$3,revision=hosting_limit.revision+1",
            [cmd.customerID, scope, JSON.stringify(cmd.values)],
          );
          await dependencies.audit(db, actor, cmd.action, cmd.customerID);
          return limits(db, cmd.customerID);
        }
        if (cmd.action === "projects.list") {
          const access = await dependencies.authorizeHosting(
            actor,
            { permission: "org:projects:manage", customerID: cmd.customerID },
            db,
          );
          const view = await dependencies.readHostingView(
            db,
            actor,
            {
              kind: "projectsummary",
              scope: {
                customerID: cmd.customerID,
                operator: access.operator,
                liveReads: access.liveReads,
              },
              query: cmd,
              ttlSeconds: 30,
              tables: [
                "webdock_auth.hosting_project",
                "webdock_admin.projects",
                "webdock_auth.hosting_cluster",
              ],
            },
            async () => {
              if (cmd.search.trim()) {
                const found = await searchPage(
                  async (page) => {
                    const rows = (
                      await db.query(
                        `SELECT h.project_id AS "projectID",p.name,h.provider,h.mode,h.own_images AS "ownImages",h.revision,h.cluster_id AS "clusterID",c.name AS "clusterName",c.country,c.region FROM webdock_auth.hosting_project h JOIN webdock_admin.projects p ON p.id=h.project_id LEFT JOIN webdock_auth.hosting_cluster c ON c.id=h.cluster_id WHERE h.customer_id=$1 ORDER BY p.name ${cmd.sort === "-name" ? "DESC" : "ASC"},p.id LIMIT 300 OFFSET $2`,
                        [cmd.customerID, (page - 1) * 300],
                      )
                    ).rows;
                    return { rows, hasMore: rows.length === 300 };
                  },
                  (r) => [r.name, r.provider, r.clusterName].join(" "),
                  cmd.search,
                  cmd.page,
                  cmd.limit,
                );
                return found;
              }
              const filter = `h.customer_id=$1 AND p.name ILIKE $2`;
              const params = [
                cmd.customerID,
                `%${cmd.search.replace(/[\\%_]/g, "\\$&")}%`,
              ];
              const total = (
                await db.query(
                  `SELECT count(*)::int AS n FROM webdock_auth.hosting_project h JOIN webdock_admin.projects p ON p.id=h.project_id WHERE ${filter}`,
                  params,
                )
              ).rows[0].n;
              const docs = (
                await db.query(
                  `SELECT h.project_id AS "projectID",p.name,h.provider,h.mode,h.own_images AS "ownImages",h.revision,h.cluster_id AS "clusterID",c.name AS "clusterName",c.country,c.region FROM webdock_auth.hosting_project h JOIN webdock_admin.projects p ON p.id=h.project_id LEFT JOIN webdock_auth.hosting_cluster c ON c.id=h.cluster_id WHERE ${filter} ORDER BY p.name ${cmd.sort === "-name" ? "DESC" : "ASC"},p.id LIMIT $3 OFFSET $4`,
                  [...params, cmd.limit, (cmd.page - 1) * cmd.limit],
                )
              ).rows;
              return {
                docs,
                page: cmd.page,
                totalDocs: total,
                totalPages: Math.ceil(total / cmd.limit),
                limit: cmd.limit,
              };
            },
          );
          return { ...view, operator: access.operator };
        }
        if (
          cmd.action === "projects.create" ||
          cmd.action === "projects.update"
        ) {
          const access = await dependencies.authorizeHosting(
            actor,
            {
              permission: "org:projects:manage",
              projectID: cmd.projectID,
              operator: true,
              write,
            },
            db,
          );
          await dependencies.planLock(db, access.customerID!);
          if (cmd.action === "projects.create") {
            const prior = await dependencies.replay(
              db,
              actor,
              cmd.idempotencyKey,
              cmd,
            );
            if (prior.existing) return prior.existing.result;
            if (
              (cmd.provider === "k3s" && !cmd.clusterID) ||
              (cmd.provider === "vercel" && cmd.clusterID)
            )
              throw new HostingError(400, "Select a cluster only for k3s.");
            if (cmd.clusterID) {
              await dependencies.clusterLock(db, cmd.clusterID);
              const c = await dependencies.clusterView(db, cmd.clusterID);
              if (
                c.dedicatedCustomerID &&
                c.dedicatedCustomerID !== access.customerID
              )
                throw new HostingError(
                  403,
                  "Cluster is assigned to another customer.",
                );
              if (
                cmd.ownImages &&
                !c.dedicatedCustomerID &&
                !cmd.confirmSharedImages
              )
                throw new HostingError(
                  409,
                  msgid(
                    "Confirm permission to run this project's images on a shared cluster.",
                  ),
                );
            }
            if (cmd.provider === "vercel" && cmd.ownImages)
              throw new HostingError(
                400,
                "Container images are not available for this provider.",
              );
            const r = await db.query(
              `INSERT INTO webdock_auth.hosting_project(project_id,customer_id,provider,cluster_id,namespace,mode,own_images) VALUES($1,$2,$3,$4,$5,$6,$7) ON CONFLICT DO NOTHING RETURNING project_id`,
              [
                cmd.projectID,
                access.customerID,
                cmd.provider,
                cmd.clusterID,
                cmd.clusterID ? `wd-${cmd.projectID}` : null,
                cmd.mode,
                cmd.ownImages,
              ],
            );
            if (!r.rowCount)
              throw new HostingError(
                409,
                "Hosting is already assigned to this project.",
              );
            const result = { projectID: cmd.projectID, revision: 1 };
            await dependencies.recordResult(
              db,
              actor,
              cmd.idempotencyKey,
              prior.hash,
              cmd.action,
              result,
              {
                projectID: cmd.projectID,
                customerID: access.customerID,
                clusterID: cmd.clusterID,
              },
            );
            await dependencies.audit(db, actor, cmd.action, cmd.projectID);
            return result;
          }
          if (cmd.ownImages) {
            const c =
              access.project.cluster_id &&
              (await dependencies.clusterView(db, access.project.cluster_id));
            if (
              !c ||
              (c.dedicatedCustomerID &&
                c.dedicatedCustomerID !== access.customerID)
            )
              throw new HostingError(
                409,
                msgid(
                  "A Kubernetes cluster available to this customer is required.",
                ),
              );
            if (!c.dedicatedCustomerID && !cmd.confirmSharedImages)
              throw new HostingError(
                409,
                msgid(
                  "Confirm permission to run this project's images on a shared cluster.",
                ),
              );
          }
          const result = await db.query(
            "UPDATE webdock_auth.hosting_project SET mode=$2,own_images=$3,revision=revision+1 WHERE project_id=$1 AND revision=$4 RETURNING revision",
            [cmd.projectID, cmd.mode, cmd.ownImages, cmd.revision],
          );
          if (!result.rowCount)
            throw new HostingError(
              409,
              "Hosting project changed. Refresh and try again.",
            );
          await dependencies.audit(db, actor, cmd.action, cmd.projectID);
          return {
            projectID: cmd.projectID,
            revision: result.rows[0].revision,
          };
        }
        if (cmd.action === "operations.get") {
          const op = (
            await db.query(
              "SELECT id,action,status,customer_id,project_id,cluster_id,generation,created_at,result,desired FROM webdock_auth.hosting_operation WHERE id=$1",
              [cmd.operationID],
            )
          ).rows[0];
          if (!op) throw new HostingError(404, "Operation is unavailable.");
          await dependencies.authorizeHosting(
            actor,
            {
              permission: "org:projects:manage",
              customerID: op.customer_id ?? undefined,
              projectID: op.project_id ?? undefined,
              operator: !op.customer_id,
            },
            db,
          );
          let reconciliation:
            | undefined
            | {
                resourceVersion: string;
              };
          if (op.action === "byok.workload") {
            const access = await dependencies.byokCluster(
              db,
              actor,
              op.cluster_id,
            );
            const snapshot = access.row.inventory;
            const resource = snapshot?.resources.find(
              (r: import("@spitzli/hosting-contracts").InventoryResource) =>
                r.uid === op.desired.resource.uid,
            );
            if (
              access.operator &&
              op.status === "needs-reconciliation" &&
              resource &&
              Date.now() - Date.parse(snapshot.observedAt) < 180000
            )
              reconciliation = { resourceVersion: resource.resourceVersion };
            if (
              !access.policy.namespaces.includes(
                op.desired.resource.namespace,
              ) ||
              (op.desired.action === "logs" && !access.liveReads)
            )
              throw new HostingError(403, "Operation is unavailable.");
          }
          return {
            id: op.id,
            ...(op.action === "byok.workload"
              ? {
                  result: op.result,
                  reconciliation,
                  clusterID: op.cluster_id,
                  resourceName: op.desired.resource.name,
                  operationType: op.desired.action,
                }
              : {}),
            action: op.action,
            status: op.status,
            customerID: op.customer_id,
            projectID: op.project_id,
            createdAt: op.created_at.toISOString(),
          };
        }
        throw new HostingError(400, "Unknown hosting action.");
      });
    });
  }
  return { executeHosting };
}
