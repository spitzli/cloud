/** Operator-only activation after a TLS-authenticated readiness probe of the exact installed generation. */
import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { Pool } from "pg";
import { verifiedPostgresConfig } from "@spitzli/database-runtime";
import { pinnedHTTPTransport } from "@spitzli/core/plugin-http";
export async function activatePinnedPlugin(
  env: Record<string, string>,
  id: string,
) {
  const url = new URL(env.DATABASE_URL_UNPOOLED);
  if (url.username !== "webdock_offline_migrate" || !env.DATABASE_CA_CERT)
    throw Error("Offline operator credentials and verified CA required");
  const config = JSON.parse(
    readFileSync(env.WEBDOCK_PLUGIN_RUNTIME_FILE, "utf8"),
  );
  const b = config.runtimes[id];
  if (!b) throw Error("Runtime binding missing");
  const db = new Pool({
    ...verifiedPostgresConfig(url.href, env.DATABASE_CA_CERT),
    max: 1,
  });
  const c = await db.connect();
  try {
    const row = (
      await c.query(
        "SELECT p.version,p.state,p.current_generation,r.image,r.namespace,r.service_name,r.credential_hash,r.configuration_revision,c.revision AS config_revision FROM webdock_auth.plugin_installation p JOIN webdock_auth.plugin_runtime r ON r.plugin_id=p.id AND r.generation=p.current_generation JOIN webdock_auth.plugin_configuration c ON c.plugin_id=p.id WHERE p.id=$1",
        [id],
      )
    ).rows[0];
    if (
      !row ||
      row.state !== "disabled" ||
      Number(row.current_generation) !== b.generation ||
      row.namespace !== b.namespace ||
      row.service_name !== b.service ||
      row.credential_hash !==
        createHash("sha256").update(b.credential).digest("hex") ||
      row.configuration_revision !== row.config_revision
    )
      throw Error("Installed runtime differs from reviewed configuration");
    const health = await pinnedHTTPTransport({
      url: new URL(
        `https://${b.service}.${b.namespace}.svc.cluster.local:3443/healthz`,
      ),
      addresses: b.addresses,
      ca: b.ca,
      headers: { authorization: "Bearer " + b.credential },
      method: "GET",
    });
    const h = health.body as any;
    if (
      health.status !== 200 ||
      h.pluginID !== id ||
      h.version !== row.version ||
      h.generation !== b.generation ||
      h.protocolVersion !== 1
    )
      throw Error("Runtime identity/readiness mismatch");
    await c.query("BEGIN");
    await c.query(
      "SELECT pg_advisory_xact_lock(hashtext('webdock-plugin-installation'))",
    );
    const changed = await c.query(
      "UPDATE webdock_auth.plugin_runtime SET status='ready',observed_at=now() WHERE plugin_id=$1 AND generation=$2 AND credential_hash=$3 AND configuration_revision=$4 AND status='starting'",
      [id, b.generation, row.credential_hash, row.configuration_revision],
    );
    if (changed.rowCount !== 1) throw Error("Runtime changed during readiness");
    const enabled = await c.query(
      "UPDATE webdock_auth.plugin_installation SET state='enabled',desired_state='enabled',runtime='ready',revision=revision+1,updated_at=now() WHERE id=$1 AND state='disabled' AND current_generation=$2 AND version=$3",
      [id, b.generation, row.version],
    );
    if (enabled.rowCount !== 1) throw Error("Installation changed");
    await c.query("COMMIT");
    return {
      id,
      version: row.version,
      generation: b.generation,
      enabled: true,
    };
  } catch (e) {
    await c.query("ROLLBACK");
    throw e;
  } finally {
    c.release();
    await db.end();
  }
}
