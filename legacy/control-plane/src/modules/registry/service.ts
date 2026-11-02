import { readAuthProviderConfiguration } from "@webdock/auth-configuration";
import { signedDatabaseScope } from "../../db/signed-scope";
import { retainedRegistryInventory } from "./extensions";
import { readCachedView } from "../../cache";
import { databaseURL } from "../../db/client";
import {
  ScopedPool,
  currentDatabaseScope,
  withDatabaseScope,
  verifiedPostgresConfig,
} from "@webdock/database-runtime";
import { drizzle } from "drizzle-orm/node-postgres";
import { createRegistryService } from "@webdock/core/registry/service";
export { RegistryError } from "@webdock/core/registry/errors";
export type { RegistryIdentity } from "@webdock/core/registry/ports";
export async function createRegistry() {
  const raw = process.env.REGISTRY_DATABASE_URL;
  if (!raw) throw Error("Registry database role is required");
  const url = databaseURL(raw),
    local =
      ["127.0.0.1", "localhost"].includes(url.hostname) &&
      process.env.NODE_ENV !== "production";
  if (!local && url.username !== "webdock_registry_runtime")
    throw Error("Signed registry runtime role is required");
  const maximum = Number(process.env.REGISTRY_POOL_MAX || (local ? 1 : 0));
  if (!Number.isInteger(maximum) || maximum < 1 || maximum > 4)
    throw Error("Registry pool maximum must be between 1 and 4");
  if (
    !process.env.DATABASE_RLS_SECRET ||
    Buffer.byteLength(process.env.DATABASE_RLS_SECRET) < 32
  )
    throw Error("Registry requires signed RLS context");
  const pool = new ScopedPool(
    {
      ...(local
        ? { connectionString: raw }
        : verifiedPostgresConfig(raw, process.env.DATABASE_CA_CERT)),
      max: maximum,
      application_name: "webdock-api-registry",
    },
    {
      secret: process.env.DATABASE_RLS_SECRET,
      audience: "webdock_registry_runtime",
      scope: signedDatabaseScope,
    },
  );
  const service = createRegistryService({
    database: drizzle({ client: pool }),
    pool,
    readCachedView,
    withSessionScope: async (actor, run) => {
      const authProvider = readAuthProviderConfiguration(process.env.WEBDOCK_AUTH_PROVIDER).provider;
      const inherited = currentDatabaseScope();
      const verified = inherited?.kind === "session" && inherited.subject === actor.subject && inherited.sessionID === actor.sessionID && (inherited.clerk || inherited.memberships);
      const memberships = authProvider === "clerk" && !verified
        ? await (await import("../auth/clerk-provider")).legacyClerkMemberships(actor.subject)
        : undefined;
      return withDatabaseScope(
        { kind: "session", authProvider, subject: actor.subject, sessionID: actor.sessionID,
          ...(memberships ? {memberships:{...memberships,subject:memberships.subject ?? actor.subject,sessionID:memberships.sessionID ?? actor.sessionID}} : {}) },
        run,
      );
    },
    beforeArchive: retainedRegistryInventory,
    close: () => pool.end(),
  });
  return {
    ...service,
    // Kept for source compatibility; CMS registration is no longer an active API.
    writeInstance: async (
      _actor: import("@webdock/core/registry/ports").RegistryIdentity,
      _key: string | null,
      _input: unknown,
      _confirmed: boolean,
    ): Promise<
      typeof import("@webdock/platform-db/schema/registry").cmsInstances.$inferSelect
    > => {
      throw new (await import("@webdock/core/registry/errors")).RegistryError(
        "CMS registration is unavailable.",
        404,
      );
    },
  };
}
