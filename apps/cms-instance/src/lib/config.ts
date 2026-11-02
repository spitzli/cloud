import { loadMediaStorage } from "./media-config.ts";
export function loadCMSConfig(
  env: Record<string, string | undefined> = process.env,
) {
  const required = (name: string) => {
    const value = env[name];
    if (!value || value.length > 4096)
      throw Error(`Missing or invalid ${name}`);
    return value;
  };
  const instance = required("CMS_INSTANCE_ID"),
    project = required("CMS_PROJECT_ID");
  if (!/^[\w:.-]{1,128}$/.test(instance) || !/^[\w:.-]{1,128}$/.test(project))
    throw Error("Invalid CMS identity");
  const origin = new URL(required("CMS_ORIGIN"));
  if (
    origin.protocol !== "https:" ||
    origin.pathname !== "/" ||
    origin.search ||
    origin.hash ||
    origin.username ||
    origin.password
  )
    throw Error("CMS_ORIGIN must be an exact HTTPS origin");
  const secret = required("PAYLOAD_SECRET"),
    bridgeSecret = required("CMS_BRIDGE_SECRET");
  if (secret.length < 32 || bridgeSecret.length < 32 || secret === bridgeSecret)
    throw Error("CMS secrets must be distinct and at least 32 characters");
  const migrating = env.CMS_MIGRATING === "true";
  const environment = env.CMS_ENVIRONMENT_ID ?? null;
  if (env.CMS_MEDIA_INITIALIZE_STORAGE && !migrating)
    throw Error(
      "Media initialization belongs only in the explicit migration job",
    );
  if (
    !migrating &&
    (env.CMS_MIGRATION_DATABASE_URL || env.PAYLOAD_MIGRATING === "true")
  )
    throw Error("Migration credentials must not be present in runtime");
  const databaseURL = required(
    migrating ? "CMS_MIGRATION_DATABASE_URL" : "DATABASE_URL",
  );
  if (migrating && env.DATABASE_URL)
    throw Error("Migration must receive only migration credentials");
  const database = new URL(databaseURL);
  if (
    !["postgres:", "postgresql:"].includes(database.protocol) ||
    !database.username ||
    !database.password ||
    database.pathname === "/"
  )
    throw Error("Invalid database URL");
  if (env.PAYLOAD_DROP_DATABASE)
    throw Error("Database destruction is forbidden");
  return {
    instance,
    project,
    environment,
    origin: origin.origin,
    siteName: required("CMS_SITE_NAME"),
    secret,
    bridgeSecret,
    databaseURL,
    databaseCA: env.DATABASE_CA_CERT,
    migrating,
    mediaStorage: loadMediaStorage(env, {
      instance,
      project,
      environment,
      migrating,
    }),
    mediaDirectory: "/data/media",
    transferDirectory: "/data/transfers",
  };
}
export type CMSConfig = ReturnType<typeof loadCMSConfig>;
/** No deployment credential is needed while Next compiles server routes. */
export function payloadEnvironment() {
  if (process.env.NEXT_PHASE === "phase-production-build")
    return loadCMSConfig({
      CMS_INSTANCE_ID: "build",
      CMS_PROJECT_ID: "build",
      CMS_ORIGIN: "https://build.invalid",
      CMS_SITE_NAME: "Build",
      PAYLOAD_SECRET: "p".repeat(32),
      CMS_BRIDGE_SECRET: "b".repeat(32),
      DATABASE_URL: "postgresql://build:build@127.0.0.1/build",
    });
  return loadCMSConfig();
}
