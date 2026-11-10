/** Unknown actions deliberately have no write capability. */
export function gitActionPermission(action: string): OrgPermission | undefined {
  switch (action) {
    case "git.connections.list":
    case "git.connections.disconnect":
    case "git.repositories.list":
    case "git.sources.list":
    case "git.source.get":
    case "git.source.configure":
    case "git.targets.bind":
      return "org:projects:manage";
    case "git.builds.list":
    case "git.builds.get":
    case "git.builds.request":
    case "git.builds.cancel":
    case "git.releases.list":
    case "git.releases.approve":
    case "git.releases.rollback":
    case "git.releases.reconcile":
      return "org:deployments:create";
  }
}

type OrgPermission = string;
import type {
  HostingActor,
  SetupConfiguration,
} from "@spitzli/hosting-contracts";
import type { Connection } from "./authorization";

export function createHostingPermissions(dependencies: {
  authorizeHosting: ReturnType<
    typeof import("./authorization").createHostingAuthorization
  >["authorizeHosting"];
}) {
  const { authorizeHosting } = dependencies;
  async function authorizeSetupConfiguration(
    db: Connection,
    actor: HostingActor,
    projectID: string,
    configuration: SetupConfiguration,
  ) {
    const permissions: OrgPermission[] = ["org:projects:manage"];
    if (configuration.database.mode !== "none")
      permissions.push("org:databases:manage");
    if (configuration.source.kind === "git")
      permissions.push("org:deployments:create");
    for (const permission of permissions)
      await authorizeHosting(actor, { projectID, write: true, permission }, db);
  }

  return { authorizeSetupConfiguration };
}
