import type { HostingPorts } from "./ports";
import { z } from "zod";
import { totals } from "./allowances";
import { createHash, createHmac } from "node:crypto";
import {
  HostingError,
  appDemand,
  resolveAppSpec,
  type HostingActor,
  type HostingOperationStatus,
} from "@spitzli/hosting-contracts";
import type { Connection } from "./authorization";
import type { Agent } from "./clusters";
export type Dependencies = {
  operationAuthority?(
    db: Connection,
    id: string,
  ): Promise<{ required: boolean; allowed: boolean }>;
  revokeBindingMail: ReturnType<
    typeof import("./mail-binding").createHostingMailBinding
  >["revokeBindingMail"];
  openEnvironment: ReturnType<
    typeof import("./environment").createHostingEnvironment
  >["openEnvironment"];
  redactValues: ReturnType<
    typeof import("./environment").createHostingEnvironment
  >["redactValues"];
  transaction: ReturnType<typeof import("./db").createHostingDb>["transaction"];
  verifyAgent: ReturnType<
    typeof import("./clusters").createHostingClusters
  >["verifyAgent"];
  env: HostingPorts["env"];
};

export function createHostingOperations(dependencies: Dependencies) {
  function transition(
    state: HostingOperationStatus,
    event:
      | "claim"
      | "lease-expired"
      | "succeeded"
      | "failed"
      | "release-without-proof",
  ): HostingOperationStatus {
    if (state === "queued" && event === "claim") return "running";
    if (state === "running" && event === "lease-expired")
      return "needs-reconciliation";
    if (state === "running" && (event === "succeeded" || event === "failed"))
      return event;
    throw new HostingError(409, "Invalid hosting operation transition.");
  }
  function canonical(value: unknown): string {
    if (Array.isArray(value)) return "[" + value.map(canonical).join(",") + "]";
    if (value && typeof value === "object")
      return (
        "{" +
        Object.entries(value)
          .sort(([a], [b]) => a.localeCompare(b))
          .filter(([, v]) => v !== undefined)
          .map(([k, v]) => JSON.stringify(k) + ":" + canonical(v))
          .join(",") +
        "}"
      );
    return JSON.stringify(value);
  }
  async function replay(
    db: Connection,
    actor: HostingActor,
    key: string,
    payload: unknown,
  ) {
    const secretPayload =
      payload && typeof payload === "object" && "environment" in payload;
    if (
      secretPayload &&
      (!dependencies.env.BETTER_AUTH_SECRET ||
        dependencies.env.BETTER_AUTH_SECRET.length < 32)
    )
      throw new HostingError(
        503,
        "Application environment encryption is not configured.",
      );
    const hash = (
      secretPayload
        ? createHmac("sha256", dependencies.env.BETTER_AUTH_SECRET!)
        : createHash("sha256")
    )
      .update(canonical(payload))
      .digest("hex");
    await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
      `hosting-request:${actor.subject}:${key}`,
    ]);
    const existing = (
      await db.query(
        "SELECT * FROM webdock_auth.hosting_operation WHERE subject=$1 AND idempotency_key=$2",
        [actor.subject, key],
      )
    ).rows[0];
    if (existing && existing.payload_hash !== hash)
      throw new HostingError(
        409,
        "Idempotency key already used with different input.",
      );
    return { hash, existing };
  }
  async function recordResult(
    db: Connection,
    actor: HostingActor,
    key: string,
    hash: string,
    action: string,
    result: unknown,
    target: {
      customerID?: string;
      projectID?: string;
      clusterID?: string;
    } = {},
  ) {
    await db.query(
      "INSERT INTO webdock_auth.hosting_operation(subject,idempotency_key,payload_hash,action,status,result,customer_id,project_id,cluster_id) VALUES($1,$2,$3,$4,'succeeded',$5,$6,$7,$8)",
      [
        actor.subject,
        key,
        hash,
        action,
        JSON.stringify(result),
        target.customerID,
        target.projectID,
        target.clusterID,
      ],
    );
  }
  async function claimOperation(agent: Agent) {
    return dependencies.transaction(async (db) => {
      const agentState = await dependencies.verifyAgent(db, agent);
      await db.query(
        "UPDATE webdock_auth.hosting_operation SET status=CASE WHEN action='apps.logs' THEN 'failed' ELSE 'needs-reconciliation' END WHERE cluster_id=$1 AND status='running' AND lease_until<now()",
        [agent.clusterID],
      );
      const row = (
        await db.query(
          `SELECT o.* FROM webdock_auth.hosting_operation o WHERE o.cluster_id=$1 AND o.status='queued' AND o.action<>'byok.workload' AND o.action NOT LIKE 'databases.%'
 AND (o.desired->>'storageVersion' IS DISTINCT FROM '1' OR EXISTS(SELECT 1 FROM webdock_auth.hosting_agent a WHERE a.cluster_id=o.cluster_id AND a.observation->'capabilities'->>'storageVersion'='1' AND (a.observation->'capabilities'->>'storage'='true' OR o.action IN ('apps.delete','apps.purgeStorage','apps.logs'))))
 AND (o.desired->>'serviceBindingsVersion' IS DISTINCT FROM '1' OR EXISTS(SELECT 1 FROM webdock_auth.hosting_agent a WHERE a.cluster_id=o.cluster_id AND a.observation->'capabilities'->>'serviceBindingsVersion'='1'))
 AND (o.desired->>'environmentVersion' IS DISTINCT FROM '1' OR EXISTS(SELECT 1 FROM webdock_auth.hosting_agent a WHERE a.cluster_id=o.cluster_id AND a.observation->'capabilities'->>'environment'='true'))
 AND NOT EXISTS(SELECT 1 FROM webdock_auth.hosting_operation busy WHERE busy.project_id=o.project_id AND (busy.status='running' OR (busy.status='needs-reconciliation' AND o.action<>'apps.logs')))
 ORDER BY o.created_at,o.id FOR UPDATE SKIP LOCKED LIMIT 1`,
          [agent.clusterID],
        )
      ).rows[0];
      if (!row) return null;
      const authority = await dependencies.operationAuthority?.(db, row.id);
      if (authority && !authority.allowed) {
        await db.query(
          "UPDATE webdock_auth.hosting_operation SET status='failed',result=$2 WHERE id=$1 AND status='queued'",
          [
            row.id,
            JSON.stringify({
              error: "Application provisioning authority changed.",
            }),
          ],
        );
        await db.query(
          "UPDATE webdock_auth.hosting_app SET status='failed',last_error=$3 WHERE id=$1 AND revision=$2",
          [
            row.app_id,
            row.target_revision,
            "Resume application provisioning with current authority.",
          ],
        );
        return null;
      }
      if (
        authority?.required &&
        agentState.observation?.capabilities?.provisioningGuardVersion !== 1
      )
        return null;
      const locked = (
        await db.query(
          "SELECT pg_try_advisory_xact_lock(hashtextextended($1,0)) AS locked",
          [`hosting-target:${row.project_id}`],
        )
      ).rows[0].locked;
      if (!locked) return null;
      const busy = (
        await db.query(
          "SELECT 1 FROM webdock_auth.hosting_operation WHERE project_id=$1 AND (status='running' OR (status='needs-reconciliation' AND $2<>'apps.logs'))",
          [row.project_id, row.action],
        )
      ).rowCount;
      if (busy) return null;
      const claimed = (
        await db.query(
          "UPDATE webdock_auth.hosting_operation SET status='running',generation=generation+1,lease_until=now()+interval '300 seconds' WHERE id=$1 RETURNING id,action,project_id,generation,lease_until,desired,target_revision",
          [row.id],
        )
      ).rows[0];
      if (claimed.desired) {
        const reservations = (
          await db.query(
            "SELECT demand,status FROM webdock_auth.hosting_reservation WHERE project_id=$1 AND status<>'released'",
            [row.project_id],
          )
        ).rows;
        const apps = (
          await db.query(
            "SELECT id,spec,status FROM webdock_auth.hosting_app WHERE project_id=$1 AND lifecycle='deployed' AND (status<>'deleted' OR COALESCE((spec->>'volumeBytes')::bigint,0)>0)",
            [row.project_id],
          )
        ).rows;
        const finalRows = apps.map((a) => {
          const target = a.id === row.app_id;
          const purged = target && claimed.desired.action === "purge-storage";
          const deleted =
            a.status === "deleted" ||
            (target &&
              ["delete", "purge-storage"].includes(claimed.desired.action));
          const spec = resolveAppSpec(target ? claimed.desired.spec : a.spec);
          return {
            demand: appDemand(
              purged ? { ...spec, volumeBytes: 0 } : spec,
              deleted,
            ),
            status: "active",
          };
        });
        claimed.desired = {
          ...claimed.desired,
          ...(authority?.required ? { pluginProvisioningGuard: true } : {}),
          quota: totals(reservations),
          finalQuota: totals(finalRows),
          operationID: claimed.id,
          generation: claimed.generation,
          leaseUntil: claimed.lease_until.toISOString(),
        };
        if (claimed.desired.environmentVersion === 1) {
          claimed.desired.environment = dependencies.openEnvironment(
            row.app_id,
            claimed.desired.environmentEncrypted,
          );
          delete claimed.desired.environmentEncrypted;
        }
      }
      return claimed;
    });
  }
  const proofSchema = z
    .object({
      appID: z.string().regex(/^[1-9][0-9]*$/),
      revision: z.number().int().positive(),
      namespace: z.string().regex(/^wd-[1-9][0-9]*$/),
      uid: z.string().min(1).max(128).nullable(),
      ready: z.boolean(),
      deleted: z.boolean(),
      replicas: z.number().int().min(0).max(20),
      logs: z.string().max(16000).optional(),
      logsBase64: z
        .string()
        .max(16000)
        .regex(
          /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/,
        )
        .optional(),
      logsTruncated: z.boolean().optional(),
      serviceBindings: z
        .array(
          z
            .object({
              id: z.string().regex(/^[1-9][0-9]*$/),
              revision: z.number().int().positive(),
            })
            .strict(),
        )
        .max(32)
        .optional(),
      storage: z
        .object({
          volumeBytes: z.number().int().nonnegative(),
          retained: z.boolean(),
        })
        .strict()
        .optional(),
    })
    .strict();
  async function guardOperation(
    agent: Agent,
    input: { id: string; generation: number },
  ) {
    return dependencies.transaction(async (db) => {
      await dependencies.verifyAgent(db, agent);
      const row = (
        await db.query(
          "SELECT * FROM webdock_auth.hosting_operation WHERE id=$1 AND cluster_id=$2",
          [input.id, agent.clusterID],
        )
      ).rows[0];
      if (
        !row ||
        row.generation !== input.generation ||
        row.status !== "running" ||
        new Date(row.lease_until).getTime() <= Date.now()
      )
        throw new HostingError(409, "Operation lease changed.");
      const authority = await dependencies.operationAuthority?.(db, row.id);
      if (authority && !authority.allowed)
        throw new HostingError(
          403,
          "Application provisioning authority changed.",
        );
      return { valid: true as const };
    });
  }
  async function completeOperation(
    agent: Agent,
    input: {
      id: string;
      generation: number;
      outcome: "succeeded" | "failed";
      proof?: unknown;
      error?: string;
    },
  ) {
    return dependencies.transaction(async (db) => {
      await dependencies.verifyAgent(db, agent);
      const row = (
        await db.query(
          "SELECT * FROM webdock_auth.hosting_operation WHERE id=$1 AND cluster_id=$2 FOR UPDATE",
          [input.id, agent.clusterID],
        )
      ).rows[0];
      if (!row || row.generation !== input.generation)
        throw new HostingError(409, "Operation lease changed.");
      if (row.status === "succeeded" && input.outcome === "succeeded")
        return { id: row.id, status: row.status };
      if (
        row.status !== "running" ||
        new Date(row.lease_until).getTime() <= Date.now()
      )
        throw new HostingError(
          409,
          "Operation lease expired. Reconcile before retrying.",
        );
      if (input.outcome === "failed") {
        await db.query(
          "INSERT INTO webdock_auth.hosting_audit(subject,action,target_id,outcome) VALUES($1,$2,$3,'failed')",
          [row.subject, row.action + ".complete", row.app_id ?? row.id],
        );
        if (row.action === "apps.logs") {
          const message = "The application logs could not be read.";
          await db.query(
            "UPDATE webdock_auth.hosting_operation SET status='failed',result=$2 WHERE id=$1",
            [row.id, JSON.stringify({ error: message })],
          );
          await db.query(
            "UPDATE webdock_auth.hosting_app SET last_error=COALESCE(last_error,$2) WHERE id=$1",
            [row.app_id, message],
          );
          return { id: row.id, status: "failed" };
        }
        const messages: Record<string, string> = {
          admission_rejected:
            "Kubernetes rejected the application safety profile.",
          rollout_timeout:
            "The application did not become ready. Inspect the healthcheck and resources.",
          ownership_conflict:
            "Managed resource ownership changed. Operator review is required.",
          execution_failed:
            "Cluster execution failed. Inspect and reconcile before retrying.",
        };
        const message =
          messages[input.error ?? ""] ?? messages.execution_failed;
        await db.query(
          "UPDATE webdock_auth.hosting_operation SET status='needs-reconciliation',result=$2 WHERE id=$1",
          [row.id, JSON.stringify({ error: message })],
        );
        if (row.app_id)
          await db.query(
            "UPDATE webdock_auth.hosting_app SET status=CASE WHEN $4 THEN 'deleted' ELSE 'failed' END,last_error=$2 WHERE id=$1 AND operation_id=$3",
            [row.app_id, message, row.id, row.action === "apps.purgeStorage"],
          );
        return { id: row.id, status: "needs-reconciliation" };
      }
      const proof = proofSchema.parse(input.proof);
      const truncatedLogs =
        proof.logsTruncated ?? (proof.logs?.length ?? 0) >= 5996;
      delete proof.logsTruncated;
      if (row.action !== "apps.logs") delete proof.logs;
      if (row.action === "apps.logs" && proof.logsBase64 !== undefined)
        proof.logs = Buffer.from(proof.logsBase64, "base64").toString("utf8");
      delete proof.logsBase64;
      if (
        !row.app_id ||
        proof.appID !== row.app_id ||
        proof.revision !== row.target_revision ||
        proof.namespace !== row.desired?.namespace
      )
        throw new HostingError(
          409,
          "Workload proof does not match the operation.",
        );
      const app = (
        await db.query(
          "SELECT * FROM webdock_auth.hosting_app WHERE id=$1 FOR UPDATE",
          [row.app_id],
        )
      ).rows[0];
      if (!app || app.revision !== row.target_revision)
        throw new HostingError(409, "Application revision changed.");
      if (row.action === "apps.logs") {
        if (
          !proof.uid ||
          proof.deleted ||
          (app.observed_uid && proof.uid !== app.observed_uid)
        )
          throw new HostingError(409, "Application log ownership changed.");
        const history = (
          await db.query(
            "SELECT DISTINCT desired->>'environmentEncrypted' AS encrypted FROM webdock_auth.hosting_operation WHERE app_id=$1 AND desired->>'environmentEncrypted' IS NOT NULL",
            [app.id],
          )
        ).rows;
        const known = [
          app.environment_encrypted,
          app.previous_environment_encrypted,
          app.observed_environment_encrypted,
          ...history.map((r) => r.encrypted),
        ];
        proof.logs = dependencies.redactValues(
          proof.logs ?? "",
          known.flatMap((sealed) =>
            Object.values(dependencies.openEnvironment(app.id, sealed)),
          ),
          truncatedLogs,
        );
        proof.logs = proof.logs.slice(0, 16000);
        await db.query(
          "UPDATE webdock_auth.hosting_app SET logs=$2,last_error=CASE WHEN last_error='The application logs could not be read.' THEN NULL ELSE last_error END WHERE id=$1",
          [app.id, proof.logs ?? ""],
        );
      } else {
        const purging = row.desired.action === "purge-storage";
        const deleting = row.desired.action === "delete" || purging;
        if (
          row.desired.storageVersion === 1 &&
          (!proof.storage ||
            proof.storage.volumeBytes !==
              (purging ? 0 : row.desired.spec.volumeBytes) ||
            proof.storage.retained !== (deleting && !purging))
        )
          throw new HostingError(
            409,
            "Persistent storage proof does not match the operation.",
          );
        if (row.desired.serviceBindingsVersion === 1) {
          if (proof.replicas > 0)
            for (const binding of (
              await db.query(
                "SELECT id FROM webdock_auth.hosting_service_binding WHERE app_id=$1 AND operation_id=$2 AND retiring_mail_key_id IS NOT NULL",
                [row.app_id, row.id],
              )
            ).rows)
              await dependencies.revokeBindingMail(db, binding.id, true);
          const expected = (
            row.desired.serviceBindings as {
              id: string;
              revision: number;
            }[]
          ).map(({ id, revision }) => ({ id, revision }));
          if (
            JSON.stringify(proof.serviceBindings) !== JSON.stringify(expected)
          )
            throw new HostingError(
              409,
              "Service connection proof does not match the operation.",
            );
          await db.query(
            "UPDATE webdock_auth.hosting_service_binding SET observed_revision=revision,state=CASE WHEN state='disconnecting' THEN 'disconnected' ELSE 'connected' END,secret_encrypted=CASE WHEN state='disconnecting' THEN NULL ELSE secret_encrypted END WHERE app_id=$1 AND operation_id=$2 AND ($3 OR state='disconnecting')",
            [row.app_id, row.id, proof.replicas > 0],
          );
        }
        const completedSpec = purging
          ? { ...row.desired.spec, volumeBytes: 0 }
          : row.desired.spec;
        if (
          !proof.ready ||
          proof.deleted !== deleting ||
          (!deleting &&
            (!proof.uid || proof.replicas !== row.desired.spec.replicas)) ||
          (deleting && (proof.uid !== null || proof.replicas !== 0))
        )
          throw new HostingError(
            409,
            "Application has not converged to the requested state.",
          );
        if (app.observed_uid && proof.uid && app.observed_uid !== proof.uid)
          throw new HostingError(409, "Application ownership changed.");
        await db.query(
          "UPDATE webdock_auth.hosting_reservation SET status='released' WHERE app_id=$1 AND operation_id<>$2",
          [app.id, row.id],
        );
        await db.query(
          "UPDATE webdock_auth.hosting_reservation SET status=$2,demand=$3 WHERE operation_id=$1",
          [
            row.id,
            deleting && !completedSpec.volumeBytes ? "released" : "active",
            JSON.stringify(appDemand(resolveAppSpec(completedSpec), deleting)),
          ],
        );
        await db.query(
          "UPDATE webdock_auth.hosting_app SET status=$2,observed_revision=$3,observed_uid=$4,observed_spec=$5,observed_environment_encrypted=$6,last_error=NULL WHERE id=$1",
          [
            app.id,
            deleting ? "deleted" : proof.replicas === 0 ? "stopped" : "ready",
            proof.revision,
            proof.uid,
            JSON.stringify(completedSpec),
            row.desired.environmentEncrypted ?? null,
          ],
        );
        if (purging)
          await db.query(
            "UPDATE webdock_auth.hosting_app SET spec=$2,previous_spec=NULL WHERE id=$1",
            [app.id, JSON.stringify(completedSpec)],
          );
        if (deleting) {
          await db.query(
            "UPDATE webdock_auth.hosting_app SET environment_encrypted=NULL,previous_environment_encrypted=NULL,observed_environment_encrypted=NULL,logs=NULL WHERE id=$1",
            [app.id],
          );
          await db.query(
            "UPDATE webdock_auth.hosting_operation SET desired=desired-'environmentEncrypted' WHERE app_id=$1",
            [app.id],
          );
        }
      }
      await db.query(
        "UPDATE webdock_auth.hosting_operation SET status='succeeded',result=$2 WHERE id=$1",
        [
          row.id,
          JSON.stringify({
            appID: app.id,
            status: ["delete", "purge-storage"].includes(row.desired.action)
              ? "deleted"
              : "observed",
            proof,
          }),
        ],
      );
      await db.query(
        "INSERT INTO webdock_auth.hosting_audit(subject,action,target_id,outcome) VALUES($1,$2,$3,'succeeded')",
        [row.subject, row.action + ".complete", app.id],
      );
      return { id: row.id, status: "succeeded", appID: app.id };
    });
  }
  return {
    transition,
    replay,
    recordResult,
    claimOperation,
    guardOperation,
    completeOperation,
  };
}
