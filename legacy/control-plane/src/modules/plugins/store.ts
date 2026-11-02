import { z } from "zod";
import type { Pool, PoolClient } from "pg";
import { currentDatabaseScope } from "@webdock/database-runtime";
import { dependencyBlockers } from "@webdock/module-sdk/plugins";
import type {
  InstalledPlugin,
  SessionActor,
  Availability,
  Mutation,
  OperationRef,
} from "@webdock/plugin-sdk/contracts";
export class PluginStateError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
  ) {
    super(message);
  }
}
export const pluginFence =
  "SELECT pg_advisory_xact_lock(hashtext('webdock-plugin-installation'))";
export async function readInstalledPlugins(
  db: Pick<Pool, "query"> | Pick<PoolClient, "query">,
): Promise<InstalledPlugin[]> {
  if (
    !(await db.query("SELECT singleton FROM webdock_auth.plugin_bootstrap"))
      .rows.length
  )
    throw new PluginStateError(
      503,
      "plugin_setup_required",
      "Initialize plugin installation state offline before enabling live control",
    );
  return (
    await db.query(
      `SELECT id,kind,revision::text,desired_state AS "desiredState",state,version,image_digest AS "imageDigest",runtime,blockers,dependencies FROM webdock_auth.plugin_installation ORDER BY id`,
    )
  ).rows;
}
const commandSchema = z.strictObject({
  id: z.string().regex(/^[a-z][a-z0-9-]{0,63}$/),
  desired: z.enum(["enabled", "draining", "disabled"]),
  expectedRevision: z.string().regex(/^(0|[1-9][0-9]{0,18})$/),
  idempotencyKey: z.string().min(1).max(128),
});
export async function requestPluginState(
  db: Pick<Pool, "connect">,
  actor: SessionActor,
  id: string,
  desired: Availability,
  mutation: Mutation,
): Promise<OperationRef> {
  commandSchema.parse({ id, desired, ...mutation });
  const scope = currentDatabaseScope();
  if (
    scope?.kind !== "session" ||
    scope.subject !== actor.subject ||
    scope.sessionID !== actor.sessionID
  )
    throw new PluginStateError(
      403,
      "permission_denied",
      "A verified operator session is required",
    );
  const c = await db.connect();
  try {
    await c.query("BEGIN");
    await c.query(
      "SELECT set_config('lock_timeout','5s',true),set_config('statement_timeout','5s',true)",
    );
    await c.query(pluginFence);
    if (
      !(await c.query("SELECT webdock_rls.operator() AS allowed")).rows[0]
        ?.allowed
    )
      throw new PluginStateError(
        403,
        "permission_denied",
        "Current operator authority is required",
      );
    const payload = {
      id,
      desired,
      expectedRevision: mutation.expectedRevision,
    };
    const prior = (
      await c.query(
        "SELECT id,status,payload FROM webdock_auth.plugin_operation WHERE actor_id=$1 AND idempotency_key=$2",
        [actor.subject, mutation.idempotencyKey],
      )
    ).rows[0];
    if (prior) {
      if (
        prior.payload.id !== id ||
        prior.payload.desired !== desired ||
        prior.payload.expectedRevision !== mutation.expectedRevision
      )
        throw new PluginStateError(
          409,
          "idempotency_conflict",
          "Key already used with a different command",
        );
      await c.query("COMMIT");
      return { id: prior.id, status: prior.status };
    }
    if (
      (
        await c.query(
          "SELECT id FROM webdock_auth.plugin_operation WHERE plugin_id=$1 AND status IN ('pending','running','blocked')",
          [id],
        )
      ).rows.length
    )
      throw new PluginStateError(
        409,
        "operation_pending",
        "Resolve the current lifecycle operation before requesting another transition",
      );
    const installed = await readInstalledPlugins(c);
    const current = installed.find((p) => p.id === id);
    if (!current)
      throw new PluginStateError(404, "not_found", "Plugin is not installed");
    if (current.revision !== mutation.expectedRevision)
      throw new PluginStateError(
        409,
        "revision_conflict",
        "Installation changed; review its current revision",
      );
    if (desired !== "enabled" && dependencyBlockers(id, null, installed).length)
      throw new PluginStateError(
        409,
        "active_dependent",
        "Disable dependent plugins first",
      );
    const changed = (
      await c.query(
        `UPDATE webdock_auth.plugin_installation SET desired_state=$2,state=CASE WHEN $2 IN ('disabled','draining') AND state<>'disabled' THEN 'draining' ELSE state END,revision=revision+1,generation=generation+1,updated_at=now() WHERE id=$1 AND revision=$3 RETURNING generation`,
        [id, desired, mutation.expectedRevision],
      )
    ).rows[0];
    if (!changed)
      throw new PluginStateError(
        409,
        "revision_conflict",
        "Installation changed",
      );
    const operation = (
      await c.query(
        `INSERT INTO webdock_auth.plugin_operation(plugin_id,actor_id,idempotency_key,payload,generation) VALUES($1,$2,$3,$4,$5) RETURNING id,status`,
        [
          id,
          actor.subject,
          mutation.idempotencyKey,
          JSON.stringify(payload),
          changed.generation,
        ],
      )
    ).rows[0];
    await c.query(
      `INSERT INTO webdock_auth.access_event(actor_id,action,target_id,outcome) VALUES($1,'plugin.state.requested',$2,'accepted')`,
      [actor.subject, id],
    );
    await c.query("COMMIT");
    return operation;
  } catch (e) {
    await c.query("ROLLBACK");
    throw e;
  } finally {
    c.release();
  }
}
