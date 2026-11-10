import type { DatabaseServices } from "@spitzli/database-contracts/runtime-ports";
import type { HostingPorts } from "./ports";
import { gitActionPermission } from "./permissions";
import {
  HostingError,
  setupSteps,
  commandSchema,
  type SetupProgress,
  type HostingCommand,
} from "@spitzli/hosting-contracts";
import type { SetupRow } from "./project-setup";
export type Dependencies = {
  authorizeProvisioning?(row: SetupRow): Promise<void>;
  beforeRelease?(row: SetupRow): Promise<boolean>;
  beforeReady?(row: SetupRow): Promise<boolean>;
  authorizeSetupConfiguration: ReturnType<
    typeof import("./permissions").createHostingPermissions
  >["authorizeSetupConfiguration"];
  transaction: ReturnType<typeof import("./db").createHostingDb>["transaction"];
  replay: ReturnType<
    typeof import("./operations").createHostingOperations
  >["replay"];
  authorizeHosting: ReturnType<
    typeof import("./authorization").createHostingAuthorization
  >["authorizeHosting"];
  executeAppCommand: ReturnType<
    typeof import("./apps").createHostingApps
  >["executeAppCommand"];
  executeDatabaseCommand: DatabaseServices["executeDatabaseCommand"];
  executeServiceBinding: ReturnType<
    typeof import("./service-bindings").createHostingServiceBindings
  >["executeServiceBinding"];
  executeGitDeployment: ReturnType<
    typeof import("./git-deployments").createHostingGitDeployments
  >["executeGitDeployment"];
  openEnvironment: ReturnType<
    typeof import("./environment").createHostingEnvironment
  >["openEnvironment"];
};

export function createHostingSetupExecution(dependencies: Dependencies) {
  async function executeSetupStep(row: SetupRow): Promise<{
    resources: SetupRow["resources"];
    steps: SetupProgress[];
    ready: boolean;
  }> {
    if (!row.actor || !row.preview)
      throw new HostingError(
        409,
        "Confirm this setup again before continuing.",
      );
    await dependencies.authorizeProvisioning?.(row);
    const actor = row.actor,
      config = row.configuration,
      resources = { ...row.resources };
    await dependencies.transaction((db) =>
      dependencies.authorizeSetupConfiguration(
        db,
        actor,
        row.project_id,
        config,
      ),
    );
    const steps = row.steps.map((s) => ({ ...s }));
    const step = steps.find((s) => !["succeeded", "skipped"].includes(s.state));
    if (!step) return { resources, steps, ready: true };
    step.state = "running";
    step.reason = null;
    const key =
      "setup-" +
      row.id +
      "-" +
      step.name +
      (step.attempt ? "-" + step.attempt : "");
    const done = () => {
      step.state = "succeeded";
    };
    const call = async (raw: HostingCommand): Promise<any> => {
      await dependencies.authorizeProvisioning?.(row);
      const command = commandSchema.parse(raw);
      if ("idempotencyKey" in command && !command.action.startsWith("git.")) {
        const recovered = await dependencies.transaction(async (db) => {
          await dependencies.authorizeHosting(
            actor,
            {
              permission: command.action.startsWith("databases.")
                ? "org:databases:manage"
                : command.action.startsWith("git.")
                  ? gitActionPermission(command.action)
                  : "org:projects:manage",
              projectID: row.project_id,
              write: true,
            },
            db,
          );
          const prior = (
            await db.query(
              "SELECT * FROM webdock_auth.hosting_operation WHERE project_id=$1 AND idempotency_key=$2 ORDER BY created_at LIMIT 1",
              [row.project_id, command.idempotencyKey],
            )
          ).rows[0];
          if (!prior) return null;
          const request = await dependencies.replay(
            db,
            actor,
            command.idempotencyKey,
            command,
          );
          if (
            prior.payload_hash !== request.hash ||
            prior.action !== command.action
          )
            throw new HostingError(
              409,
              "The saved setup operation has different configuration. Review it before retrying.",
            );
          return prior.app_id
            ? {
                id: prior.id,
                appID: prior.app_id,
                revision: prior.target_revision,
                status: prior.status,
              }
            : prior.result;
        });
        if (recovered) return recovered;
      }
      return command.action.startsWith("apps.")
        ? dependencies.executeAppCommand(actor, command)
        : command.action.startsWith("databases.")
          ? dependencies.executeDatabaseCommand(actor, command)
          : command.action.startsWith("services.")
            ? dependencies.executeServiceBinding(actor, command)
            : dependencies.executeGitDeployment(actor, command);
    };
    if (step.name === "application") {
      if (!resources.appID) {
        const created = await call({
          action: "apps.prepare",
          projectID: row.project_id,
          name: config.name,
          spec: config.spec,
          environment: Object.entries(
            dependencies.openEnvironment(
              "setup:" + row.id,
              row.environment_encrypted,
            ),
          ).map(([name, value]) => ({ name, value })),
          subscriptionRevision: row.preview.subscriptionRevision,
          idempotencyKey: key,
        });
        resources.appID = created.appID;
      }
      done();
    } else if (step.name === "database") {
      if (config.database.mode === "none") step.state = "skipped";
      else {
        if (!resources.databaseID)
          resources.databaseID =
            config.database.mode === "existing"
              ? config.database.databaseID
              : (
                  await call({
                    action: "databases.acceptOrder",
                    projectID: row.project_id,
                    offerID: row.preview.offers[0]?.id ?? "",
                    idempotencyKey: key,
                  })
                ).databaseID;
        const database = await call({
          action: "databases.get",
          databaseID: resources.databaseID!,
        });
        if (database.state === "ready") done();
        else if (
          ["degraded", "needs-reconciliation", "retained", "purged"].includes(
            database.state,
          )
        )
          throw new HostingError(
            409,
            "Database provisioning needs attention before setup can continue.",
          );
        else step.reason = "Waiting for database readiness.";
      }
    } else if (step.name === "bindings") {
      const services = [];
      if (config.database.mode !== "none")
        services.push({
          kind: "postgres" as const,
          databaseID: resources.databaseID!,
          variable: config.database.variable,
        });
      if (config.mail) services.push({ kind: "mail" as const, ...config.mail });
      const ids = [];
      for (const service of services) {
        const bound = await call({
          action: "services.bind",
          projectID: row.project_id,
          appID: resources.appID!,
          service,
          revision: 0,
          idempotencyKey: key + "-" + service.kind,
        });
        ids.push(bound.id);
      }
      resources.bindingIDs = ids;
      done();
    } else if (step.name === "source") {
      if (config.source.kind !== "git") step.state = "skipped";
      else {
        const source = await call({
          action: "git.source.get",
          projectID: row.project_id,
          targetID: resources.appID!,
        });
        const configured =
          source.source ??
          (await call({
            action: "git.source.configure",
            projectID: row.project_id,
            targetID: resources.appID!,
            connectionID: config.source.connectionID!,
            repositoryID: config.source.repositoryID!,
            branch: config.source.branch,
            rootDirectory: config.source.rootDirectory,
            recipe: "dockerfile",
            healthPath: config.spec.healthPath,
            revision: 0,
            enabled: true,
            autoPublish: config.autoPublish,
            buildProvider: config.source.buildProvider,
            workflowPath: config.source.workflowPath,
            artifactPrefix: config.source.artifactPrefix,
          }));
        resources.sourceID = configured.id;
        done();
      }
    } else if (step.name === "build") {
      if (config.source.kind !== "git") step.state = "skipped";
      else {
        if (!resources.buildID) {
          if (config.source.buildProvider === "github-actions") {
            const builds = await call({
              action: "git.builds.list",
              projectID: row.project_id,
              sourceID: resources.sourceID,
              page: 1,
              limit: 100,
            });
            const releases = await call({
              action: "git.releases.list",
              projectID: row.project_id,
              sourceID: resources.sourceID,
              page: 1,
              limit: 100,
            });
            const terminal = new Set(
              releases.docs
                .filter((r: { status: string }) =>
                  ["failed", "superseded", "needs-reconciliation"].includes(
                    r.status,
                  ),
                )
                .map((r: { buildID: string }) => r.buildID),
            );
            resources.buildID = builds.docs.find(
              (b: { id: string; sourceSHA: string; status: string }) =>
                b.sourceSHA === row.preview!.sourceSHA &&
                b.status === "succeeded" &&
                !terminal.has(b.id),
            )?.id;
            if (!resources.buildID) {
              step.reason =
                "Run the configured GitHub Actions workflow for the reviewed commit.";
              return { resources, steps, ready: false };
            }
          } else
            resources.buildID = (
              await call({
                action: "git.builds.request",
                projectID: row.project_id,
                targetID: resources.appID!,
                sourceSHA: row.preview.sourceSHA!,
                idempotencyKey: key,
              })
            ).id;
        }
        const build = await call({
          action: "git.builds.get",
          projectID: row.project_id,
          buildID: resources.buildID!,
        });
        if (["failed", "cancelled"].includes(build.status))
          throw new HostingError(
            409,
            "Build failed. Review the build logs before retrying.",
          );
        if (build.status === "succeeded") done();
      }
    } else if (step.name === "release") {
      if (
        dependencies.beforeRelease &&
        !(await dependencies.beforeRelease({ ...row, resources }))
      ) {
        step.reason = "Waiting for application initialization.";
        return { resources, steps, ready: false };
      }
      if (config.source.kind === "git") {
        const releases = await call({
          action: "git.releases.list",
          projectID: row.project_id,
          sourceID: resources.sourceID,
          page: 1,
          limit: 100,
        });
        const release = releases.docs.find(
          (r: { buildID?: string }) => r.buildID === resources.buildID,
        );
        if (!release) {
          step.reason = "Waiting for the build artifact.";
          return { resources, steps, ready: false };
        }
        resources.releaseID = release.id;
        if (release.status === "awaiting-approval") {
          step.reason =
            "Approve this exact release in Deployments to continue.";
          return { resources, steps, ready: false };
        }
        if (
          ["failed", "needs-reconciliation", "superseded"].includes(
            release.status,
          )
        )
          throw new HostingError(
            409,
            "Release publication needs attention. Review Deployments.",
          );
        if (release.status === "ready") done();
      } else {
        if (!resources.operationID) {
          const app = await call({
            action: "apps.get",
            appID: resources.appID!,
          });
          const deployed = await call(
            app.lifecycle === "draft"
              ? {
                  action: "apps.deploy",
                  appID: resources.appID!,
                  revision: app.revision,
                  spec: {
                    ...config.spec,
                    template:
                      config.source.kind === "template"
                        ? "healthcheck"
                        : "custom",
                    image: row.preview.image!,
                  },
                  subscriptionRevision: app.subscriptionRevision,
                  idempotencyKey: key,
                }
              : {
                  action: "apps.restart",
                  appID: resources.appID!,
                  revision: app.revision,
                  subscriptionRevision: app.subscriptionRevision,
                  idempotencyKey: key,
                },
          );
          resources.operationID = deployed.id;
          step.operationID = deployed.id;
        }
        const app = await call({ action: "apps.get", appID: resources.appID! });
        if (app.status === "ready" && app.observedRevision === app.revision)
          done();
        else if (["failed", "deleted"].includes(app.status)) {
          const operation = await dependencies.transaction(
            async (db) =>
              (
                await db.query(
                  "SELECT status FROM webdock_auth.hosting_operation WHERE id=$1 AND project_id=$2",
                  [resources.operationID, row.project_id],
                )
              ).rows[0],
          );
          if (!["queued", "running"].includes(operation?.status))
            throw new HostingError(
              409,
              "Application deployment needs attention. Review its operation and logs.",
            );
        }
      }
    } else {
      const app = await call({ action: "apps.get", appID: resources.appID! });
      const bindings = await call({
        action: "services.list",
        projectID: row.project_id,
        appID: resources.appID!,
      });
      if (bindings.some((b: { state: string }) => b.state === "blocked"))
        throw new HostingError(
          409,
          "A service connection needs attention before setup can finish.",
        );
      if (
        app.status === "ready" &&
        app.revision === app.observedRevision &&
        (resources.bindingIDs ?? []).every((id) =>
          bindings.some(
            (b: { id: string; state: string }) =>
              b.id === id && b.state === "connected",
          ),
        )
      )
        done();
      else
        step.reason = "Waiting for observed application and service readiness.";
    }
    if (
      steps.every((s) => ["succeeded", "skipped"].includes(s.state)) &&
      dependencies.beforeReady &&
      !(await dependencies.beforeReady({ ...row, resources, steps }))
    ) {
      step.state = "running";
      step.reason = "Waiting for verified application publication.";
    }
    return {
      resources,
      steps,
      ready:
        steps.length === setupSteps.length &&
        steps.every((s) => ["succeeded", "skipped"].includes(s.state)),
    };
  }
  return { executeSetupStep };
}
