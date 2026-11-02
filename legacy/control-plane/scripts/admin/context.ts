import { withDatabaseScope } from "@webdock/database-runtime";
import { recordID } from "../../src/modules/registry/contracts";
import type { RegistryActor } from "../../src/modules/integrations/operator";
export function requireApply() {
  if (!process.argv.includes("--apply"))
    throw Error(
      "Pass --apply only for the reviewed registry changes. No email or infrastructure provisioning is implicit.",
    );
}
export async function runOperator(
  run: (actor: RegistryActor) => Promise<void>,
) {
  if (!process.env.NODE_ENV)
    Object.assign(process.env, { NODE_ENV: "production" });
  const subject = recordID.parse(process.env.WEBDOCK_OPERATOR_SUBJECT),
    sessionID = recordID.parse(process.env.WEBDOCK_OPERATOR_SESSION_ID);
  const { databasePools } = await import("../../src/db/runtime");
  const { registryService, closeRegistry } =
    await import("../../src/modules/registry/context");
  try {
    await (
      await import("../../src/db/readiness")
    ).verifyDatabaseReadiness(
      databasePools,
      ["auth", "registry"],
      (await registryService()).raw,
    );
    await withDatabaseScope(
      { kind: "session", subject, sessionID },
      async () => {
        const actor = await (
          await import("../../src/modules/integrations/operator")
        ).currentRegistryActor();
        await run(actor);
      },
    );
  } finally {
    await closeRegistry();
    await databasePools.close();
  }
}
