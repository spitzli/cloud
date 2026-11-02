import { createHash } from "node:crypto";
import { PluginError } from "@webdock/plugin-sdk/contracts";
import type { ManagedAppsScope } from "@webdock/core/plugin-apps";
import type { Connection } from "./authorization";
import type {
  ProvisioningRecord,
  ProvisioningStore,
  ProvisionedAppBinding,
  ProvisioningStatusView,
} from "./plugin-provisioning";
const fields = `id,plugin_id AS "pluginID",customer_id AS "customerID",project_id AS "projectID",template_id AS "templateID",name,actor,plugin_generation AS "pluginGeneration",grant_revision AS "grantRevision",setup_id AS "setupID",binding,template_state AS "templateState"`;
type Transaction = <T>(run: (db: Connection) => Promise<T>) => Promise<T>;
export function createProvisioningStore(transaction: Transaction) {
  const store = {
    async assertLive(
      scope: Pick<
        ManagedAppsScope,
        | "pluginID"
        | "customerID"
        | "projectID"
        | "pluginGeneration"
        | "grantRevision"
      >,
      capability:
        "plugin.apps.read" | "plugin.apps.provision" = "plugin.apps.provision",
    ) {
      if (capability === "plugin.apps.read") {
        const allowed = await transaction(
          async (db) =>
            (
              await db.query(
                "SELECT webdock_rls.plugin_allows($1,$2,$3,'plugin.apps.read') AS allowed",
                [scope.pluginID, scope.customerID, scope.projectID],
              )
            ).rows[0]?.allowed,
        );
        if (!allowed)
          throw new PluginError(
            "permission_denied",
            "Application access changed",
          );
        return;
      }
      const allowed = await transaction(
        async (db) =>
          (
            await db.query(
              "SELECT webdock_rls.plugin_provisioning_scope($1,$2,$3,$4,$5) AS allowed",
              [
                scope.pluginID,
                scope.customerID,
                scope.projectID,
                scope.pluginGeneration ?? null,
                scope.grantRevision ?? null,
              ],
            )
          ).rows[0]?.allowed,
      );
      if (!allowed)
        throw new PluginError(
          "permission_denied",
          "Application provisioning authority changed",
        );
    },
    async reserve(
      scope: ManagedAppsScope,
      input: { template_id: string; name: string; request_id: string },
    ): Promise<ProvisioningRecord> {
      return transaction(async (db) => {
        await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
          `plugin-provision:${scope.pluginID}:${scope.projectID}`,
        ]);
        const hash = createHash("sha256")
          .update(
            JSON.stringify({
              template_id: input.template_id,
              name: input.name,
            }),
          )
          .digest("hex");
        const receipt = (
          await db.query(
            "SELECT request_hash FROM webdock_auth.hosting_plugin_provisioning_request WHERE plugin_id=$1 AND project_id=$2 AND request_id=$3",
            [scope.pluginID, scope.projectID, input.request_id],
          )
        ).rows[0];
        if (receipt && receipt.request_hash !== hash)
          throw new PluginError(
            "conflict",
            "Provisioning request has different configuration",
          );
        const actor = {
          subject: scope.subject,
          sessionID: scope.sessionID,
          source: "studio",
          scopes: ["hosting:read", "hosting:write"],
          ...(scope.consoleIdentity
            ? { consoleIdentity: scope.consoleIdentity }
            : {}),
        };
        let record = (
          await db.query(
            `SELECT ${fields} FROM webdock_auth.hosting_plugin_provisioning WHERE plugin_id=$1 AND project_id=$2 AND template_id=$3 FOR UPDATE`,
            [scope.pluginID, scope.projectID, input.template_id],
          )
        ).rows[0] as ProvisioningRecord | undefined;
        if (!record)
          record = (
            await db.query(
              `INSERT INTO webdock_auth.hosting_plugin_provisioning(plugin_id,customer_id,project_id,template_id,name,actor,plugin_generation,grant_revision) VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING ${fields}`,
              [
                scope.pluginID,
                scope.customerID,
                scope.projectID,
                input.template_id,
                input.name,
                JSON.stringify(actor),
                scope.pluginGeneration,
                scope.grantRevision,
              ],
            )
          ).rows[0];
        if (!record || record.customerID !== scope.customerID)
          throw new PluginError(
            "permission_denied",
            "Provisioning is unavailable",
          );
        const provisioningActor = { ...actor, pluginProvisioningID: record.id };
        // A fresh explicit request reauthorizes interrupted work, never allocates a second app.
        await db.query(
          "UPDATE webdock_auth.hosting_plugin_provisioning SET actor=$2,plugin_generation=$3,grant_revision=$4,updated_at=now() WHERE id=$1",
          [
            record.id,
            JSON.stringify(provisioningActor),
            scope.pluginGeneration,
            scope.grantRevision,
          ],
        );
        record = {
          ...record,
          actor: provisioningActor as ProvisioningRecord["actor"],
          pluginGeneration: scope.pluginGeneration!,
          grantRevision: scope.grantRevision!,
        };
        await db.query(
          "INSERT INTO webdock_auth.hosting_plugin_provisioning_request(plugin_id,customer_id,project_id,request_id,request_hash,provisioning_id) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(plugin_id,project_id,request_id) DO NOTHING",
          [
            scope.pluginID,
            scope.customerID,
            scope.projectID,
            input.request_id,
            hash,
            record.id,
          ],
        );
        return record;
      });
    },
    async status(
      record: ProvisioningRecord,
    ): Promise<ProvisioningStatusView | undefined> {
      return transaction(
        async (db) =>
          (
            await db.query(
              "SELECT webdock_rls.plugin_provisioning_status($1) AS status",
              [record.id],
            )
          ).rows[0]?.status ?? undefined,
      );
    },
    async list(
      scope: Pick<ManagedAppsScope, "pluginID" | "customerID" | "projectID">,
    ): Promise<ProvisioningRecord[]> {
      return transaction(
        async (db) =>
          (
            await db.query(
              `SELECT ${fields} FROM webdock_auth.hosting_plugin_provisioning WHERE plugin_id=$1 AND customer_id=$2 AND project_id=$3 ORDER BY created_at,id LIMIT 20`,
              [scope.pluginID, scope.customerID, scope.projectID],
            )
          ).rows,
      );
    },
    async get(
      scope: Pick<ManagedAppsScope, "pluginID" | "customerID" | "projectID">,
      id: string,
    ): Promise<ProvisioningRecord | null> {
      return transaction(
        async (db) =>
          (
            await db.query(
              `SELECT ${fields} FROM webdock_auth.hosting_plugin_provisioning WHERE id=$1 AND plugin_id=$2 AND customer_id=$3 AND project_id=$4`,
              [id, scope.pluginID, scope.customerID, scope.projectID],
            )
          ).rows[0] ?? null,
      );
    },
    async attachSetup(record: ProvisioningRecord, setupID: string) {
      await transaction(async (db) => {
        const result = await db.query(
          "UPDATE webdock_auth.hosting_plugin_provisioning SET setup_id=$2,updated_at=now() WHERE id=$1 AND (setup_id IS NULL OR setup_id=$2) AND EXISTS(SELECT 1 FROM webdock_auth.hosting_setup s WHERE s.id=$2 AND s.project_id=$3 AND s.customer_id=$4)",
          [record.id, setupID, record.projectID, record.customerID],
        );
        if (!result.rowCount)
          throw new PluginError("conflict", "Provisioning setup changed");
      });
    },
    async bySetup(setupID: string): Promise<ProvisioningRecord | null> {
      return transaction(
        async (db) =>
          (
            await db.query(
              `SELECT ${fields} FROM webdock_auth.hosting_plugin_provisioning WHERE setup_id=$1`,
              [setupID],
            )
          ).rows[0] ?? null,
      );
    },
    async updateTemplateState(
      id: string,
      update: (
        current: Record<string, unknown>,
      ) => Promise<Record<string, unknown>>,
    ) {
      return transaction(async (db) => {
        const row = (
          await db.query(
            "SELECT template_state FROM webdock_auth.hosting_plugin_provisioning WHERE id=$1 FOR UPDATE",
            [id],
          )
        ).rows[0];
        if (!row)
          throw new PluginError("not_found", "Provisioning is unavailable");
        const state = await update(row.template_state);
        await db.query(
          "UPDATE webdock_auth.hosting_plugin_provisioning SET template_state=$2,updated_at=now() WHERE id=$1",
          [id, JSON.stringify(state)],
        );
        return state;
      });
    },
    async setBinding(
      record: ProvisioningRecord,
      binding: ProvisionedAppBinding,
    ) {
      await transaction(async (db) => {
        const result = await db.query(
          "UPDATE webdock_auth.hosting_plugin_provisioning SET binding=$2,updated_at=now() WHERE id=$1 AND plugin_generation=$3 AND grant_revision=$4 AND (binding IS NULL OR binding=$2::jsonb)",
          [
            record.id,
            JSON.stringify(binding),
            record.pluginGeneration,
            record.grantRevision,
          ],
        );
        if (!result.rowCount)
          throw new PluginError("conflict", "Provisioning binding changed");
      });
    },
    async resolve(
      scope: Pick<ManagedAppsScope, "pluginID" | "customerID" | "projectID">,
      templateID: string,
    ): Promise<ProvisionedAppBinding | null> {
      return transaction(
        async (db) =>
          (
            await db.query(
              "SELECT p.binding FROM webdock_auth.hosting_plugin_provisioning p WHERE p.plugin_id=$1 AND p.customer_id=$2 AND p.project_id=$3 AND p.template_id=$4 AND webdock_rls.plugin_provisioning_status(p.id)->>'state'='ready'",
              [scope.pluginID, scope.customerID, scope.projectID, templateID],
            )
          ).rows[0]?.binding ?? null,
      );
    },
  } satisfies ProvisioningStore & Record<string, unknown>;
  return store;
}
export const pluginProvisioningStore = createProvisioningStore(async (run) =>
  (await import("./db")).transaction(run),
);
export const updateTemplateState = pluginProvisioningStore.updateTemplateState;
export const getProvisioningBySetup = pluginProvisioningStore.bySetup;
export const assertProvisioningLive = pluginProvisioningStore.assertLive;
export const resolvePluginAppBinding = pluginProvisioningStore.resolve;

export async function assertProvisioningActor(
  id: string,
  db: Connection,
  permission = "org:projects:manage",
): Promise<void> {
  const mapping = (
    await db.query(
      "SELECT webdock_rls.plugin_provisioning_actor($1) AS mapping",
      [id],
    )
  ).rows[0]?.mapping;
  if (!mapping)
    throw new PluginError(
      "permission_denied",
      "Application provisioning identity changed",
    );
  const reference = mapping.actor.consoleIdentity;
  if (reference) {
    const { revalidateStoredConsoleIdentity } =
      await import("../auth/clerk-provider");
    const principal = await revalidateStoredConsoleIdentity(reference, {
      identity: async () => mapping.identity,
      organization: async () => mapping.organization,
    });
    if (
      principal.userID !== mapping.actor.subject ||
      principal.sessionID !== mapping.actor.sessionID ||
      (!principal.operator &&
        (principal.customerID !== mapping.customerID ||
          !(principal.permissions ?? []).some((value) => value === permission)))
    )
      throw new PluginError(
        "permission_denied",
        "Application provisioning identity changed",
      );
  } else {
    const { consoleAuthConfiguration } =
      await import("../auth/console-configuration");
    if (
      consoleAuthConfiguration().requireExistingMapping ||
      mapping.legacyAllowed !== true
    )
      throw new PluginError(
        "permission_denied",
        "Confirm application provisioning with the current identity",
      );
  }
}
export async function pluginOperationAuthority(
  db: Connection,
  id: string,
): Promise<{ required: boolean; allowed: boolean }> {
  const row = (
    await db.query(
      "SELECT webdock_rls.plugin_setup_operation_binding($1) AS binding_id, webdock_rls.plugin_setup_operation_live($1) AS allowed,(SELECT action FROM webdock_auth.hosting_operation WHERE id=$1) AS action",
      [id],
    )
  ).rows[0];
  if (row?.binding_id && row.allowed === true) {
    try {
      await assertProvisioningActor(
        row.binding_id,
        db,
        row.action?.startsWith("databases.")
          ? "org:databases:manage"
          : "org:projects:manage",
      );
    } catch {
      return { required: true, allowed: false };
    }
  }
  return { required: !!row?.binding_id, allowed: row?.allowed === true };
}
