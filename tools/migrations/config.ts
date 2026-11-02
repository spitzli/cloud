import { databaseURL } from "../../legacy/control-plane/src/db/client";
export const runtimeRoles = [
  "webdock_auth_bootstrap",
  "webdock_auth_runtime",
  "webdock_worker_runtime",
  "webdock_admin_runtime",
  "webdock_registry_runtime",
  "webdock_runtime",
  "webdock_browser_website",
  "webdock_cms_archive_runtime",
] as const;
export const ownerRoles = [
  "webdock_platform_owner",
  "webdock_policy_owner",
  "webdock_website_owner",
] as const;
export function migrationConfig(
  env: Record<string, string | undefined>,
  target: string | undefined,
) {
  if (!env.DATABASE_URL_UNPOOLED)
    throw Error("Explicit offline DATABASE_URL_UNPOOLED required");
  const url = databaseURL(env.DATABASE_URL_UNPOOLED);
  const actual = `${url.hostname}:${url.port || 5432}${url.pathname}`;
  if (target !== actual)
    throw Error("Pass the exact migration host:port/database");
  const local = ["127.0.0.1", "localhost"].includes(url.hostname);
  if (!local && !env.DATABASE_CA_CERT)
    throw Error("Remote migrations require a verified CA");
  if (
    !env.DATABASE_CONTEXT_SECRET ||
    Buffer.byteLength(env.DATABASE_CONTEXT_SECRET) < 32
  )
    throw Error("Context signing secret must contain at least 32 bytes");
  if (
    env.WEBDOCK_BOOTSTRAP_EMPTY_DATABASE !== undefined &&
    !["true", "false"].includes(env.WEBDOCK_BOOTSTRAP_EMPTY_DATABASE)
  )
    throw Error("Invalid bootstrap mode");
  const bootstrap = env.WEBDOCK_BOOTSTRAP_EMPTY_DATABASE === "true";
  if (
    !bootstrap &&
    decodeURIComponent(url.username) !== "webdock_offline_migrate"
  )
    throw Error("Use the dedicated offline migrator for upgrades");
  let passwords: Record<string, string> = {};
  if (bootstrap) {
    try {
      passwords = JSON.parse(env.MIGRATION_ROLE_PASSWORDS || "");
    } catch {
      throw Error("Bootstrap requires private MIGRATION_ROLE_PASSWORDS JSON");
    }
    const roles = [...runtimeRoles, "webdock_offline_migrate"];
    if (
      !passwords ||
      typeof passwords !== "object" ||
      Array.isArray(passwords) ||
      Object.keys(passwords).length !== roles.length ||
      roles.some(
        (role) =>
          typeof passwords[role] !== "string" ||
          !/^\S{24,256}$/.test(passwords[role]),
      )
    )
      throw Error(
        "Supply strong passwords for each runtime role and offline migrator",
      );
    if (new Set(roles.map((role) => passwords[role])).size !== roles.length)
      throw Error(
        "Each runtime role and offline migrator require distinct passwords",
      );
  }
  return {
    url,
    local,
    bootstrap,
    passwords,
    contextSecret: env.DATABASE_CONTEXT_SECRET,
    ca: env.DATABASE_CA_CERT,
  };
}
