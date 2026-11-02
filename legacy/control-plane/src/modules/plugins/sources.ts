import {
  currentDatabaseScope,
  withDatabaseScope,
  type DatabaseScope,
} from "@webdock/database-runtime";
import { database, workerDatabase } from "../../db/runtime";
import { currentRegistryActor } from "../integrations/operator";
import { RegistryError } from "../registry/service";
import { getGitHub } from "../integrations/github/client";
import { browserHeaders } from "../portal/request-context";
import { revalidateStoredConsoleIdentity } from "../auth/clerk-provider";
import { withConsolePrincipal } from "../auth/principal";
import { createPluginSourceService } from "./sources-store";
import { verifySourceRuntime } from "./sources-runtime";
import {
  createVerifiedSourceActorRunner,
  sourceOperatorIdentity,
} from "./sources-identity";
import { createPluginInstallerDispatch } from "./sources-dispatch";
import {
  actionsSourceInput,
  createGitHubActionsVerifier,
} from "./sources-actions";
import {
  installerConfigured,
  runtimeBindingSchema,
  sourceFail,
  type SourceRow,
} from "./sources-policy";
async function currentActor() {
  const actor = await currentRegistryActor().catch((error) => {
      if (error instanceof RegistryError && [401, 403].includes(error.status))
        sourceFail(
          "permission_denied",
          "A current operator session is required.",
          error.status,
        );
      throw error;
    }),
    scope = currentDatabaseScope();
  return sourceOperatorIdentity(actor, scope);
}
const withVerifiedSourceActor = createVerifiedSourceActorRunner({
  revalidate: revalidateStoredConsoleIdentity,
  withPrincipal: withConsolePrincipal,
  authorize: async (projectID, customerID) => {
    await currentActor();
    if (
      !(
        await database.query(
          "SELECT 1 FROM webdock_admin.projects p JOIN webdock_admin.customers c ON c.id=p.customer_id WHERE p.id=$1 AND p.customer_id=$2 AND p.status='active' AND c.status='active' AND NOT EXISTS(SELECT 1 FROM webdock_admin.project_deletion d WHERE d.project_id=p.id AND d.status<>'deleted')",
          [projectID, customerID],
        )
      ).rows.length
    )
      sourceFail(
        "project_unavailable",
        "Select an active project and customer.",
        403,
      );
  },
});
async function verifyActor(
  scope: Extract<DatabaseScope, { kind: "session" }>,
  projectID: string,
  customerID: string,
) {
  try {
    return await withVerifiedSourceActor(
      scope,
      projectID,
      customerID,
      async () => true,
    );
  } catch {
    return false;
  }
}
async function probe(
  row: SourceRow,
  binding: ReturnType<typeof runtimeBindingSchema.parse>,
) {
  return verifySourceRuntime(
    row,
    binding,
    process.env.WEBDOCK_PLUGIN_RUNTIME_FILE!,
  );
}
const settings = {
  token: () => process.env.WEBDOCK_PLUGIN_INSTALLER_TOKEN ?? "",
  configured: () => installerConfigured(),
  actor: currentActor,
  verifyActor,
  probe,
};
const interactive = createPluginSourceService({
  ...settings,
  db: database,
  github: async (source, actorID) => {
    const actor = await currentRegistryActor();
    const { registryID } = sourceOperatorIdentity(
      actor,
      currentDatabaseScope(),
      actorID,
    );
    const github = getGitHub();
    if (!github)
      sourceFail(
        "github_unavailable",
        "Configure and connect GitHub before selecting a repository.",
        503,
      );
    return github.source(
      browserHeaders(),
      registryID,
      {
        installation: source.installation,
        installationPage: source.installationPage,
        page: source.page,
        repository: source.repository,
      },
      source.ref,
    );
  },
});
const installer = createPluginSourceService({
  ...settings,
  db: workerDatabase,
});
const dispatchInstaller = createPluginInstallerDispatch({
  nextReady: installer.nextReady,
  withActor: (job, work) =>
    withVerifiedSourceActor(
      job.actor_scope,
      job.project_id,
      job.customer_id,
      work,
    ),
  activate: (id) => interactive.command({ action: "activate", id }),
  worker: installer.worker,
});
export const pluginSources = (input: unknown) => interactive.command(input);
const verifyActions = createGitHubActionsVerifier();
export async function pluginActionsSource(
  authorization: string | null,
  input: unknown,
) {
  const parsed = actionsSourceInput.safeParse(input);
  if (!parsed.success)
    sourceFail("invalid_input", "Invalid Actions source request.", 400);
  const claims = await verifyActions(authorization, parsed.data);
  return withDatabaseScope(
    { kind: "system", purpose: "plugin-controller" },
    () => installer.actionsSource(parsed.data, claims),
  );
}
export const pluginInstaller = (action: string, input: unknown) =>
  withDatabaseScope({ kind: "system", purpose: "plugin-controller" }, () =>
    dispatchInstaller(action, input),
  );
