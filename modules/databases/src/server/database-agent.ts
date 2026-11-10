import type { DatabaseRuntimePorts } from "./ports";
import {
  createHash,
  createHmac,
  timingSafeEqual,
  X509Certificate,
} from "node:crypto";
import { z } from "zod";
import {
  HostingError,
  resourceID,
  type HostingActor,
} from "@spitzli/hosting-contracts";
import type { Agent } from "@spitzli/hosting-contracts/runtime-ports";
import type { Connection } from "@spitzli/hosting-contracts/runtime-ports";
export type Dependencies = {
  operationAuthority?(
    db: Connection,
    id: string,
  ): Promise<{ required: boolean; allowed: boolean }>;
  verifyAgent: DatabaseRuntimePorts["verifyAgent"];
  transaction: DatabaseRuntimePorts["transaction"];
  clusterLock: DatabaseRuntimePorts["clusterLock"];
  authorizeHosting: DatabaseRuntimePorts["authorizeHosting"];
  withStoredIdentity?<T>(
    reference: NonNullable<HostingActor["consoleIdentity"]>,
    run: (principal: { userID: string; sessionID: string }) => Promise<T>,
  ): Promise<T>;
  decryptSecret: DatabaseRuntimePorts["decryptSecret"];
  env: DatabaseRuntimePorts["env"];
};

export function createHostingDatabaseAgent(dependencies: Dependencies) {
  const leaseSchema = z.object({
    operationID: resourceID,
    generation: z.number().int().positive(),
    leaseToken: z.string().regex(/^[a-f0-9]{64}$/),
  });
  const checkpointSchema = leaseSchema
    .extend({
      phase: z.enum(["applied", "configuring", "observing"]),
      cnpgUID: z.string().min(1).max(128),
      caCertificate: z.string().max(262144).optional(),
      previousPodUID: z.string().min(1).max(128).optional(),
      migrationJobUID: z.string().min(1).max(128).optional(),
      copySHA256: z
        .string()
        .regex(/^[a-f0-9]{64}$/)
        .optional(),
    })
    .strict();
  const proofSchema = z
    .object({
      installationID: resourceID,
      databaseID: resourceID.nullable(),
      cnpgUID: z.string().min(1).max(128),
      revision: z.number().int().positive(),
      state: z.enum(["ready", "paused", "retained", "purged"]),
      endpointReady: z.boolean(),
      copySHA256: z
        .string()
        .regex(/^[a-f0-9]{64}$/)
        .optional(),
    })
    .strict();
  const completionSchema = leaseSchema
    .extend({
      outcome: z.enum(["succeeded", "needs-reconciliation"]),
      proof: proofSchema.optional(),
    })
    .strict();
  function conflict(): never {
    throw new HostingError(
      409,
      "Database operation requires reconciliation or belongs to another agent.",
    );
  }
  function token(id: string, generation: number) {
    const secret = dependencies.env.BETTER_AUTH_SECRET;
    if (!secret || secret.length < 32)
      throw new HostingError(503, "Database lease signing is not configured.");
    return createHmac("sha256", secret)
      .update(`database-lease:${id}:${generation}`)
      .digest("hex");
  }
  const digest = (value: string) =>
    createHash("sha256").update(value).digest("hex");
  async function verify(db: Connection, agent: Agent) {
    const a = await dependencies.verifyAgent(db, agent);
    const c = (
      await db.query(
        "SELECT verified,ownership FROM webdock_auth.hosting_cluster WHERE id=$1",
        [agent.clusterID],
      )
    ).rows[0];
    if (
      !c?.verified ||
      c.ownership !== "platform" ||
      a.observation?.capabilities?.databaseVersion !== 1 ||
      !a.last_seen ||
      Date.now() - new Date(a.last_seen).getTime() >= 90000
    )
      conflict();
    return a;
  }
  async function job(
    db: Connection,
    agent: Agent,
    input: z.infer<typeof leaseSchema>,
    completed = false,
  ) {
    await verify(db, agent);
    await dependencies.clusterLock(db, agent.clusterID);
    const o = (
      await db.query(
        `SELECT o.*,j.backend_id,j.database_id,j.action AS database_action,j.revision AS database_revision,j.lease_token_hash,j.checkpoint,j.migration_id
 FROM webdock_auth.hosting_operation o JOIN webdock_auth.hosting_database_job j ON j.operation_id=o.id
 WHERE o.id=$1 AND o.cluster_id=$2 FOR UPDATE OF o,j`,
        [input.operationID, agent.clusterID],
      )
    ).rows[0];
    if (
      !o ||
      input.generation !== agent.generation ||
      !o.lease_token_hash ||
      !timingSafeEqual(
        Buffer.from(o.lease_token_hash, "hex"),
        Buffer.from(digest(input.leaseToken), "hex"),
      ) ||
      (!(completed && o.status === "succeeded") &&
        (o.status !== "running" ||
          new Date(o.lease_until).getTime() <= Date.now()))
    )
      conflict();
    if (
      !(completed && o.status === "succeeded") &&
      dependencies.operationAuthority &&
      !(await dependencies.operationAuthority(db, o.id)).allowed
    )
      throw new HostingError(403, "Database provisioning authority changed.");
    return o;
  }
  async function claimDatabaseOperation(agent: Agent) {
    return dependencies.transaction(async (db) => {
      const a = await dependencies.verifyAgent(db, agent);
      if (a.observation?.capabilities?.databaseVersion !== 1) return null;
      await verify(db, agent);
      await dependencies.clusterLock(db, agent.clusterID);
      await db.query(
        `WITH expired AS (
        UPDATE webdock_auth.hosting_operation SET status='needs-reconciliation'
        WHERE cluster_id=$1 AND action LIKE 'databases.%' AND status='running' AND lease_until<now()
        RETURNING id
      ) UPDATE webdock_auth.hosting_database d SET state='needs-reconciliation'
        FROM webdock_auth.hosting_database_job j JOIN expired e ON e.id=j.operation_id
        WHERE d.id=j.database_id AND j.action<>'purgeSource'`,
        [agent.clusterID],
      );
      const row = (
        await db.query(
          `SELECT o.*,j.backend_id,j.database_id,j.action AS database_action,j.revision AS database_revision,j.checkpoint,j.migration_id FROM webdock_auth.hosting_operation o JOIN webdock_auth.hosting_database_job j ON j.operation_id=o.id
  WHERE o.cluster_id=$1 AND (o.status='running' OR (o.status='queued' AND NOT EXISTS(
   SELECT 1 FROM webdock_auth.hosting_database_job busy JOIN webdock_auth.hosting_operation bo ON bo.id=busy.operation_id
   WHERE (busy.backend_id IN (j.backend_id,j.source_backend_id) OR busy.source_backend_id IN (j.backend_id,j.source_backend_id)) AND bo.status IN ('running','needs-reconciliation'))))
  ORDER BY (o.status='running') DESC,o.created_at,o.id FOR UPDATE OF o,j SKIP LOCKED LIMIT 1`,
          [agent.clusterID],
        )
      ).rows[0];
      if (!row) return null;
      if (
        dependencies.operationAuthority &&
        !(await dependencies.operationAuthority(db, row.id)).allowed
      ) {
        await db.query(
          "UPDATE webdock_auth.hosting_operation SET status='needs-reconciliation' WHERE id=$1",
          [row.id],
        );
        if (row.database_id)
          await db.query(
            "UPDATE webdock_auth.hosting_database SET state='needs-reconciliation' WHERE id=$1",
            [row.database_id],
          );
        return null;
      }
      const first = row.status === "queued";
      if (first && !row.checkpoint) {
        try {
          const actor: HostingActor = {
            subject: row.subject,
            sessionID: row.actor_session,
            source: "studio",
            scopes: ["hosting:read", "hosting:write"],
            ...(row.desired?.consoleIdentity
              ? { consoleIdentity: row.desired.consoleIdentity }
              : {}),
          };
          const authorize = () =>
            dependencies.authorizeHosting(
              actor,
              row.project_id
                ? {
                    permission: "org:databases:manage",
                    projectID: row.project_id,
                    write: true,
                    tenantAdmin: true,
                  }
                : { operator: true, write: true },
              db,
            );
          if (actor.consoleIdentity) {
            if (!dependencies.withStoredIdentity) conflict();
            await dependencies.withStoredIdentity(
              actor.consoleIdentity,
              (principal) => {
                if (
                  principal.userID !== actor.subject ||
                  principal.sessionID !== actor.sessionID
                )
                  throw new HostingError(
                    403,
                    "Database operation identity changed.",
                  );
                return authorize();
              },
            );
          } else await authorize();
        } catch {
          await db.query(
            "UPDATE webdock_auth.hosting_operation SET status='needs-reconciliation' WHERE id=$1",
            [row.id],
          );
          if (row.database_id && row.database_action !== "purgeSource")
            await db.query(
              "UPDATE webdock_auth.hosting_database SET state='needs-reconciliation' WHERE id=$1",
              [row.database_id],
            );
          return null;
        }
      }
      if (first) {
        row.generation++;
        row.lease_until = new Date(Date.now() + 300000);
        await db.query(
          "UPDATE webdock_auth.hosting_operation SET status='running',generation=$2,lease_until=$3 WHERE id=$1",
          [row.id, row.generation, row.lease_until],
        );
        await db.query(
          "UPDATE webdock_auth.hosting_database_job SET lease_token_hash=$2 WHERE operation_id=$1",
          [row.id, digest(token(row.id, row.generation))],
        );
      }
      const backend = (
        await db.query(
          "SELECT * FROM webdock_auth.hosting_database_backend WHERE id=$1",
          [row.backend_id],
        )
      ).rows[0];
      const target = row.database_id
        ? (
            await db.query(
              "SELECT d.*,p.plan FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_plan p ON p.id=COALESCE(d.pending_plan_revision_id,d.plan_revision_id) WHERE d.id=$1",
              [row.database_id],
            )
          ).rows[0]
        : null;
      let members = (
        await db.query(
          "SELECT id,project_id,database_name,state FROM webdock_auth.hosting_database WHERE backend_id=$1 AND state<>'purged' ORDER BY id",
          [row.backend_id],
        )
      ).rows.map((d) => ({
        id: d.id,
        projectID: d.project_id,
        databaseName: d.database_name,
        enabled: !["paused", "pausing", "retained", "deleting"].includes(
          d.state,
        ),
      }));
      const retained =
        row.database_action === "purgeSource"
          ? (
              await db.query(
                "SELECT * FROM webdock_auth.hosting_database_migration WHERE id=$1 AND source_backend_id=$2",
                [row.migration_id, row.backend_id],
              )
            ).rows[0]
          : null;
      if (row.database_action === "purgeSource" && !retained) conflict();
      const migration = ["prepareMigration", "migrate"].includes(
        row.database_action,
      )
        ? (
            await db.query(
              `SELECT m.*,b.cnpg_uid,b.host,b.direct_port,b.kind AS source_kind FROM webdock_auth.hosting_database_migration m JOIN webdock_auth.hosting_database_backend b ON b.id=m.source_backend_id WHERE m.database_id=$1 AND m.target_backend_id=$2 AND m.state IN ('preparing','copying','needs-reconciliation')`,
              [row.database_id, row.backend_id],
            )
          ).rows[0]
        : null;
      if (
        ["prepareMigration", "migrate"].includes(row.database_action) &&
        !migration
      )
        conflict();
      if (row.database_action === "purgeSource")
        members = [
          ...members.filter((m) => m.id !== target.id),
          {
            id: target.id,
            projectID: target.project_id,
            databaseName: target.database_name,
            enabled: false,
          },
        ];
      if (row.database_action === "prepareMigration") members = [];
      if (row.database_action === "migrate")
        members = [
          {
            id: target.id,
            projectID: target.project_id,
            databaseName: target.database_name,
            enabled: true,
          },
        ];
      const phase =
        first && !row.checkpoint
          ? "apply"
          : row.checkpoint?.phase === "applied"
            ? "configure"
            : row.checkpoint?.phase === "observing"
              ? "observe"
              : "uncertain";
      return {
        version: 1,
        operationID: row.id,
        generation: agent.generation,
        leaseToken: token(row.id, row.generation),
        leaseUntil: new Date(row.lease_until).toISOString(),
        clusterID: agent.clusterID,
        action:
          row.database_action === "purgeSource" ? "purge" : row.database_action,
        phase,
        revision: row.database_revision,
        previousPodUID: row.checkpoint?.previousPodUID ?? null,
        backend: {
          id: backend.id,
          kind: backend.kind,
          namespace: "wd-db-" + backend.id,
          node: backend.node,
          poolID: backend.pool_id,
          revision: backend.revision,
          cnpgUID: backend.cnpg_uid,
          resources: backend.profile,
          host: backend.host,
          directPort: backend.direct_port,
          pooledPort: backend.pooled_port,
        },
        database:
          target && row.database_action !== "prepareMigration"
            ? {
                id: target.id,
                kind:
                  row.database_action === "purgeSource"
                    ? backend.kind === "shared-pool"
                      ? "shared"
                      : "dedicated"
                    : row.database_action === "migrate"
                      ? "dedicated"
                      : target.kind,
                name: target.database_name,
                revision: target.revision,
                credentialRevision: retained
                  ? retained.source_credential_revision
                  : target.credential_revision,
                connectionLimit: target.plan.connectionLimit,
                credentials: dependencies.decryptSecret<{
                  applicationPassword: string;
                  migrationPassword: string;
                }>(
                  retained
                    ? retained.source_credentials_encrypted
                    : target.credentials_encrypted,
                  `database:${target.id}:${retained ? retained.source_credential_revision : target.credential_revision}`,
                ),
              }
            : null,
        members,
        ...(row.database_action === "migrate"
          ? {
              migration: {
                id: migration.id,
                source: {
                  id: migration.source_backend_id,
                  namespace: "wd-db-" + migration.source_backend_id,
                  cnpgUID: migration.cnpg_uid,
                  host: migration.host,
                  kind: migration.source_kind,
                },
                transferPassword: dependencies.decryptSecret<{
                  password: string;
                }>(
                  migration.transfer_credentials_encrypted,
                  "database-migration:" + migration.id,
                ).password,
                jobUID: migration.job_uid,
                copySHA256: migration.copy_sha256,
              },
            }
          : {}),
      };
    });
  }
  async function checkpointDatabaseOperation(agent: Agent, input: unknown) {
    const data = checkpointSchema.parse(input);
    return dependencies.transaction(async (db) => {
      const o = await job(db, agent, data);
      const backend = (
        await db.query(
          "SELECT cnpg_uid,ca_certificate FROM webdock_auth.hosting_database_backend WHERE id=$1 FOR UPDATE",
          [o.backend_id],
        )
      ).rows[0];
      if (backend.cnpg_uid && backend.cnpg_uid !== data.cnpgUID) conflict();
      if (data.caCertificate) {
        let ca: X509Certificate;
        try {
          ca = new X509Certificate(data.caCertificate);
        } catch {
          conflict();
        }
        if (
          !ca.ca ||
          data.caCertificate.includes("PRIVATE KEY") ||
          Date.parse(ca.validTo) <= Date.now() ||
          Date.parse(ca.validFrom) > Date.now() ||
          (backend.ca_certificate &&
            backend.ca_certificate !== data.caCertificate)
        )
          conflict();
        await db.query(
          "UPDATE webdock_auth.hosting_database_backend SET ca_certificate=$2 WHERE id=$1",
          [o.backend_id, data.caCertificate],
        );
      }
      const previous = o.checkpoint?.phase;
      if (
        (data.phase === "applied" && previous) ||
        (data.phase === "configuring" && previous !== "applied") ||
        (data.phase === "observing" &&
          !["configuring", "observing"].includes(previous))
      )
        conflict();
      await db.query(
        "UPDATE webdock_auth.hosting_database_backend SET cnpg_uid=$2 WHERE id=$1",
        [o.backend_id, data.cnpgUID],
      );
      await db.query(
        "UPDATE webdock_auth.hosting_database_job SET checkpoint=$2 WHERE operation_id=$1",
        [
          o.id,
          JSON.stringify({
            ...o.checkpoint,
            phase: data.phase,
            cnpgUID: data.cnpgUID,
            ...(data.previousPodUID
              ? { previousPodUID: data.previousPodUID }
              : {}),
            ...(data.migrationJobUID
              ? { migrationJobUID: data.migrationJobUID }
              : {}),
            ...(data.copySHA256 ? { copySHA256: data.copySHA256 } : {}),
          }),
        ],
      );
      if (data.migrationJobUID || data.copySHA256) {
        if (o.database_action !== "migrate") conflict();
        const m = (
          await db.query(
            "SELECT job_uid FROM webdock_auth.hosting_database_migration WHERE database_id=$1 AND target_backend_id=$2 FOR UPDATE",
            [o.database_id, o.backend_id],
          )
        ).rows[0];
        if (
          !m ||
          (m.job_uid &&
            data.migrationJobUID &&
            m.job_uid !== data.migrationJobUID)
        )
          conflict();
        await db.query(
          "UPDATE webdock_auth.hosting_database_migration SET job_uid=COALESCE(job_uid,$3),copy_sha256=COALESCE($4,copy_sha256) WHERE database_id=$1 AND target_backend_id=$2",
          [
            o.database_id,
            o.backend_id,
            data.migrationJobUID ?? null,
            data.copySHA256 ?? null,
          ],
        );
      }
      await db.query(
        "UPDATE webdock_auth.hosting_operation SET lease_until=now()+interval '300 seconds' WHERE id=$1",
        [o.id],
      );
      return { accepted: true };
    });
  }
  async function completeDatabaseOperation(agent: Agent, input: unknown) {
    const data = completionSchema.parse(input);
    return dependencies.transaction(async (db) => {
      const o = await job(db, agent, data, true);
      if (o.status === "succeeded") {
        if (
          data.outcome !== "succeeded" ||
          JSON.stringify(proofSchema.parse(o.result)) !==
            JSON.stringify(data.proof)
        )
          conflict();
        return { accepted: true };
      }
      if (data.outcome === "needs-reconciliation") {
        await db.query(
          "UPDATE webdock_auth.hosting_operation SET status='needs-reconciliation' WHERE id=$1",
          [o.id],
        );
        if (o.database_id && o.database_action !== "purgeSource")
          await db.query(
            "UPDATE webdock_auth.hosting_database SET state='needs-reconciliation' WHERE id=$1",
            [o.database_id],
          );
        return { accepted: true };
      }
      const proof = data.proof,
        backend = (
          await db.query(
            "SELECT * FROM webdock_auth.hosting_database_backend WHERE id=$1 FOR UPDATE",
            [o.backend_id],
          )
        ).rows[0];
      const expected =
        o.database_action === "pause"
          ? "paused"
          : o.database_action === "delete"
            ? "retained"
            : ["purge", "purgeSource"].includes(o.database_action)
              ? "purged"
              : "ready";
      if (
        !proof ||
        o.checkpoint?.phase !== "observing" ||
        proof.installationID !== o.backend_id ||
        proof.databaseID !==
          (o.database_action === "prepareMigration" ? null : o.database_id) ||
        proof.cnpgUID !== backend.cnpg_uid ||
        proof.revision !== o.database_revision ||
        proof.state !== expected ||
        (expected === "ready" && !proof.endpointReady)
      )
        conflict();
      if (o.database_action === "purgeSource") {
        const changed = await db.query(
          "UPDATE webdock_auth.hosting_database_migration SET state='purged',source_credentials_encrypted='',transfer_credentials_encrypted='' WHERE id=$1 AND source_backend_id=$2 AND state='retained' RETURNING id",
          [o.migration_id, o.backend_id],
        );
        if (!changed.rowCount) conflict();
        if (backend.kind === "dedicated") {
          await db.query(
            "UPDATE webdock_auth.hosting_database_backend SET state='purged',endpoint_ready=false,observed_at=now() WHERE id=$1",
            [backend.id],
          );
          await db.query(
            "DELETE FROM webdock_auth.hosting_database_port WHERE backend_id=$1",
            [backend.id],
          );
        }
        await db.query(
          "UPDATE webdock_auth.hosting_operation SET status='succeeded',result=$2 WHERE id=$1",
          [o.id, JSON.stringify(proof)],
        );
        return { accepted: true };
      }
      if (o.database_action === "prepareMigration") {
        await db.query(
          "UPDATE webdock_auth.hosting_database_backend SET state='ready',observed_revision=revision,observed_at=now(),endpoint_ready=true WHERE id=$1",
          [backend.id],
        );
        await db.query(
          "UPDATE webdock_auth.hosting_database_job SET action='migrate',checkpoint=NULL,lease_token_hash=NULL WHERE operation_id=$1",
          [o.id],
        );
        await db.query(
          "UPDATE webdock_auth.hosting_database_migration SET state='copying' WHERE database_id=$1 AND target_backend_id=$2",
          [o.database_id, o.backend_id],
        );
        await db.query(
          "UPDATE webdock_auth.hosting_operation SET status='queued',action='databases.migrate',lease_until=NULL WHERE id=$1",
          [o.id],
        );
        return { accepted: true, pending: true };
      }
      if (o.database_id) {
        if (o.database_action === "migrate") {
          const m = (
            await db.query(
              "SELECT * FROM webdock_auth.hosting_database_migration WHERE database_id=$1 AND target_backend_id=$2 FOR UPDATE",
              [o.database_id, o.backend_id],
            )
          ).rows[0];
          if (
            !m?.job_uid ||
            !m.copy_sha256 ||
            proof.copySHA256 !== m.copy_sha256
          )
            conflict();
          await db.query(
            "UPDATE webdock_auth.hosting_database_migration SET state='retained',retained_until=now()+interval '7 days' WHERE id=$1",
            [m.id],
          );
          await db.query(
            "UPDATE webdock_auth.hosting_database SET backend_id=pending_backend_id,pending_backend_id=NULL,kind='dedicated' WHERE id=$1",
            [o.database_id],
          );
          await db.query(
            "UPDATE webdock_auth.hosting_database_backend SET state='retained',endpoint_ready=false WHERE id=$1 AND kind='dedicated'",
            [m.source_backend_id],
          );
        }
        if (o.database_action === "resize" || o.database_action === "migrate") {
          const target = (
            await db.query(
              "SELECT pending_order_id,pending_plan_revision_id,order_id FROM webdock_auth.hosting_database WHERE id=$1 FOR UPDATE",
              [o.database_id],
            )
          ).rows[0];
          if (!target?.pending_order_id || !target.pending_plan_revision_id)
            conflict();
          await db.query(
            "UPDATE webdock_auth.hosting_database_order SET ended_at=COALESCE(ended_at,now()) WHERE id=$1",
            [target.order_id],
          );
          await db.query(
            "UPDATE webdock_auth.hosting_database SET order_id=pending_order_id,plan_revision_id=pending_plan_revision_id,pending_order_id=NULL,pending_plan_revision_id=NULL WHERE id=$1",
            [o.database_id],
          );
        }
        const updated = await db.query(
          `UPDATE webdock_auth.hosting_database SET state=$3,observed_revision=revision,observed_at=now(),
    confirmed_credential_revision=CASE WHEN $3='ready' THEN credential_revision ELSE confirmed_credential_revision END,
    confirmed_credentials_encrypted=CASE WHEN $3='ready' THEN credentials_encrypted ELSE confirmed_credentials_encrypted END,
    retained_until=CASE WHEN $3='retained' THEN now()+interval '7 days' ELSE retained_until END
    WHERE id=$1 AND revision=$2 RETURNING order_id`,
          [o.database_id, o.database_revision, expected],
        );
        if (!updated.rowCount) conflict();
        if (expected === "ready")
          await db.query(
            "UPDATE webdock_auth.hosting_database_order SET billing_started_at=COALESCE(billing_started_at,now()) WHERE id=$1",
            [updated.rows[0].order_id],
          );
        if (expected === "retained")
          await db.query(
            "UPDATE webdock_auth.hosting_database_order SET ended_at=COALESCE(ended_at,now()) WHERE id=$1",
            [updated.rows[0].order_id],
          );
      }
      if (backend.kind === "dedicated" || !o.database_id)
        await db.query(
          "UPDATE webdock_auth.hosting_database_backend SET state=$2,observed_revision=revision,observed_at=now(),endpoint_ready=$3 WHERE id=$1",
          [backend.id, expected, proof.endpointReady && expected === "ready"],
        );
      else if (expected === "ready")
        await db.query(
          "UPDATE webdock_auth.hosting_database_backend SET endpoint_ready=true,observed_at=now() WHERE id=$1",
          [backend.id],
        );
      if (expected === "purged" && backend.kind === "dedicated")
        await db.query(
          "DELETE FROM webdock_auth.hosting_database_port WHERE backend_id=$1",
          [backend.id],
        );
      await db.query(
        "UPDATE webdock_auth.hosting_operation SET status='succeeded',result=$2 WHERE id=$1",
        [o.id, JSON.stringify(proof)],
      );
      return { accepted: true };
    });
  }
  async function guardDatabaseOperation(agent: Agent, input: unknown) {
    const data = leaseSchema.strict().parse(input);
    return dependencies.transaction(async (db) => {
      await job(db, agent, data);
      return { valid: true };
    });
  }
  return {
    claimDatabaseOperation,
    checkpointDatabaseOperation,
    completeDatabaseOperation,
    guardDatabaseOperation,
  };
}
