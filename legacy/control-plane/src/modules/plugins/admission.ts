import { AsyncLocalStorage } from "node:async_hooks";
import type { Pool, PoolClient } from "pg";
import {
  currentDatabaseScope,
  sqlCommandWords,
} from "@webdock/database-runtime";
import { ModuleError } from "@webdock/module-sdk";
import type { Intent } from "@webdock/plugin-sdk/contracts";
import { PluginStateError } from "./store";
type Connection = Pick<PoolClient, "query">;
export type AdmissionContext = {
  db: Connection;
  revision: string;
  operationID: string | null;
};
export type Continuation = {
  operationID: string;
  authorize: (db: Connection) => Promise<boolean>;
};
type Options = { continuation?: Continuation };
type Transaction = {
  pool: Pick<Pool, "connect">;
  db: Connection;
  scope: string;
  operationID: string | null;
  readOnly: boolean;
  hasFailure: boolean;
  failed?: unknown;
};
const active = new AsyncLocalStorage<Transaction>();
const sharedFence =
  "SELECT pg_advisory_xact_lock_shared(hashtext('webdock-plugin-installation'))";
/** SQL-only boundary: callers persist their existing operation/outbox using this
 * connection and operationID. Provider I/O runs after this transaction commits. */
export function createModuleAdmission(pool: Pick<Pool, "connect">) {
  async function check<T>(
    tx: Transaction,
    id: string,
    intent: Intent,
    run: (context: AdmissionContext) => Promise<T>,
    options: Options,
  ): Promise<T> {
    if (
      !/^[a-z][a-z0-9-]{0,63}$/.test(id) ||
      !["read", "write", "provision", "continue"].includes(intent)
    )
      throw new PluginStateError(
        400,
        "invalid_admission",
        "Invalid module admission",
      );
    if (tx.scope !== JSON.stringify(currentDatabaseScope()))
      throw new PluginStateError(
        403,
        "admission_scope_changed",
        "Database authority changed during admission",
      );
    if (tx.readOnly && intent !== "read")
      throw new PluginStateError(
        409,
        "admission_read_only",
        "Cannot turn a read transaction into mutating work",
      );
    if (
      !(
        await tx.db.query("SELECT singleton FROM webdock_auth.plugin_bootstrap")
      ).rows.length
    )
      throw new PluginStateError(
        503,
        "plugin_setup_required",
        "Initialize plugin installation state offline before enabling live control",
      );
    const state = (
      await tx.db.query(
        "SELECT state,kind,revision::text FROM webdock_auth.plugin_installation WHERE id=$1",
        [id],
      )
    ).rows[0];
    if (!state || state.state === "disabled")
      throw new ModuleError("module_disabled", id);
    if (intent === "continue") {
      const proof = options.continuation;
      if (
        !proof ||
        !/^[1-9][0-9]{0,18}$/.test(proof.operationID) ||
        !(await proof.authorize(tx.db))
      )
        throw new PluginStateError(
          403,
          "continuation_denied",
          "An authorized accepted operation is required",
        );
      if (tx.operationID && tx.operationID !== proof.operationID)
        throw new PluginStateError(
          409,
          "operation_mismatch",
          "Nested admission must retain the accepted operation",
        );
      tx.operationID = proof.operationID;
    } else {
      if (options.continuation)
        throw new PluginStateError(
          400,
          "invalid_admission",
          "Continuation authority cannot admit new work",
        );
      if (
        state.state === "draining" &&
        (state.kind === "external" || intent !== "read")
      )
        throw new ModuleError("module_draining", id);
      if (intent !== "read" && !tx.operationID)
        tx.operationID = (
          await tx.db.query("SELECT webdock_auth.next_snowflake() AS id")
        ).rows[0].id;
    }
    return run({
      db: tx.db,
      revision: state.revision,
      operationID: tx.operationID,
    });
  }
  return async function withModuleAdmission<T>(
    id: string,
    intent: Intent,
    run: (context: AdmissionContext) => Promise<T>,
    options: Options = {},
  ): Promise<T> {
    const inherited = active.getStore();
    if (inherited) {
      try {
        if (inherited.pool !== pool)
          throw new PluginStateError(
            409,
            "admission_pool_changed",
            "Nested admission must share its database transaction",
          );
        return await check(inherited, id, intent, run, options);
      } catch (error) {
        if (!inherited.hasFailure) inherited.failed = error;
        inherited.hasFailure = true;
        throw error;
      }
    }
    const scope = currentDatabaseScope();
    if (!scope)
      throw new PluginStateError(
        403,
        "admission_scope_required",
        "Verified database authority is required",
      );
    const c = await pool.connect();
    let accepting = true;
    try {
      await c.query("BEGIN");
      if (intent === "read") await c.query("SET TRANSACTION READ ONLY");
      await c.query(
        "SELECT set_config('lock_timeout','5s',true),set_config('statement_timeout','5s',true),set_config('transaction_timeout','5s',true)",
      );
      await c.query(sharedFence);
      const tx: Transaction = {
        pool,
        db: { query: c.query.bind(c) },
        scope: JSON.stringify(scope),
        operationID: null,
        readOnly: intent === "read",
        hasFailure: false,
      };
      tx.db.query = ((...args: unknown[]) => {
        try {
          if (!accepting)
            throw new PluginStateError(
              409,
              "admission_closed",
              "Admission transaction has ended",
            );
          const input = args[0];
          const sql =
            typeof input === "string"
              ? input
              : (input as { text?: unknown })?.text;
          if (typeof sql !== "string")
            throw new PluginStateError(
              400,
              "invalid_admission_query",
              "SQL query text required",
            );
          const words = sqlCommandWords(sql);
          const rollbackTo =
            words[0] === "ROLLBACK" &&
            (words[1] === "TO" ||
              (["WORK", "TRANSACTION"].includes(words[1]) &&
                words[2] === "TO"));
          if (
            [
              "BEGIN",
              "START",
              "COMMIT",
              "END",
              "ABORT",
              "ROLLBACK",
              "PREPARE",
            ].includes(words[0]) &&
            !rollbackTo
          )
            throw new PluginStateError(
              409,
              "admission_transaction_control",
              "Admission owns transaction boundaries",
            );
          return Reflect.apply(c.query, c, args);
        } catch (error) {
          if (!tx.hasFailure) tx.failed = error;
          tx.hasFailure = true;
          throw error;
        }
      }) as Connection["query"];
      const result = await active.run(tx, () =>
        check(tx, id, intent, run, options),
      );
      accepting = false;
      if (tx.hasFailure) throw tx.failed;
      const committed = await c.query("COMMIT");
      if (committed.command !== "COMMIT")
        throw new PluginStateError(
          409,
          "admission_rolled_back",
          "Durable admission did not commit",
        );
      return result;
    } catch (error) {
      try {
        await c.query("ROLLBACK");
      } catch {
        /* Preserve the original database/transaction failure. */
      }
      throw error;
    } finally {
      accepting = false;
      c.release();
    }
  };
}
export async function withModuleAdmission<T>(
  id: string,
  intent: Intent,
  run: (context: AdmissionContext) => Promise<T>,
  options: Options = {},
): Promise<T> {
  const { database } = await import("../../db/runtime");
  return createModuleAdmission(database)(id, intent, run, options);
}
