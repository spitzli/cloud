import { assertModuleCanDisable } from "./module-preflight-check";
import { withDatabaseScope } from "@webdock/database-runtime";
import { assertMailCanDisable } from "@webdock/module-mail/server/lifecycle";
import { modulePreflightChecks } from "./db/module-preflight-tables";
import { installationRegistry } from "./installation";
/** Startup-only check: resource data and encryption keys are never modified. */
export async function verifyModuleShutdown(
  env: Record<string, string | undefined> = process.env,
) {
  const registry = installationRegistry(env);
  const { workerDatabase } = await import("./db/runtime");
  if (registry.state("mail") === "disabled")
    await withDatabaseScope({ kind: "system", purpose: "mail-cron" }, () =>
      assertMailCanDisable(workerDatabase, {
        query: (sql, values) =>
          withDatabaseScope(
            { kind: "system", purpose: "mail-send-gateway" },
            () => workerDatabase.query(sql, values),
          ),
      }),
    );
  for (const [module, checks] of Object.entries(modulePreflightChecks)) {
    if (registry.state(module) !== "disabled") continue;
    await withDatabaseScope(
      { kind: "system", purpose: "installation-preflight" },
      async () => {
        await assertModuleCanDisable(workerDatabase, module, checks);
      },
    );
  }
}
