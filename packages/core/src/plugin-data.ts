import { createHash } from "node:crypto";
import type { Pool } from "pg";
import {
  PluginError,
  documentKeySchema,
  documentWriteSchema,
  type Capability,
  type PluginDocument,
} from "@spitzli/plugin-sdk/contracts";
export type DocumentScope = {
  pluginID: string;
  customerID: string;
  projectID: string;
  operationID: string;
};
/** Runs in the host's verified invocation + signed RLS context. Never accept these IDs from a plugin body. */
export interface PluginDataAuthority {
  run<T>(
    request: Request,
    capability: Capability,
    work: (scope: DocumentScope, pool: Pick<Pool, "connect">) => Promise<T>,
  ): Promise<T>;
}
const canonical = (v: unknown): unknown =>
  v && typeof v === "object"
    ? Array.isArray(v)
      ? v.map(canonical)
      : Object.fromEntries(
          Object.entries(v)
            .sort(([a], [b]) => a.localeCompare(b))
            .map(([k, x]) => [k, canonical(x)]),
        )
    : v;
export function createPluginData(authority: PluginDataAuthority) {
  async function read(request: Request, key: string): Promise<PluginDocument> {
    if (!documentKeySchema.safeParse(key).success)
      throw new PluginError("invalid_input", "Invalid document key");
    return authority.run(request, "plugin.data.read", async (s, pool) => {
      const c = await pool.connect();
      try {
        const row = (
          await c.query(
            "SELECT value,revision::text FROM webdock_auth.plugin_document WHERE plugin_id=$1 AND customer_id=$2 AND project_id=$3 AND key=$4",
            [s.pluginID, s.customerID, s.projectID, key],
          )
        ).rows[0];
        return row ?? { value: null, revision: null };
      } finally {
        c.release();
      }
    });
  }
  async function write(
    request: Request,
    raw: unknown,
  ): Promise<PluginDocument> {
    const parsed = documentWriteSchema.safeParse(raw);
    if (!parsed.success)
      throw new PluginError("invalid_input", "Invalid document write");
    const input = parsed.data;
    return authority.run(request, "plugin.data.write", async (s, pool) => {
      const c = await pool.connect();
      const scope = [s.pluginID, s.customerID, s.projectID];
      const digest = createHash("sha256")
        .update(JSON.stringify(canonical(input)))
        .digest("hex");
      try {
        await c.query("BEGIN");
        await c.query(
          "SELECT set_config('statement_timeout','5000',true),set_config('lock_timeout','3000',true)",
        );
        await c.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
          JSON.stringify([...scope, input.key]),
        ]);
        await c.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
          JSON.stringify([...scope, s.operationID, input.idempotencyKey]),
        ]);
        const receipt = (
          await c.query(
            "SELECT payload_sha256,result FROM webdock_auth.plugin_document_receipt WHERE plugin_id=$1 AND customer_id=$2 AND project_id=$3 AND operation_id=$4 AND idempotency_key=$5",
            [...scope, s.operationID, input.idempotencyKey],
          )
        ).rows[0];
        if (receipt) {
          if (receipt.payload_sha256 !== digest)
            throw new PluginError(
              "conflict",
              "Idempotency key reused with different data",
            );
          await c.query("COMMIT");
          return receipt.result;
        }
        const current = (
          await c.query(
            "SELECT revision::text FROM webdock_auth.plugin_document WHERE plugin_id=$1 AND customer_id=$2 AND project_id=$3 AND key=$4 FOR UPDATE",
            [...scope, input.key],
          )
        ).rows[0];
        if ((current?.revision ?? null) !== input.expectedRevision)
          throw new PluginError(
            "conflict",
            "Document changed; reload before saving",
          );
        const value = JSON.stringify(input.value);
        const row = current
          ? (
              await c.query(
                "UPDATE webdock_auth.plugin_document SET value=$5::jsonb,revision=revision+1,updated_at=now() WHERE plugin_id=$1 AND customer_id=$2 AND project_id=$3 AND key=$4 RETURNING value,revision::text",
                [...scope, input.key, value],
              )
            ).rows[0]
          : (
              await c.query(
                "INSERT INTO webdock_auth.plugin_document(plugin_id,customer_id,project_id,key,value,revision) VALUES($1,$2,$3,$4,$5::jsonb,1) RETURNING value,revision::text",
                [...scope, input.key, value],
              )
            ).rows[0];
        if (!row)
          throw new PluginError("permission_denied", "Document access denied");
        await c.query(
          "INSERT INTO webdock_auth.plugin_document_receipt(plugin_id,customer_id,project_id,operation_id,idempotency_key,key,payload_sha256,result) VALUES($1,$2,$3,$4,$5,$6,$7,$8::jsonb)",
          [
            ...scope,
            s.operationID,
            input.idempotencyKey,
            input.key,
            digest,
            JSON.stringify(row),
          ],
        );
        await c.query("COMMIT");
        return row;
      } catch (error) {
        await c.query("ROLLBACK");
        throw error;
      } finally {
        c.release();
      }
    });
  }
  async function handle(request: Request): Promise<Response> {
    try {
      const match =
        /^\/v1\/plugin\/data\/([a-zA-Z0-9][a-zA-Z0-9_.-]{0,239})$/.exec(
          new URL(request.url).pathname,
        );
      if (match && request.method === "GET")
        return Response.json(await read(request, match[1]));
      if (
        new URL(request.url).pathname === "/v1/plugin/data" &&
        request.method === "PUT"
      ) {
        const chunks: Uint8Array[] = [];
        let size = 0;
        const reader = request.body?.getReader();
        if (reader)
          try {
            for (;;) {
              const { done, value } = await reader.read();
              if (done) break;
              size += value.length;
              if (size > 70 * 1024)
                throw new PluginError("invalid_input", "Request too large");
              chunks.push(value);
            }
          } finally {
            await reader.cancel();
          }
        let body;
        try {
          body = JSON.parse(Buffer.concat(chunks).toString());
        } catch {
          throw new PluginError("invalid_input", "Invalid JSON");
        }
        return Response.json(await write(request, body));
      }
      return Response.json(
        { error: { code: "not_found", message: "Route not found" } },
        { status: 404 },
      );
    } catch (e) {
      const error =
        e instanceof PluginError
          ? e
          : new PluginError(
              "unavailable",
              "Document service unavailable",
              true,
            );
      return Response.json(
        { error: error.toJSON() },
        {
          status:
            error.code === "conflict"
              ? 409
              : error.code === "permission_denied"
                ? 403
                : error.code === "invalid_input"
                  ? 400
                  : 503,
        },
      );
    }
  }
  return { read, write, handle };
}
