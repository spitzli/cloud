import type { DatabaseServices } from "@spitzli/database-contracts/runtime-ports";
import { assertSetupProducts } from "./setup-admission";
import { assertSetupConfirmation } from "./setup-confirmation";
export { assertSetupConfirmation } from "./setup-confirmation";
import type { HostingPorts } from "./ports";
import { randomUUID } from "node:crypto";
import {
  HostingError,
  appSpecSchema,
  healthcheckImage,
  resolveAppSpec,
  type HostingActor,
  type SetupConfiguration,
  type SetupPreview,
} from "@spitzli/hosting-contracts";
import { limits } from "./allowances";
import { appDemand } from "@spitzli/hosting-contracts";
export type Dependencies = {
  authorizeSetupConfiguration: ReturnType<
    typeof import("./permissions").createHostingPermissions
  >["authorizeSetupConfiguration"];
  requireDatabases: HostingPorts["requireDatabases"];
  requireMail: HostingPorts["requireMail"];
  applicationCapabilities: ReturnType<
    typeof import("./setup-capabilities").createHostingSetupCapabilities
  >["applicationCapabilities"];
  setupBuildCapabilities: ReturnType<
    typeof import("./setup-capabilities").createHostingSetupCapabilities
  >["setupBuildCapabilities"];
  transaction: ReturnType<typeof import("./db").createHostingDb>["transaction"];
  authorizeHosting: ReturnType<
    typeof import("./authorization").createHostingAuthorization
  >["authorizeHosting"];
  checkDemand: ReturnType<
    typeof import("./reservations").createHostingReservations
  >["checkDemand"];
  executeDatabaseCommand: DatabaseServices["executeDatabaseCommand"];
  createGitHubProvider: HostingPorts["createGitHubProvider"];
  applicationMailRoute: ReturnType<
    typeof import("./mail-binding").createHostingMailBinding
  >["applicationMailRoute"];
  env: HostingPorts["env"];
};

export function createHostingSetupPreview(dependencies: Dependencies) {
  async function previewProjectSetup(
    actor: HostingActor,
    input: {
      id: string;
      projectID: string;
      revision: number;
      configuration: SetupConfiguration;
      environmentNames?: string[];
      databaseID?: string;
      appID?: string;
    },
  ): Promise<SetupPreview> {
    const config = input.configuration;
    assertSetupProducts(dependencies, config, {
      databaseID: input.databaseID,
    });
    const result: SetupPreview = {
      id: randomUUID(),
      revision: input.revision,
      expiresAt: new Date(Date.now() + 15 * 60000).toISOString(),
      sourceSHA: null,
      image: null,
      subscriptionRevision: 0,
      blockers: [],
      offers: [],
    };
    const managedNames = [
      ...(config.database.mode === "none" ? [] : [config.database.variable]),
      ...(config.mail ? [config.mail.variable, "WEBDOCK_MAIL_API_URL"] : []),
    ];
    if (
      new Set(managedNames).size !== managedNames.length ||
      managedNames.some((name) => input.environmentNames?.includes(name))
    )
      result.blockers.push(
        "Choose distinct service variables and remove conflicting manual variables before deploying.",
      );
    await dependencies.transaction(async (db) => {
      await dependencies.authorizeSetupConfiguration(
        db,
        actor,
        input.projectID,
        config,
      );
      const access = await dependencies.authorizeHosting(
        actor,
        {
          permission: "org:projects:manage",
          projectID: input.projectID,
          write: true,
        },
        db,
      );
      if (
        (
          await db.query(
            "SELECT id FROM webdock_auth.hosting_app WHERE project_id=$1 AND name=$2 AND status<>'deleted' AND ($3::text IS NULL OR id<>$3) LIMIT 1",
            [input.projectID, config.name, input.appID ?? null],
          )
        ).rowCount
      )
        result.blockers.push("An application already uses this name.");
      const cap = await limits(db, access.customerID!, input.projectID, "k3s");
      result.subscriptionRevision = cap.subscriptionRevision;
      if (!access.project?.cluster_id || access.project.provider !== "k3s") {
        result.blockers.push(
          "Assign managed container hosting before setting up this application.",
        );
        return;
      }
      if (dependencies.env.WEBDOCK_SETUP_ENABLED !== "true")
        result.blockers.push(
          "Enable the project setup worker before deploying.",
        );
      try {
        await dependencies.checkDemand(db, {
          customerID: access.customerID!,
          projectID: input.projectID,
          clusterID: access.project.cluster_id,
          provider: "k3s",
          demand: appDemand({
            ...config.spec,
            template: "custom",
            image: healthcheckImage,
          }),
          subscriptionRevision: cap.subscriptionRevision,
        });
      } catch (error) {
        result.blockers.push(
          error instanceof HostingError
            ? error.message
            : "Hosting capacity could not be verified.",
        );
      }
      const agent = {
        capabilities: await dependencies.applicationCapabilities(
          db,
          access.project.cluster_id,
        ),
      };
      if (
        (config.database.mode !== "none" || config.mail) &&
        agent?.capabilities?.serviceBindingsVersion !== 1
      )
        result.blockers.push(
          "Update the cluster agent before connecting services.",
        );
      if (
        config.source.kind !== "template" &&
        !access.operator &&
        !access.project.own_images
      )
        result.blockers.push("Custom images are not enabled for this project.");
      if (
        config.database.mode === "new" &&
        !input.databaseID &&
        dependencies.env.WEBDOCK_DATABASE_CLUSTER_ID !==
          access.project.cluster_id
      )
        result.blockers.push(
          "Managed databases must use the same cluster as this application.",
        );
      if (config.source.kind === "image") {
        if (
          !appSpecSchema.shape.image.safeParse(config.source.reference).success
        )
          result.blockers.push(
            "Use an immutable image digest supported by this cluster.",
          );
        else result.image = config.source.reference;
      } else if (config.source.kind === "template")
        result.image = resolveAppSpec({
          ...config.spec,
          template: "healthcheck",
          image: healthcheckImage,
        }).image;
      else {
        const connection = (
          await db.query(
            "SELECT c.installation_id FROM webdock_auth.git_connection c JOIN webdock_auth.git_repository r ON r.connection_id=c.id WHERE c.customer_id=$1 AND c.id=$2 AND r.repository_id=$3 AND c.state='active'",
            [
              access.customerID,
              config.source.connectionID,
              config.source.repositoryID,
            ],
          )
        ).rows[0];
        if (!connection)
          result.blockers.push(
            "Connect an authorized GitHub repository first.",
          );
        else
          try {
            result.sourceSHA = (
              await dependencies.createGitHubProvider().resolveSource({
                installationID: connection.installation_id,
                repositoryID: config.source.repositoryID!,
                branch: config.source.branch,
              })
            ).sha;
          } catch {
            result.blockers.push(
              "GitHub source could not be verified. Reconnect or try again.",
            );
          }
        const builds = await dependencies.setupBuildCapabilities(
          db,
          input.projectID,
        );
        if (
          config.source.buildProvider === "isolated"
            ? !builds.isolated
            : !builds.githubActions
        )
          result.blockers.push(
            "Configure verified build capacity before deploying from Git.",
          );
        if (
          !dependencies.env.WEBDOCK_GIT_REGISTRY_HOST ||
          agent?.capabilities?.registryVersion !== 1
        )
          result.blockers.push(
            "Configure verified artifact and registry storage.",
          );
        if (config.source.dockerfile !== "Dockerfile")
          result.blockers.push(
            "This build worker requires Dockerfile in the selected root directory.",
          );
      }
      if (config.database.mode === "existing") {
        const d = (
          await db.query(
            "SELECT d.state,b.cluster_id FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_backend b ON b.id=d.backend_id WHERE d.id=$1 AND d.project_id=$2 AND d.customer_id=$3",
            [config.database.databaseID, input.projectID, access.customerID],
          )
        ).rows[0];
        if (
          !d ||
          d.cluster_id !== access.project.cluster_id ||
          d.state !== "ready"
        )
          result.blockers.push(
            "Choose a ready database in the same project and cluster.",
          );
      }
      if (config.mail) {
        try {
          dependencies.applicationMailRoute();
        } catch (error) {
          result.blockers.push((error as HostingError).message);
        }
        const domain = (
          await db.query(
            "SELECT 1 FROM webdock_auth.mail_connection c JOIN webdock_auth.mail_connection_domain b ON b.connection_id=c.id JOIN webdock_auth.mail_sender_domain d ON d.id=b.domain_id WHERE c.id=$1 AND c.customer_id=$2 AND c.state='ready' AND c.deleted_at IS NULL AND d.id=$3 AND d.customer_id=c.customer_id AND d.status='ownership_verified' AND d.deleted_at IS NULL",
            [config.mail.connectionID, access.customerID, config.mail.domainID],
          )
        ).rowCount;
        if (!domain)
          result.blockers.push(
            "Verify a native Mail connection and sender domain first.",
          );
      }
    });
    if (config.database.mode === "new" && !input.databaseID)
      try {
        const offer = await dependencies.executeDatabaseCommand(actor, {
          action: "databases.offers.create",
          projectID: input.projectID,
          planRevisionID: config.database.planRevisionID,
          name: config.database.name,
          idempotencyKey: "setup-offer-" + result.id,
        });
        result.offers.push({
          id: offer.id,
          name: offer.name,
          monthlyPriceCents: offer.snapshot.monthlyPriceCents,
          currency: offer.snapshot.currency,
          terms: offer.snapshot.terms,
          expiresAt: offer.expiresAt,
          backupStatus: offer.snapshot.backupStatus,
        });
        result.expiresAt = new Date(
          Math.min(
            new Date(result.expiresAt).getTime(),
            new Date(offer.expiresAt).getTime(),
          ),
        ).toISOString();
      } catch (error) {
        result.blockers.push(
          error instanceof HostingError
            ? error.message
            : "Database offer is unavailable.",
        );
      }
    return result;
  }
  return { previewProjectSetup, assertSetupConfirmation };
}
