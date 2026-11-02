import { loadCMSConfig } from "./lib/config.ts";
import { verifiedPostgresConfig } from "@webdock/database-runtime";
import { projectBridgeUser } from "@webdock/cms-bridge/auth";
import type { BridgeRole } from "@webdock/cms-bridge/types";
import pg from "pg";
import { verifyMediaBinding } from "./lib/media-storage.ts";
if (process.env.CMS_MIGRATING !== "true")
  throw Error("Run only in the bounded CMS migration job.");
const environment = loadCMSConfig();
process.env.PAYLOAD_MIGRATING = "true";
const pool = new pg.Pool({
  ...verifiedPostgresConfig(environment.databaseURL, environment.databaseCA),
  max: 1,
});
const client = await pool.connect();
let payload: import("payload").Payload | undefined;
try {
  const result = await client.query<{ locked: boolean }>(
    "SELECT pg_try_advisory_lock(hashtext('webdock-cms-schema-v1')) AS locked",
  );
  if (!result.rows[0]?.locked)
    throw Error("A CMS migration is already running.");
  await client.query("CREATE SCHEMA IF NOT EXISTS app");
  const { getPayload } = await import("payload");
  const config = (await import("./payload.config.ts")).default;
  const { migrations } = await import("./migrations/index.ts");
  payload = await getPayload({ config, disableOnInit: true });
  const migration = await payload.db.migrate({
    migrations: migrations as never,
    shouldPrompt: false,
  });
  if (migration?.cancelled)
    throw Error("Migration requires explicit reconciliation.");
  if (process.env.CMS_MEDIA_INITIALIZE_STORAGE === "true")
    await verifyMediaBinding(environment, true);
  const subject = process.env.CMS_BOOTSTRAP_SUBJECT,
    role = process.env.CMS_BOOTSTRAP_ROLE ?? "admin";
  if (subject) {
    if (
      !/^[\x21-\x7e]{1,256}$/.test(subject) ||
      !["reader", "editor", "admin", "operator"].includes(role)
    )
      throw Error("Invalid bootstrap identity.");
    await projectBridgeUser(payload, {
      instance: environment.instance,
      project: environment.project,
      subject,
      role: role as BridgeRole,
      grantEpoch: "bootstrap",
    });
  }
  console.log(
    JSON.stringify({
      status: "migrated",
      instance: environment.instance,
      schema: "app",
      version: "1",
    }),
  );
} finally {
  if (payload) await payload.destroy();
  await client
    .query("SELECT pg_advisory_unlock(hashtext('webdock-cms-schema-v1'))")
    .catch(() => {});
  client.release();
  await pool.end();
}
