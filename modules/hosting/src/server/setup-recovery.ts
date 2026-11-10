import type { HostingPorts } from "./ports";
import { isDeepStrictEqual } from "node:util";
import {
  HostingError,
  type SetupConfiguration,
  type EnvironmentPatch,
} from "@spitzli/hosting-contracts";
import type { SetupRow } from "./project-setup";
import type { Connection } from "./authorization";
export type Dependencies = {
  openEnvironment: ReturnType<
    typeof import("./environment").createHostingEnvironment
  >["openEnvironment"];
  patchEnvironment: ReturnType<
    typeof import("./environment").createHostingEnvironment
  >["patchEnvironment"];
  sealEnvironment: ReturnType<
    typeof import("./environment").createHostingEnvironment
  >["sealEnvironment"];
};

export function createHostingSetupRecovery(dependencies: Dependencies) {
  function resetGitSetupRetry(
    row: Pick<SetupRow, "steps" | "resources">,
    releaseState: string | undefined,
  ) {
    if (releaseState === "needs-reconciliation")
      throw new HostingError(
        409,
        "Reconcile the interrupted Git release in Deployments before retrying setup.",
      );
    if (!["failed", "superseded"].includes(releaseState ?? "")) return false;
    delete row.resources.buildID;
    delete row.resources.releaseID;
    const build = row.steps.find((s) => s.name === "build")!;
    build.attempt = (build.attempt ?? 0) + 1;
    let reset = false;
    for (const step of row.steps) {
      if (step === build) reset = true;
      if (reset) {
        step.state = "pending";
        step.reason = null;
        step.operationID = null;
      }
    }
    return true;
  }
  async function repairSetupEnvironment(
    db: Connection,
    row: SetupRow,
    configuration: SetupConfiguration,
    patch: EnvironmentPatch,
  ) {
    if (
      row.state !== "blocked" ||
      !row.steps.some((s) => s.name === "bindings" && s.state === "blocked") ||
      !row.resources.appID ||
      !isDeepStrictEqual(row.configuration, configuration)
    )
      throw new HostingError(
        409,
        "Use application settings for changes after provisioning has started.",
      );
    const app = (
      await db.query(
        "SELECT id,environment_encrypted FROM webdock_auth.hosting_app WHERE id=$1 AND project_id=$2 AND lifecycle='draft' AND status='draft' FOR UPDATE",
        [row.resources.appID, row.project_id],
      )
    ).rows[0];
    if (!app)
      throw new HostingError(
        409,
        "Use application settings for changes after provisioning has started.",
      );
    const bindings = (
      await db.query(
        "SELECT variable,service FROM webdock_auth.hosting_service_binding WHERE app_id=$1 AND state<>'disconnected' FOR SHARE",
        [app.id],
      )
    ).rows;
    const managed = new Set(
      bindings.flatMap((b) =>
        b.service.kind === "mail"
          ? [b.variable, "WEBDOCK_MAIL_API_URL"]
          : [b.variable],
      ),
    );
    if (patch.some((p) => managed.has(p.name)))
      throw new HostingError(
        409,
        "Managed service variables cannot be edited manually.",
      );
    const appEnvironment = dependencies.patchEnvironment(
      dependencies.openEnvironment(app.id, app.environment_encrypted),
      patch,
    );
    const environment = dependencies.patchEnvironment(
      dependencies.openEnvironment(
        "setup:" + row.id,
        row.environment_encrypted,
      ),
      patch,
    );
    await db.query(
      "UPDATE webdock_auth.hosting_app SET environment_encrypted=$2,revision=revision+1 WHERE id=$1",
      [app.id, dependencies.sealEnvironment(app.id, appEnvironment)],
    );
    return (
      await db.query(
        "UPDATE webdock_auth.hosting_setup SET environment_encrypted=$2,revision=revision+1,updated_at=now() WHERE id=$1 RETURNING *",
        [row.id, dependencies.sealEnvironment("setup:" + row.id, environment)],
      )
    ).rows[0] as SetupRow;
  }
  return { resetGitSetupRetry, repairSetupEnvironment };
}
