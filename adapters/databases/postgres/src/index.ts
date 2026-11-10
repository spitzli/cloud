import { Pool, type PoolConfig } from "pg";
import { drizzle } from "drizzle-orm/node-postgres";
import { ScopedPool, verifiedPostgresConfig } from "@spitzli/database-runtime";
import type { DatabaseConfig } from "@spitzli/database-runtime";
export type { DatabaseConfig } from "@spitzli/database-runtime";
export function databaseURL(raw: string): URL {
  let url: URL;
  try {
    url = new URL(raw);
  } catch {
    throw Error("Invalid database URL");
  }
  if (
    !["postgres:", "postgresql:"].includes(url.protocol) ||
    url.hash ||
    url.pathname.length < 2 ||
    [...url.searchParams.keys()].some((key) => key !== "sslmode")
  )
    throw Error("Database URL cannot override connection parameters");
  return url;
}
export function createDatabases(config: DatabaseConfig) {
  if (!Number.isInteger(config.max) || config.max < 1 || config.max > 4)
    throw Error("Database pool maximum must be between 1 and 4");
  if (
    config.environment === "production" &&
    (!config.signingSecret || !config.authURL || !config.workerURL)
  )
    throw Error("Production requires separate signed database roles");
  const connection = (raw: string): PoolConfig => {
    const url = databaseURL(raw);
    const local = ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
    if (!["postgres:", "postgresql:"].includes(url.protocol))
      throw Error("Invalid database URL");
    if (config.environment === "production" && local)
      throw Error("Production requires verified database TLS");
    if (
      config.environment === "production" &&
      ["postgres", ""].includes(url.username)
    )
      throw Error("Runtime owner credentials are forbidden");
    return {
      ...(local
        ? {
            connectionString: raw,
            options: "-c search_path=webdock_auth,pg_catalog",
          }
        : verifiedPostgresConfig(raw, config.ca)),
      max: config.max,
      application_name: "webdock-api",
      connectionTimeoutMillis: 10_000,
      idleTimeoutMillis: 5_000,
    };
  };
  if (config.environment === "production") {
    for (const [raw, role] of [
      [config.businessURL, "webdock_auth_runtime"],
      [config.authURL, "webdock_auth_bootstrap"],
      [config.workerURL, "webdock_worker_runtime"],
    ]) {
      if (new URL(raw).username !== role)
        throw Error("Unexpected database runtime role");
    }
  }
  const businessConfig = connection(config.businessURL),
    authConfig = connection(config.authURL),
    workerConfig = connection(config.workerURL);
  if (
    config.environment === "production" &&
    new Set(
      [config.businessURL, config.authURL, config.workerURL].map(
        (raw) => new URL(raw).username,
      ),
    ).size !== 3
  )
    throw Error("Database runtime credentials must be distinct");
  const business = new ScopedPool(businessConfig, {
    secret: config.signingSecret,
    audience: "webdock_auth_runtime",
    scope: config.scope,
  });
  const authentication = new Pool(authConfig);
  const worker = new ScopedPool(workerConfig, {
    secret: config.signingSecret,
    audience: "webdock_worker_runtime",
    scope: config.scope,
  });
  return {
    business,
    authentication,
    worker,
    orm: drizzle({ client: business }),
    workerOrm: drizzle({ client: worker }),
    close: async () => {
      await Promise.all([business.end(), authentication.end(), worker.end()]);
    },
  };
}
/** A worker never receives an authentication/bootstrap connection, even through a shared legacy import. */
export function createWorkerDatabases(
  config: Pick<
    DatabaseConfig,
    "environment" | "workerURL" | "signingSecret" | "ca" | "max" | "scope"
  >,
) {
  if (!Number.isInteger(config.max) || config.max < 1 || config.max > 4)
    throw Error("Invalid worker pool budget");
  const url = databaseURL(config.workerURL),
    local = ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname);
  if (
    config.environment === "production" &&
    (!config.signingSecret ||
      local ||
      url.username !== "webdock_worker_runtime")
  )
    throw Error(
      "Worker requires its restricted role and verified database TLS",
    );
  const pool = new ScopedPool(
    {
      ...(local
        ? { connectionString: config.workerURL }
        : verifiedPostgresConfig(config.workerURL, config.ca)),
      max: config.max,
      application_name: "webdock-worker",
      connectionTimeoutMillis: 10_000,
      idleTimeoutMillis: 5000,
    },
    { secret: config.signingSecret, audience: "webdock_worker_runtime", scope: config.scope },
  );
  const orm = drizzle({ client: pool });
  return {
    business: pool,
    authentication: pool,
    worker: pool,
    orm,
    workerOrm: orm,
    close: () => pool.end(),
  };
}
