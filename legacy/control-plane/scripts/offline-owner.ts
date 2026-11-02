import { readFileSync, statSync } from "node:fs";
import { parseEnv } from "node:util";
import { Pool } from "pg";
import { verifiedPostgresConfig } from "@webdock/database-runtime";
import { databaseURL } from "../src/db/client";
export async function offlineOwner(
  file: string | undefined,
  target: string | undefined,
) {
  if (!file || statSync(file).mode & 0o077)
    throw Error("Use a private offline migration environment file");
  const env = parseEnv(readFileSync(file, "utf8")),
    url = databaseURL(env.DATABASE_URL_UNPOOLED || "");
  if (target !== `${url.hostname}:${url.port || 5432}${url.pathname}`)
    throw Error("Pass the exact host:port/database target");
  const local =
    url.hostname === "127.0.0.1" &&
    ["/webdock_api_test", "/webdock_rls_test"].includes(url.pathname);
  if (
    !local &&
    ([
      "postgres",
      "webdock_auth_runtime",
      "webdock_auth_bootstrap",
      "webdock_worker_runtime",
      "webdock_registry_runtime",
      "webdock_admin_runtime",
    ].includes(url.username) ||
      !env.DATABASE_CA_CERT)
  )
    throw Error("Use a dedicated offline migrator and verified CA");
  const pool = new Pool({
    ...(local
      ? { connectionString: url.href }
      : verifiedPostgresConfig(url.href, env.DATABASE_CA_CERT)),
    max: 1,
    application_name: "webdock-api-offline",
  });
  if (
    (await pool.query("SELECT current_database() AS name")).rows[0].name !==
    decodeURIComponent(url.pathname.slice(1))
  ) {
    await pool.end();
    throw Error("Connected database differs from target");
  }
  return { pool, url, env };
}
