import { assertSetupProducts } from "./setup-admission";
import type { HostingPorts } from "./ports";
import {
  HostingError,
  setupCommandSchema,
  setupSteps,
  type HostingActor,
  type SetupView,
  type SetupConfiguration,
  type SetupResources,
  type SetupPreview,
  type SetupProgress,
} from "@spitzli/hosting-contracts";
export type Dependencies = {
  authorizeSetupConfiguration: ReturnType<
    typeof import("./permissions").createHostingPermissions
  >["authorizeSetupConfiguration"];
  requireDatabases: HostingPorts["requireDatabases"];
  requireMail: HostingPorts["requireMail"];
  executeSetupSource: ReturnType<
    typeof import("./setup-source").createHostingSetupSource
  >["executeSetupSource"];
  repairSetupEnvironment: ReturnType<
    typeof import("./setup-recovery").createHostingSetupRecovery
  >["repairSetupEnvironment"];
  resetGitSetupRetry: ReturnType<
    typeof import("./setup-recovery").createHostingSetupRecovery
  >["resetGitSetupRetry"];
  executeAppCommand: ReturnType<
    typeof import("./apps").createHostingApps
  >["executeAppCommand"];
  transaction: ReturnType<typeof import("./db").createHostingDb>["transaction"];
  audit: ReturnType<typeof import("./db").createHostingDb>["audit"];
  authorizeHosting: ReturnType<
    typeof import("./authorization").createHostingAuthorization
  >["authorizeHosting"];
  replay: ReturnType<
    typeof import("./operations").createHostingOperations
  >["replay"];
  recordResult: ReturnType<
    typeof import("./operations").createHostingOperations
  >["recordResult"];
  openEnvironment: ReturnType<
    typeof import("./environment").createHostingEnvironment
  >["openEnvironment"];
  sealEnvironment: ReturnType<
    typeof import("./environment").createHostingEnvironment
  >["sealEnvironment"];
  patchEnvironment: ReturnType<
    typeof import("./environment").createHostingEnvironment
  >["patchEnvironment"];
  previewProjectSetup: ReturnType<
    typeof import("./setup-preview").createHostingSetupPreview
  >["previewProjectSetup"];
  assertSetupConfirmation: ReturnType<
    typeof import("./setup-preview").createHostingSetupPreview
  >["assertSetupConfirmation"];
  env: HostingPorts["env"];
};
export type SetupRow = {
  id: string;
  customer_id: string;
  project_id: string;
  subject: string;
  actor: HostingActor | null;
  configuration: SetupConfiguration;
  environment_encrypted: string | null;
  revision: number;
  state: SetupView["state"];
  preview: SetupPreview | null;
  confirmed_preview_id: string | null;
  resources: SetupResources;
  steps: SetupProgress[];
  reason: string | null;
  generation: number;
  lease_until: Date | null;
};
export function createHostingProjectSetup(dependencies: Dependencies) {
  function setupView(row: SetupRow): SetupView {
    return {
      id: row.id,
      projectID: row.project_id,
      revision: row.revision,
      state: row.state,
      configuration: row.configuration,
      environmentNames: Object.keys(
        dependencies.openEnvironment(
          "setup:" + row.id,
          row.environment_encrypted,
        ),
      ),
      resources: row.resources,
      steps: row.steps,
      preview: row.preview,
      reason: row.reason,
    };
  }
  async function readSetup(
    actor: HostingActor,
    projectID: string,
    id: string,
    write = false,
  ): Promise<SetupRow> {
    return dependencies.transaction(async (db) => {
      await dependencies.authorizeHosting(
        actor,
        { permission: "org:projects:manage", projectID, write },
        db,
      );
      const row = (
        await db.query(
          "SELECT * FROM webdock_auth.hosting_setup WHERE id=$1 AND project_id=$2",
          [id, projectID],
        )
      ).rows[0];
      if (!row) throw new HostingError(404, "Project setup is unavailable.");
      return row;
    });
  }
  async function executeProjectSetup(actor: HostingActor, raw: unknown) {
    const cmd = setupCommandSchema.parse(raw);
    if (cmd.action === "setup.options" || cmd.action === "setup.inspectSource")
      return dependencies.executeSetupSource(actor, cmd);
    if (cmd.action === "setup.get")
      return setupView(await readSetup(actor, cmd.projectID, cmd.setupID));
    if (cmd.action === "setup.list")
      return dependencies.transaction(async (db) => {
        await dependencies.authorizeHosting(
          actor,
          { permission: "org:projects:manage", projectID: cmd.projectID },
          db,
        );
        return {
          docs: (
            await db.query(
              "SELECT * FROM webdock_auth.hosting_setup WHERE project_id=$1 ORDER BY updated_at DESC,id DESC LIMIT $2 OFFSET $3",
              [cmd.projectID, cmd.limit, (cmd.page - 1) * cmd.limit],
            )
          ).rows.map(setupView),
        };
      });
    if (cmd.action === "setup.preview") {
      const saved = await readSetup(actor, cmd.projectID, cmd.setupID, true);
      if (saved.revision !== cmd.revision || saved.state === "running")
        throw new HostingError(
          409,
          "Setup changed. Review the current configuration.",
        );
      assertSetupProducts(dependencies, saved.configuration, {
        databaseID: saved.resources.databaseID,
      });
      if (
        saved.configuration.database.mode === "new" &&
        !saved.resources.databaseID &&
        saved.preview?.offers[0]
      ) {
        const existing = await dependencies.transaction(async (db) => {
          await dependencies.authorizeHosting(
            actor,
            {
              permission: "org:projects:manage",
              projectID: cmd.projectID,
              write: true,
            },
            db,
          );
          return (
            await db.query(
              "SELECT d.id FROM webdock_auth.hosting_database d JOIN webdock_auth.hosting_database_order o ON o.id=d.order_id WHERE o.offer_id=$1 AND d.project_id=$2",
              [saved.preview!.offers[0].id, cmd.projectID],
            )
          ).rows[0];
        });
        if (existing) saved.resources.databaseID = existing.id;
      }
      const preview = await dependencies.previewProjectSetup(actor, {
        id: saved.id,
        projectID: saved.project_id,
        revision: saved.revision,
        configuration: saved.configuration,
        environmentNames: setupView(saved).environmentNames,
        databaseID: saved.resources.databaseID,
        appID: saved.resources.appID,
      });
      return dependencies.transaction(async (db) => {
        await dependencies.authorizeHosting(
          actor,
          {
            permission: "org:projects:manage",
            projectID: cmd.projectID,
            write: true,
          },
          db,
        );
        const row = (
          await db.query(
            "UPDATE webdock_auth.hosting_setup SET preview=$3,resources=$4,state='awaiting-confirmation',updated_at=now(),reason=NULL WHERE id=$1 AND revision=$2 AND state<>'running' RETURNING *",
            [
              saved.id,
              cmd.revision,
              JSON.stringify(preview),
              JSON.stringify(saved.resources),
            ],
          )
        ).rows[0];
        if (!row)
          throw new HostingError(
            409,
            "Setup changed. Review the current configuration.",
          );
        return setupView(row);
      });
    }
    return dependencies.transaction(async (db) => {
      const access = await dependencies.authorizeHosting(
        actor,
        {
          permission: "org:projects:manage",
          projectID: cmd.projectID,
          write: true,
        },
        db,
      );
      if (
        dependencies.env.DATABASE_RLS_SECRET &&
        !(
          await db.query(
            "SELECT webdock_rls.setup_project_available($1) AS allowed",
            [cmd.projectID],
          )
        ).rows[0]?.allowed
      )
        throw new HostingError(
          409,
          "This project is being deleted. Setup cannot continue.",
        );
      const request = await dependencies.replay(
        db,
        actor,
        cmd.idempotencyKey,
        cmd,
      );
      if (request.existing) return request.existing.result as SetupView;
      let row: SetupRow | undefined = cmd.setupID
        ? (
            await db.query(
              "SELECT * FROM webdock_auth.hosting_setup WHERE id=$1 AND project_id=$2 FOR UPDATE",
              [cmd.setupID, cmd.projectID],
            )
          ).rows[0]
        : undefined;
      if (cmd.setupID && !row)
        throw new HostingError(404, "Project setup is unavailable.");
      if ((row?.revision ?? 0) !== cmd.revision)
        throw new HostingError(
          409,
          "Setup changed. Review the current configuration.",
        );
      await dependencies.authorizeSetupConfiguration(
        db,
        actor,
        cmd.projectID,
        cmd.action === "setup.save" ? cmd.configuration : row!.configuration,
      );
      if (cmd.action === "setup.save") {
        assertSetupProducts(dependencies, cmd.configuration, {
          confirmed: !!row?.confirmed_preview_id,
          databaseID: row?.resources.databaseID,
        });
        if (row && Object.keys(row.resources).length) {
          row = await dependencies.repairSetupEnvironment(
            db,
            row,
            cmd.configuration,
            cmd.environment ?? [],
          );
        } else {
          if (row && row.state === "running")
            throw new HostingError(
              409,
              "Use application settings for changes after provisioning has started.",
            );
          const id =
            row?.id ??
            (await db.query("SELECT webdock_auth.next_snowflake() AS id"))
              .rows[0].id;
          const environment = dependencies.sealEnvironment(
            "setup:" + id,
            dependencies.patchEnvironment(
              dependencies.openEnvironment(
                "setup:" + id,
                row?.environment_encrypted,
              ),
              cmd.environment ?? [],
            ),
          );
          row = (
            await db.query(
              `INSERT INTO webdock_auth.hosting_setup(id,customer_id,project_id,subject,configuration,environment_encrypted,steps) VALUES($1,$2,$3,$4,$5,$6,$7)
        ON CONFLICT(id) DO UPDATE SET configuration=$5,environment_encrypted=$6,revision=hosting_setup.revision+1,state='draft',preview=NULL,confirmed_preview_id=NULL,reason=NULL,updated_at=now() RETURNING *`,
              [
                id,
                access.customerID,
                cmd.projectID,
                actor.subject,
                JSON.stringify(cmd.configuration),
                environment,
                JSON.stringify(
                  setupSteps.map((name) => ({
                    name,
                    state: "pending",
                    operationID: null,
                    reason: null,
                  })),
                ),
              ],
            )
          ).rows[0];
        }
      } else {
        if (!row?.preview)
          throw new HostingError(409, "Review the setup before deploying.");
        assertSetupProducts(dependencies, row.configuration, {
          confirmed:
            cmd.action === "setup.resume" ||
            (cmd.action === "setup.confirm" &&
              row.confirmed_preview_id === cmd.previewID &&
              ["running", "ready"].includes(row.state)),
          databaseID: row.resources.databaseID,
        });
        if (cmd.action === "setup.confirm") {
          dependencies.assertSetupConfirmation({
            setupRevision: row.revision,
            submittedRevision: cmd.revision,
            previewID: row.preview.id,
            submittedPreviewID: cmd.previewID,
            expiresAt: new Date(row.preview.expiresAt),
            now: new Date(),
            requiredOfferIDs: row.preview.offers.map((o) => o.id),
            acceptedOfferIDs: cmd.acceptedOfferIDs,
          });
          if (row.preview.blockers.length)
            throw new HostingError(409, row.preview.blockers[0]);
          if (
            row.confirmed_preview_id === cmd.previewID &&
            ["running", "ready"].includes(row.state)
          )
            return setupView(row);
        } else if (
          !row.confirmed_preview_id ||
          !["blocked", "failed"].includes(row.state)
        )
          throw new HostingError(
            409,
            "Only an interrupted confirmed setup can be resumed.",
          );
        if (cmd.action === "setup.resume") {
          const interrupted = row.steps.find((s) =>
            ["blocked", "failed"].includes(s.state),
          );
          if (interrupted && ["build", "release"].includes(interrupted.name)) {
            interrupted.attempt = (interrupted.attempt ?? 0) + 1;
            if (
              interrupted.name === "release" &&
              row.configuration.source.kind === "git"
            ) {
              const release = (
                await db.query(
                  "SELECT r.status FROM webdock_auth.git_release r JOIN webdock_auth.git_artifact a ON a.id=r.artifact_id WHERE r.project_id=$1 AND r.source_id=$2 AND a.build_id=$3 ORDER BY r.created_at DESC,r.id DESC LIMIT 1",
                  [
                    row.project_id,
                    row.resources.sourceID,
                    row.resources.buildID,
                  ],
                )
              ).rows[0];
              dependencies.resetGitSetupRetry(row, release?.status);
            }
            if (interrupted.name === "build") {
              delete row.resources.buildID;
              delete row.resources.releaseID;
            }
            if (
              interrupted.name === "release" &&
              row.configuration.source.kind !== "git" &&
              row.resources.operationID
            ) {
              const operation = (
                await db.query(
                  "SELECT status FROM webdock_auth.hosting_operation WHERE id=$1 AND project_id=$2",
                  [row.resources.operationID, row.project_id],
                )
              ).rows[0];
              if (operation?.status === "needs-reconciliation")
                await dependencies.executeAppCommand(
                  actor,
                  {
                    action: "apps.reconcile",
                    appID: row.resources.appID!,
                    idempotencyKey:
                      "setup-" + row.id + "-recover-" + interrupted.attempt,
                  },
                  db,
                );
              else if (operation?.status === "failed")
                delete row.resources.operationID;
            }
            let reset = false;
            for (const step of row.steps) {
              if (step === interrupted) reset = true;
              if (reset) {
                step.state = "pending";
                step.reason = null;
                step.operationID = null;
              }
            }
          }
          await db.query(
            "UPDATE webdock_auth.hosting_setup SET steps=$2,resources=$3 WHERE id=$1",
            [row.id, JSON.stringify(row.steps), JSON.stringify(row.resources)],
          );
        }
        if (
          row.configuration.database.mode === "new" &&
          !row.resources.databaseID &&
          new Date(row.preview.expiresAt) <= new Date()
        )
          throw new HostingError(
            409,
            "Setup preview expired. Review it again.",
          );
        row = (
          await db.query(
            "UPDATE webdock_auth.hosting_setup SET actor=$2,confirmed_preview_id=preview->>'id',state='running',reason=NULL,lease_until=NULL,generation=generation+1,next_attempt_at=now(),updated_at=now() WHERE id=$1 RETURNING *",
            [row.id, JSON.stringify(actor)],
          )
        ).rows[0];
      }
      if (actor.pluginProvisioningID && cmd.action === "setup.save") {
        const attached = await db.query(
          "UPDATE webdock_auth.hosting_plugin_provisioning SET setup_id=$2,updated_at=now() WHERE id=$1 AND project_id=$3 AND customer_id=$4 AND actor->>'subject'=$5 AND actor->>'sessionID'=$6 AND (setup_id IS NULL OR setup_id=$2)",
          [
            actor.pluginProvisioningID,
            row!.id,
            cmd.projectID,
            access.customerID,
            actor.subject,
            actor.sessionID,
          ],
        );
        if (!attached.rowCount)
          throw new HostingError(
            409,
            "Application provisioning ownership changed.",
          );
      }
      const result = setupView(row!);
      await dependencies.recordResult(
        db,
        actor,
        cmd.idempotencyKey,
        request.hash,
        cmd.action,
        result,
        { customerID: access.customerID, projectID: cmd.projectID },
      );
      await dependencies.audit(db, actor, cmd.action, row!.id);
      return result;
    });
  }
  return { setupView, readSetup, executeProjectSetup };
}
