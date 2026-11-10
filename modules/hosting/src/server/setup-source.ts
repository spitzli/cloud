import type { DatabaseServices } from "@spitzli/database-contracts/runtime-ports";
import type { HostingPorts } from "./ports";
import { inspectRepositoryFiles } from "./setup-inspection";
import {
  HostingError,
  setupCommandSchema,
  type HostingActor,
} from "@spitzli/hosting-contracts";
import { limits } from "./allowances";
export type Dependencies = {
  setupProductAvailable: HostingPorts["setupProductAvailable"];
  setupBuildCapabilities: ReturnType<
    typeof import("./setup-capabilities").createHostingSetupCapabilities
  >["setupBuildCapabilities"];
  readSetup: ReturnType<
    typeof import("./project-setup").createHostingProjectSetup
  >["readSetup"];
  createGitHubProvider: HostingPorts["createGitHubProvider"];
  transaction: ReturnType<typeof import("./db").createHostingDb>["transaction"];
  authorizeHosting: ReturnType<
    typeof import("./authorization").createHostingAuthorization
  >["authorizeHosting"];
  executeGitDeployment: ReturnType<
    typeof import("./git-deployments").createHostingGitDeployments
  >["executeGitDeployment"];
  executeDatabaseCommand: DatabaseServices["executeDatabaseCommand"];
  env: HostingPorts["env"];
};

export function createHostingSetupSource(dependencies: Dependencies) {
  async function executeSetupSource(actor: HostingActor, raw: unknown) {
    const cmd = setupCommandSchema.parse(raw);
    if (cmd.action === "setup.inspectSource") {
      const saved = await dependencies.readSetup(
        actor,
        cmd.projectID,
        cmd.setupID,
      );
      const source = saved.configuration.source;
      if (saved.revision !== cmd.revision)
        throw new HostingError(
          409,
          "Setup changed. Review the current configuration.",
        );
      if (source.kind !== "git" || !source.connectionID || !source.repositoryID)
        throw new HostingError(
          409,
          "Connect an authorized GitHub repository first.",
        );
      const connection = await dependencies.transaction(async (db) => {
        await dependencies.authorizeHosting(
          actor,
          { permission: "org:projects:manage", projectID: cmd.projectID },
          db,
        );
        return (
          await db.query(
            "SELECT c.installation_id FROM webdock_auth.git_connection c JOIN webdock_auth.git_repository r ON r.connection_id=c.id WHERE c.id=$1 AND c.customer_id=$2 AND r.repository_id=$3 AND c.state='active'",
            [source.connectionID, saved.customer_id, source.repositoryID],
          )
        ).rows[0];
      });
      if (!connection)
        throw new HostingError(404, "Git deployment is unavailable.");
      const provider = dependencies.createGitHubProvider();
      const input = {
        installationID: connection.installation_id,
        repositoryID: source.repositoryID,
        branch: source.branch,
      };
      const resolved = await provider.resolveSource(input);
      const files: Record<string, string> = {};
      for (const name of ["Dockerfile", "package.json"]) {
        const text = await provider.readSourceFile(
          input,
          resolved.sha,
          source.rootDirectory === "."
            ? name
            : source.rootDirectory + "/" + name,
        );
        if (text !== null) files[name] = text;
      }
      return {
        ...inspectRepositoryFiles(files),
        sourceSHA: resolved.sha,
        rootDirectory: source.rootDirectory,
      };
    }
    if (cmd.action !== "setup.options")
      throw new HostingError(400, "Unknown setup source operation.");
    const context = await dependencies.transaction(async (db) => {
      const access = await dependencies.authorizeHosting(
        actor,
        { permission: "org:projects:manage", projectID: cmd.projectID },
        db,
      );
      const project = (
        await db.query(
          'SELECT p.name,c.name AS "customerName" FROM webdock_admin.projects p JOIN webdock_admin.customers c ON c.id=p.customer_id WHERE p.id=$1',
          [cmd.projectID],
        )
      ).rows[0];
      const clusters = access.operator
        ? (
            await db.query(
              "SELECT id,name,country,region FROM webdock_auth.hosting_cluster WHERE verified AND ownership='platform' ORDER BY name LIMIT 100",
            )
          ).rows
        : [];
      const mail = dependencies.setupProductAvailable("mail")
        ? (
            await db.query(
              'SELECT c.id,c.kind,c.state,d.id AS "domainID",d.domain,d.status FROM webdock_auth.mail_connection c LEFT JOIN webdock_auth.mail_connection_domain b ON b.connection_id=c.id LEFT JOIN webdock_auth.mail_sender_domain d ON d.id=b.domain_id AND d.deleted_at IS NULL WHERE c.customer_id=$1 AND c.deleted_at IS NULL ORDER BY c.id,d.domain',
              [access.customerID],
            )
          ).rows
        : [];
      const applications = (
        await db.query(
          "SELECT id,name,lifecycle,status FROM webdock_auth.hosting_app WHERE project_id=$1 AND status<>'deleted' ORDER BY name,id LIMIT 200",
          [cmd.projectID],
        )
      ).rows;
      return {
        applications,
        builds: await dependencies.setupBuildCapabilities(db, cmd.projectID),
        enabled: dependencies.env.WEBDOCK_SETUP_ENABLED === "true",
        project: {
          ...project,
          projectID: cmd.projectID,
          customerID: access.customerID!,
          configured: Boolean(access.project?.provider),
          provider: access.project?.provider,
          canManage:
            access.canWrite &&
            (access.operator || access.project?.mode === "selfservice"),
          canAssignHosting: access.operator,
        },
        clusters,
        mail,
        allowances: await limits(db, access.customerID!, cmd.projectID, "k3s"),
      };
    });
    const databaseAvailable = dependencies.setupProductAvailable("databases");
    const [connections, databases, plans] = await Promise.all([
      dependencies.executeGitDeployment(actor, {
        action: "git.connections.list",
        customerID: context.project.customerID,
      }),
      databaseAvailable
        ? dependencies.executeDatabaseCommand(actor, {
            action: "databases.list",
            projectID: cmd.projectID,
          })
        : Promise.resolve([]),
      databaseAvailable
        ? dependencies.executeDatabaseCommand(actor, {
            action: "databases.plans.list",
            projectID: cmd.projectID,
          })
        : Promise.resolve([]),
    ]);
    return { ...context, connections: connections.docs, databases, plans };
  }
  return { executeSetupSource };
}
