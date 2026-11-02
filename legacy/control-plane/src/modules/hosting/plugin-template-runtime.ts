import { createHash, randomBytes } from "node:crypto";
import { isIP } from "node:net";
import { isDeepStrictEqual } from "node:util";
import { z } from "zod";
import {
  HostingError,
  appDemand,
  setupConfigurationSchema,
} from "@webdock/hosting-contracts";
import { allocationTotals } from "@webdock/hosting-contracts/capacity-totals";
import type { Agent } from "@webdock/module-hosting/server/clusters";
import type {
  ApprovedAppTemplate,
  ApprovedAppTemplateHooks,
  TemplateHookContext,
} from "./plugin-provisioning";
import {
  updateTemplateState,
  assertProvisioningActor,
} from "./plugin-provisioning-store";

type Connection = import("./authorization").Connection;
async function transaction<T>(run: (db: Connection) => Promise<T>): Promise<T> {
  return (await import("./db")).transaction(run);
}
const planLock: typeof import("./db").planLock = async (...args) =>
  (await import("./db")).planLock(...args);
const clusterLock: typeof import("./db").clusterLock = async (...args) =>
  (await import("./db")).clusterLock(...args);
const verifyAgent: typeof import("./clusters").verifyAgent = async (...args) =>
  (await import("./clusters")).verifyAgent(...args);
const checkDemand: typeof import("./reservations").checkDemand = async (
  ...args
) => (await import("./reservations")).checkDemand(...args);
const authorizeHosting: typeof import("./authorization").authorizeHosting =
  async (...args) =>
    (await import("./authorization")).authorizeHosting(...args);

const approvalSchema = z
  .object({
    image: z
      .string()
      .regex(/^[a-z0-9.-]+\/[a-zA-Z0-9_./-]+@sha256:[a-f0-9]{64}$/),
    domain: z.string().regex(/^[a-z0-9-]+(?:\.[a-z0-9-]+)+$/),
    addresses: z
      .array(z.string().refine((value) => !!isIP(value)))
      .min(1)
      .max(8),
    certificateIssuer: z.string().regex(/^[a-z0-9][a-z0-9-]{0,62}$/),
    ingressClass: z.string().regex(/^[a-z0-9][a-z0-9-]{0,62}$/),
    authIssuer: z
      .string()
      .url()
      .refine((value) => {
        const url = new URL(value);
        return (
          url.protocol === "https:" &&
          !url.username &&
          !url.password &&
          !url.search &&
          !url.hash &&
          url.pathname === "/api/auth"
        );
      }),
    authAddresses: z
      .array(z.string().refine((value) => !!isIP(value)))
      .min(1)
      .max(8),
    planRevisionID: z.string().regex(/^[1-9][0-9]{0,18}$/),
  })
  .strict();
type Approval = z.infer<typeof approvalSchema>;
type Operation = {
  phase: "migration" | "publication";
  status: "queued" | "running" | "succeeded" | "needs-reconciliation";
  generation: number;
  leaseToken?: string;
  leaseUntil?: string;
  jobUID?: string;
  certificateUID?: string;
  ingressUID?: string;
  intentRecorded?: boolean;
  migrationSucceeded?: boolean;
  storageState?: "unallocated" | "allocating" | "ready" | "unknown";
  proof?: Record<string, unknown>;
};
type State = {
  instanceID: string;
  clientID: string;
  origin: string;
  secrets: string;
  oauthSecretHash: string;
  approval: Approval;
  migration?: Operation;
  publication?: Operation;
  reservationID?: string;
};
const state = (value: Record<string, unknown>) => value as unknown as State;
const purpose = (id: string) => "plugin-template:" + id;
const environment = async (id: string, s: State) =>
  (await import("../platform/platform")).decryptMailSecret<
    Record<string, string>
  >(s.secrets, purpose(id));
const fields =
  "p.*,s.resources,s.configuration,s.preview,a.spec AS app_spec,a.revision AS app_revision,a.lifecycle AS app_lifecycle,a.observed_uid AS app_observed_uid,h.cluster_id,h.namespace";
const joins =
  "FROM webdock_auth.hosting_plugin_provisioning p JOIN webdock_auth.hosting_setup s ON s.id=p.setup_id JOIN webdock_auth.hosting_app a ON a.id=s.resources->>'appID' AND a.project_id=p.project_id JOIN webdock_auth.hosting_project h ON h.project_id=p.project_id";

export function assertTemplateApproval(
  stored: unknown,
  approved: unknown,
): void {
  if (!isDeepStrictEqual(stored, approved))
    throw new HostingError(
      409,
      "Approved template changed; retain and review the existing instance.",
    );
}

export const templateClaimSQL = `SELECT ${fields} ${joins} WHERE h.cluster_id=$1 AND ((p.template_state->'migration'->>'status'='queued' OR (p.template_state->'migration'->>'status'='running' AND COALESCE((p.template_state->'migration'->>'leaseUntil')::timestamptz,'-infinity')<=now())) OR (p.template_state->'publication'->>'status'='queued' OR (p.template_state->'publication'->>'status'='running' AND COALESCE((p.template_state->'publication'->>'leaseUntil')::timestamptz,'-infinity')<=now()))) ORDER BY p.updated_at,p.id FOR UPDATE OF p SKIP LOCKED LIMIT 20`;

export function managedCMSTemplate(raw: unknown): {
  templates: ApprovedAppTemplate[];
  hooks: ApprovedAppTemplateHooks;
} {
  const approved = approvalSchema.parse(raw);
  const { name: _, ...configuration } = setupConfigurationSchema.parse({
    name: "Managed CMS",
    source: { kind: "image", reference: approved.image },
    spec: {
      args: [],
      port: 3000,
      healthPath: "/api/health",
      cpuMillicores: 500,
      memoryBytes: 1073741824,
      ephemeralBytes: 268435456,
      volumeBytes: 2147483648,
      replicas: 1,
    },
    database: {
      mode: "new",
      name: "CMS",
      planRevisionID: approved.planRevisionID,
      variable: "DATABASE_URL",
    },
    mail: null,
    autoPublish: false,
  });
  const template: ApprovedAppTemplate = {
    id: "managed-cms",
    configuration,
    async environment(scope, id, name) {
      const saved = state(
        await updateTemplateState(id, async (current) => {
          if (current.secrets) {
            assertTemplateApproval(state(current).approval, approved);
            return current;
          }
          const ids = await transaction(
            async (db) =>
              (
                await db.query(
                  "SELECT webdock_auth.next_snowflake() AS instance,webdock_auth.next_snowflake() AS client",
                )
              ).rows[0],
          );
          const origin = `https://${ids.instance}.${approved.domain}`;
          const oauthSecret = randomBytes(32).toString("base64url");
          return {
            ...current,
            instanceID: ids.instance,
            clientID: ids.client,
            origin,
            approval: approved,
            oauthSecretHash: createHash("sha256")
              .update(oauthSecret)
              .digest("base64url"),
            secrets: (await import("../platform/platform")).encryptMailSecret(
              {
                WEBDOCK_AUTH_ISSUER: approved.authIssuer,
                WEBDOCK_SSO_APP_ORIGIN: origin,
                WEBDOCK_SSO_CLIENT_ID: ids.client,
                WEBDOCK_SSO_CLIENT_SECRET: oauthSecret,
                WEBDOCK_SSO_COOKIE_SECRET:
                  randomBytes(32).toString("base64url"),
                WEBDOCK_SSO_ENFORCE: "true",
                CMS_INSTANCE_ID: ids.instance,
                CMS_PROJECT_ID: scope.projectID,
                CMS_ORIGIN: origin,
                CMS_SITE_NAME: name,
                PAYLOAD_SECRET: randomBytes(32).toString("base64url"),
                CMS_BRIDGE_SECRET: randomBytes(32).toString("base64url"),
              },
              purpose(id),
            ),
          };
        }),
      );
      return Object.entries(await environment(id, saved)).map(
        ([name, value]) => ({ name, value }),
      );
    },
  };
  const enqueue = async (
    context: TemplateHookContext,
    phase: Operation["phase"],
  ) =>
    transaction(async (db) => {
      const { record, setup } = context;
      const row = (
        await db.query(
          `SELECT ${fields} ${joins} WHERE p.id=$1 FOR UPDATE OF p`,
          [record.id],
        )
      ).rows[0];
      if (
        !row ||
        !(
          await db.query(
            "SELECT webdock_rls.plugin_provisioning_live($1) AS allowed",
            [record.id],
          )
        ).rows[0]?.allowed
      )
        throw new HostingError(
          403,
          "Application provisioning authority changed.",
        );
      const s = state(row.template_state);
      assertTemplateApproval(s.approval, approved);
      if (s[phase]?.status === "needs-reconciliation")
        throw new HostingError(
          409,
          "Application initialization needs operator reconciliation; retained resources will not be recreated.",
        );
      if (!s[phase]) {
        await authorizeHosting(
          context.actor,
          {
            permission: "org:projects:manage",
            projectID: record.projectID,
            write: true,
            live: true,
          },
          db,
        );
        if (phase === "migration") {
          if (row.app_lifecycle !== "draft" || row.app_observed_uid)
            throw new HostingError(
              409,
              "Initial storage allocation requires an undeployed application.",
            );
          await planLock(db, record.customerID);
          await clusterLock(db, row.cluster_id);
          const demand = appDemand({
            ...setup.configuration.spec,
            template: "custom",
            image: approved.image,
          });
          await checkDemand(db, {
            customerID: record.customerID,
            projectID: record.projectID,
            clusterID: row.cluster_id,
            provider: "k3s",
            demand,
            subscriptionRevision: setup.preview!.subscriptionRevision,
          });
          const operation = (
            await db.query(
              "INSERT INTO webdock_auth.hosting_operation(subject,customer_id,project_id,cluster_id,app_id,action,idempotency_key,payload_hash,status) VALUES($1,$2,$3,$4,$5,'templates.initialize',$6,$6,'succeeded') RETURNING id",
              [
                record.actor.subject,
                record.customerID,
                record.projectID,
                row.cluster_id,
                setup.resources.appID,
                "template-" + record.id,
              ],
            )
          ).rows[0];
          await db.query(
            "INSERT INTO webdock_auth.hosting_reservation(operation_id,customer_id,project_id,cluster_id,app_id,demand,status) VALUES($1,$2,$3,$4,$5,$6,'reserved')",
            [
              operation.id,
              record.customerID,
              record.projectID,
              row.cluster_id,
              setup.resources.appID,
              JSON.stringify(demand),
            ],
          );
          s.reservationID = operation.id;
        }
        s[phase] = {
          phase,
          status: "queued",
          generation: 0,
          ...(phase === "migration"
            ? { storageState: "unallocated" as const }
            : {}),
        };
        await db.query(
          "UPDATE webdock_auth.hosting_plugin_provisioning SET template_state=$2,updated_at=now() WHERE id=$1",
          [record.id, JSON.stringify(s)],
        );
      }
      return s;
    });
  return {
    templates: [template],
    hooks: {
      async beforeRelease(context) {
        const s = await enqueue(context, "migration");
        return { ready: s.migration?.status === "succeeded" };
      },
      async publish(context) {
        const s = await enqueue(context, "publication");
        if (s.publication?.status !== "succeeded") return null;
        await transaction((db) =>
          db.query("SELECT webdock_auth.publish_plugin_template($1)", [
            context.record.id,
          ]),
        );
        return {
          appID: context.setup.resources.appID!,
          websiteBindingID: s.instanceID,
          origin: s.origin,
          secretReference: purpose(context.record.id),
          pinnedAddresses: s.approval.addresses,
        };
      },
    },
  };
}

export async function resolveTemplateBridgeSecret(
  reference: string,
): Promise<string> {
  const match = /^plugin-template:([1-9][0-9]{0,18})$/.exec(reference);
  if (!match) throw new HostingError(403, "Invalid instance secret reference.");
  return transaction(async (db) => {
    const row = (
      await db.query(
        "SELECT template_state,binding FROM webdock_auth.hosting_plugin_provisioning WHERE id=$1",
        [match[1]],
      )
    ).rows[0];
    if (!row?.binding || row.binding.secretReference !== reference)
      throw new HostingError(403, "Instance binding unavailable.");
    return (await environment(match[1], state(row.template_state)))
      .CMS_BRIDGE_SECRET;
  });
}

const identity = z.object({
  operationID: z.string().regex(/^[1-9][0-9]{0,18}$/),
  generation: z.number().int().positive(),
  leaseToken: z.string().regex(/^[A-Za-z0-9_-]{43}$/),
  phase: z.enum(["migration", "publication"]),
});
export function createTemplateAgentRequest(dependencies: {
  transaction: typeof transaction;
  verifyAgent: typeof verifyAgent;
  assertProvisioningActor: typeof assertProvisioningActor;
  environment: typeof environment;
}) {
  return async function templateAgentRequest(
    agent: Agent,
    action: string,
    input: unknown,
  ): Promise<unknown> {
    return dependencies.transaction(async (db) => {
      await dependencies.verifyAgent(db, agent);
      if (action === "template-claim") {
        z.object({}).strict().parse(input);
        const rows = (await db.query(templateClaimSQL, [agent.clusterID])).rows;
        for (const row of rows) {
          const candidateState = state(row.template_state);
          const candidatePhase: Operation["phase"] =
            candidateState.migration?.status === "succeeded"
              ? "publication"
              : "migration";
          const candidate = candidateState[candidatePhase];
          if (
            candidate?.status === "running" &&
            Date.parse(candidate.leaseUntil!) > Date.now()
          )
            continue;
          await db.query("SAVEPOINT template_candidate");
          try {
            if (
              !(
                await db.query(
                  "SELECT webdock_rls.plugin_provisioning_live($1) AS allowed",
                  [row.id],
                )
              ).rows[0]?.allowed
            )
              throw new HostingError(
                403,
                "Application provisioning authority changed.",
              );
            await dependencies.assertProvisioningActor(row.id, db);
            const s = state(row.template_state),
              phase: Operation["phase"] =
                s.migration?.status === "succeeded"
                  ? "publication"
                  : "migration",
              op = s[phase]!;
            if (phase === "migration")
              await dependencies.assertProvisioningActor(
                row.id,
                db,
                "org:databases:manage",
              );
            if (
              op.status === "running" &&
              Date.parse(op.leaseUntil!) > Date.now()
            )
              continue;
            op.status = "running";
            op.generation++;
            op.leaseToken = randomBytes(32).toString("base64url");
            op.leaseUntil = new Date(Date.now() + 180_000).toISOString();
            const database = (
              await db.query(
                "SELECT d.*,b.ca_certificate,b.state AS backend_state,b.endpoint_ready FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id WHERE d.id=$1 AND d.project_id=$2 AND b.cluster_id=$3",
                [row.resources.databaseID, row.project_id, agent.clusterID],
              )
            ).rows[0];
            if (
              !database ||
              database.state !== "ready" ||
              database.backend_state !== "ready" ||
              !database.endpoint_ready ||
              database.credential_revision !==
                database.confirmed_credential_revision
            )
              throw new HostingError(
                409,
                "Database is not ready for initialization.",
              );
            const env = await dependencies.environment(row.id, s);
            if (phase === "migration") {
              const credentials = (
                await import("../platform/platform")
              ).decryptMailSecret<{ migrationPassword: string }>(
                database.confirmed_credentials_encrypted,
                `database:${database.id}:${database.confirmed_credential_revision}`,
              );
              const url = new URL(
                `postgresql://postgres-rw.wd-db-${database.backend_id}.svc.cluster.local:5432/${database.database_name}`,
              );
              url.username = "wd_migrate_" + database.id;
              url.password = credentials.migrationPassword;
              url.searchParams.set("sslmode", "verify-full");
              Object.assign(env, {
                CMS_MIGRATING: "true",
                CMS_MIGRATION_DATABASE_URL: url.href,
                DATABASE_CA_CERT: database.ca_certificate,
                CMS_BOOTSTRAP_SUBJECT: row.actor.subject,
                CMS_BOOTSTRAP_ROLE: "admin",
              });
            }
            const reservations = (
              await db.query(
                "SELECT demand,status FROM webdock_auth.hosting_reservation WHERE project_id=$1 AND status<>'released'",
                [row.project_id],
              )
            ).rows;
            await db.query(
              "UPDATE webdock_auth.hosting_plugin_provisioning SET template_state=$2,updated_at=now() WHERE id=$1",
              [row.id, JSON.stringify(s)],
            );
            return {
              version: 1,
              templateID: row.template_id,
              operationID: row.id,
              generation: op.generation,
              leaseToken: op.leaseToken,
              leaseUntil: op.leaseUntil,
              phase,
              projectID: row.project_id,
              clusterID: agent.clusterID,
              appID: row.resources.appID,
              instanceID: s.instanceID,
              namespace: row.namespace,
              revision: row.app_revision,
              spec: { ...row.configuration.spec, image: s.approval.image },
              quota: allocationTotals(reservations),
              environment: env,
              backendID: database.backend_id,
              origin: s.origin,
              pinnedAddresses: s.approval.addresses,
              jobUID: op.jobUID ?? null,
              certificateUID: op.certificateUID ?? null,
              ingressUID: op.ingressUID ?? null,
              intentRecorded: op.intentRecorded ?? false,
              storageState: op.storageState ?? "unknown",
            };
          } catch (error) {
            await db.query("ROLLBACK TO SAVEPOINT template_candidate");
            const reason =
              error instanceof HostingError &&
              ![401, 403].includes(error.status)
                ? "initialization_unavailable"
                : "authority_changed";
            await db.query(
              "SELECT webdock_auth.block_template_operation($1,$2,$3)",
              [row.id, candidatePhase, reason],
            );
          } finally {
            await db.query("RELEASE SAVEPOINT template_candidate");
          }
        }
        return null;
      }
      const parsed = identity
        .extend(
          action === "template-checkpoint"
            ? {
                jobUID: z.string().min(1).max(128).optional(),
                certificateUID: z.string().min(1).max(128).optional(),
                ingressUID: z.string().min(1).max(128).optional(),
                intentRecorded: z.literal(true).optional(),
                migrationSucceeded: z.literal(true).optional(),
                storageState: z.enum(["allocating", "ready"]).optional(),
              }
            : {
                outcome: z.enum([
                  "pending",
                  "succeeded",
                  "needs-reconciliation",
                ]),
                proof: z.record(z.string(), z.unknown()).optional(),
              },
        )
        .strict()
        .parse(input);
      const row = (
        await db.query(
          `SELECT ${fields} ${joins} WHERE p.id=$1 AND h.cluster_id=$2 FOR UPDATE OF p`,
          [parsed.operationID, agent.clusterID],
        )
      ).rows[0];
      if (
        !row ||
        !(
          await db.query(
            "SELECT webdock_rls.plugin_provisioning_live($1) AS allowed",
            [parsed.operationID],
          )
        ).rows[0]?.allowed
      )
        throw new HostingError(
          403,
          "Application provisioning authority changed.",
        );
      await dependencies.assertProvisioningActor(row.id, db);
      if (parsed.phase === "migration")
        await dependencies.assertProvisioningActor(
          row.id,
          db,
          "org:databases:manage",
        );
      const s = state(row.template_state),
        op = s[parsed.phase];
      if (
        !op ||
        op.status !== "running" ||
        op.generation !== parsed.generation ||
        op.leaseToken !== parsed.leaseToken ||
        Date.parse(op.leaseUntil!) <= Date.now()
      )
        throw new HostingError(409, "Template operation lease changed.");
      if (action === "template-checkpoint") {
        const value = parsed as z.infer<typeof identity> & {
          jobUID?: string;
          certificateUID?: string;
          ingressUID?: string;
          intentRecorded?: boolean;
          migrationSucceeded?: boolean;
          storageState?: "allocating" | "ready";
        };
        for (const key of ["certificateUID", "ingressUID"] as const) {
          if (value[key]) {
            if (
              parsed.phase !== "publication" ||
              (op[key] && op[key] !== value[key])
            )
              throw new HostingError(409, "Publication resource UID changed.");
            op[key] = value[key];
          }
        }
        if (value.jobUID && op.jobUID && value.jobUID !== op.jobUID)
          throw new HostingError(409, "Initialization Job identity changed.");
        if (value.storageState) {
          if (
            parsed.phase !== "migration" ||
            (value.storageState === "allocating" &&
              op.storageState !== "unallocated" &&
              op.storageState !== "allocating")
          )
            throw new HostingError(
              409,
              "Storage allocation intent cannot be reset.",
            );
          op.storageState = value.storageState;
        }
        if (value.jobUID) op.jobUID = value.jobUID;
        if (value.intentRecorded) op.intentRecorded = true;
        if (value.migrationSucceeded) op.migrationSucceeded = true;
      } else {
        const value = parsed as z.infer<typeof identity> & {
          outcome: Operation["status"] | "pending";
          proof?: Record<string, unknown>;
        };
        if (value.outcome === "succeeded") {
          const proof = value.proof;
          if (
            !proof ||
            proof.phase !== parsed.phase ||
            proof.instanceID !== s.instanceID ||
            (parsed.phase === "migration"
              ? !op.migrationSucceeded || proof.jobUID !== op.jobUID
              : !op.certificateUID ||
                !op.ingressUID ||
                proof.certificateUID !== op.certificateUID ||
                proof.ingressUID !== op.ingressUID ||
                proof.origin !== s.origin ||
                JSON.stringify(proof.pinnedAddresses) !==
                  JSON.stringify(s.approval.addresses))
          )
            throw new HostingError(
              409,
              "Initialization proof differs from the approved instance.",
            );
          op.proof = proof;
          if (parsed.phase === "migration")
            await db.query(
              "UPDATE webdock_auth.hosting_reservation SET status='active' WHERE operation_id=$1 AND app_id=$2",
              [s.reservationID, row.resources.appID],
            );
        }
        op.status =
          value.outcome === "pending"
            ? "queued"
            : (value.outcome as Operation["status"]);
      }
      await db.query(
        "UPDATE webdock_auth.hosting_plugin_provisioning SET template_state=$2,updated_at=now() WHERE id=$1",
        [row.id, JSON.stringify(s)],
      );
      return { ok: true };
    });
  };
}
export const templateAgentRequest = createTemplateAgentRequest({
  transaction,
  verifyAgent,
  assertProvisioningActor,
  environment,
});

type BindingScope = { pluginID: string; projectID: string; customerID: string };
export async function resolveApprovedTemplateHTTP(
  scope: BindingScope,
  name: string,
): Promise<
  | (import("@webdock/core/plugin-http").HTTPBinding & {
      websiteBindingID: string;
    })
  | null
> {
  if (name !== "cms" || scope.pluginID !== "managed-cms") return null;
  const { resolvePluginAppBinding } =
    await import("./plugin-provisioning-store");
  const binding = await resolvePluginAppBinding(scope, "managed-cms");
  if (!binding) return null;
  return {
    origin: binding.origin,
    addresses: binding.pinnedAddresses,
    headers: {
      Authorization:
        "Bearer " +
        (await resolveTemplateBridgeSecret(binding.secretReference)),
    },
    websiteBindingID: binding.websiteBindingID,
    ...(binding.ca ? { ca: binding.ca } : {}),
    rules: [
      {
        method: "GET",
        path: "/api/webdock/cms",
        query: ["request", "transfer", "offset"],
      },
      { method: "POST", path: "/api/webdock/cms", query: [] },
    ],
  };
}
export async function resolveApprovedTemplateApps(
  scope: BindingScope,
): Promise<import("@webdock/core/plugin-apps").ManagedAppBinding[]> {
  if (scope.pluginID !== "managed-cms") return [];
  const { resolvePluginAppBinding } =
    await import("./plugin-provisioning-store");
  const binding = await resolvePluginAppBinding(scope, "managed-cms");
  return binding
    ? [{ appID: binding.appID, kind: "managed-cms", origin: binding.origin }]
    : [];
}
