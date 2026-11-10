import { gitActionPermission } from "./permissions";
import type { HostingPorts } from "./ports";
import { createHash, randomBytes } from "node:crypto";
import {
  HostingError,
  gitCommandSchema,
  gitReadActions,
  type HostingActor,
} from "@spitzli/hosting-contracts";
import type { Connection } from "./authorization";
import type { GitRepositoryBinding } from "../github";
export type Dependencies = {
  reconcileGitRelease: ReturnType<
    typeof import("./git-reconciliation").createHostingGitReconciliation
  >["reconcileGitRelease"];
  bindGitTarget: ReturnType<
    typeof import("./git-targets").createHostingGitTargets
  >["bindGitTarget"];
  openEnvironment: ReturnType<
    typeof import("./environment").createHostingEnvironment
  >["openEnvironment"];
  sealEnvironment: ReturnType<
    typeof import("./environment").createHostingEnvironment
  >["sealEnvironment"];
  patchEnvironment: ReturnType<
    typeof import("./environment").createHostingEnvironment
  >["patchEnvironment"];
  redactValues: ReturnType<
    typeof import("./environment").createHostingEnvironment
  >["redactValues"];
  authorizeHosting: ReturnType<
    typeof import("./authorization").createHostingAuthorization
  >["authorizeHosting"];
  transaction: ReturnType<typeof import("./db").createHostingDb>["transaction"];
  audit: ReturnType<typeof import("./db").createHostingDb>["audit"];
  createGitHubProvider: HostingPorts["createGitHubProvider"];
  env: HostingPorts["env"];
};
type SourceRow = {
  health_path: string;
  repository_name?: string;
  id: string;
  customer_id: string;
  project_id: string;
  connection_id: string;
  repository_id: string;
  branch: string;
  root_directory: string;
  recipe: string;
  build_provider: string;
  workflow_path: string;
  artifact_prefix: string;
  target_id: string;
  revision: number;
  enabled: boolean;
  auto_publish: boolean;
  build_environment_encrypted: string | null;
  connection_state: string;
  connection_generation: number;
  environment_revision: number;
};
type BuildRow = {
  id: string;
  project_id: string;
  source_id: string;
  target_id?: string;
  source_sha: string;
  status: string;
  source_revision: number;
  generation: number;
  created_at: Date;
  finished_at: Date | null;
  logs: string | null;
  failure_code: string | null;
  actions_provenance: Awaited<
    ReturnType<
      ReturnType<Dependencies["createGitHubProvider"]>["verifyActionsRun"]
    >
  > | null;
};
export type GitBuildCompletion = {
  buildID: string;
  generation: number;
  status: "succeeded" | "failed";
  logs: string;
  failureCode?: string;
  artifact?: {
    kind: "oci" | "vercel";
    digest: string;
    storageKey: string;
    sizeBytes: number;
  };
};
export function createHostingGitDeployments(dependencies: Dependencies) {
  const hash = (s: string) => createHash("sha256").update(s).digest("hex");
  const unavailable = () =>
    new HostingError(404, "Git deployment is unavailable.");
  const conflict = () =>
    new HostingError(409, "Git deployment changed. Refresh and try again.");
  const sourceView = (r: SourceRow | undefined) =>
    r
      ? {
          id: r.id,
          customerID: r.customer_id,
          projectID: r.project_id,
          connectionID: r.connection_id,
          repositoryID: r.repository_id,
          repositoryName: r.repository_name,
          branch: r.branch,
          rootDirectory: r.root_directory,
          recipe: r.recipe,
          buildProvider: r.build_provider,
          workflowPath: r.workflow_path,
          artifactPrefix: r.artifact_prefix,
          healthPath: r.health_path,
          targetID: r.target_id,
          revision: r.revision,
          enabled: r.enabled,
          autoPublish: r.auto_publish,
          buildEnvironmentNames: Object.keys(
            dependencies.openEnvironment(
              "git-build:" + r.id,
              r.build_environment_encrypted,
            ),
          ).sort(),
        }
      : null;
  const buildView = (r: BuildRow) => ({
    id: r.id,
    projectID: r.project_id,
    sourceID: r.source_id,
    targetID: r.target_id,
    sourceSHA: r.source_sha,
    status: r.status,
    revision: r.source_revision,
    generation: r.generation,
    createdAt: r.created_at,
    finishedAt: r.finished_at,
    logs: r.logs,
    actionsRunURL:
      r.actions_provenance?.artifact?.repository &&
      /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(
        r.actions_provenance.artifact.repository,
      ) &&
      /^[1-9][0-9]*$/.test(r.actions_provenance.runID)
        ? `https://github.com/${r.actions_provenance.artifact.repository}/actions/runs/${r.actions_provenance.runID}`
        : null,
    failureCode: r.failure_code,
  });
  const releaseView = (r: {
    id: string;
    project_id: string;
    source_id: string;
    target_id?: string;
    artifact_id: string;
    build_id?: string;
    publication_target?: {
      targetID?: string;
    };
    status: string;
    revision: number;
    approved_by: string | null;
    operation_id: string | null;
    provider_deployment_id: string | null;
    created_at: Date;
  }) => ({
    id: r.id,
    projectID: r.project_id,
    sourceID: r.source_id,
    targetID: r.target_id ?? r.publication_target?.targetID,
    artifactID: r.artifact_id,
    buildID: r.build_id,
    status: r.status,
    revision: r.revision,
    approvedBy: r.approved_by,
    operationID: r.operation_id,
    providerDeploymentID: r.provider_deployment_id,
    createdAt: r.created_at,
  });
  async function connectionView(
    db: Connection,
    r: {
      id: string;
      customer_id: string;
      installation_id: string;
      account_login: string;
      state: string;
      generation: number;
      revision: number;
    },
  ) {
    return {
      id: r.id,
      customerID: r.customer_id,
      installationID: r.installation_id,
      accountLogin: r.account_login,
      state: r.state,
      generation: r.generation,
      revision: r.revision,
      repositories: (
        await db.query(
          "SELECT metadata FROM webdock_auth.git_repository WHERE connection_id=$1",
          [r.id],
        )
      ).rows.map((v) => v.metadata),
    };
  }
  async function source(
    db: Connection,
    projectID: string,
    selector: {
      targetID?: string;
      sourceID?: string;
    } = {},
  ) {
    const rows = (
      await db.query(
        "SELECT s.*,r.metadata->>'fullName' AS repository_name,c.installation_id,c.generation AS connection_generation,c.state AS connection_state FROM webdock_auth.git_source s JOIN webdock_auth.git_connection c ON c.id=s.connection_id JOIN webdock_auth.git_repository r ON r.connection_id=s.connection_id AND r.repository_id=s.repository_id WHERE s.project_id=$1 AND ($2::text IS NULL OR s.target_id=$2) AND ($3::text IS NULL OR s.id=$3) ORDER BY s.id FOR UPDATE OF s",
        [projectID, selector.targetID ?? null, selector.sourceID ?? null],
      )
    ).rows;
    if (rows.length > 1)
      throw new HostingError(
        409,
        "Select an application for this Git operation.",
      );
    return rows[0];
  }
  function active(s: SourceRow | undefined) {
    if (!s || !s.enabled || s.connection_state !== "active")
      throw new HostingError(
        409,
        "Connect GitHub and enable a verified source first.",
      );
  }
  async function gitEnvironmentSnapshot(db: Connection, s: SourceRow) {
    if (s.recipe === "dockerfile") {
      const app = (
        await db.query(
          "SELECT spec,environment_encrypted,revision FROM webdock_auth.hosting_app WHERE id=$1 AND project_id=$2 AND status<>'deleted' FOR SHARE",
          [s.target_id, s.project_id],
        )
      ).rows[0];
      if (!app) throw unavailable();
      const runtime = { ...app.spec };
      delete runtime.image;
      delete runtime.template;
      return {
        identity: hash(
          JSON.stringify({
            environment: Object.entries(
              dependencies.openEnvironment(
                s.target_id,
                app.environment_encrypted,
              ),
            ).sort(),
            runtime: Object.fromEntries(Object.entries(runtime).sort()),
            buildEnvironment: Object.entries(
              dependencies.openEnvironment(
                "git-build:" + s.id,
                s.build_environment_encrypted,
              ),
            ).sort(),
          }),
        ),
        revision: app.revision,
      };
    }
    const target = (
      await db.query(
        "SELECT t.generation,t.mode,v.revision FROM webdock_auth.git_vercel_target t LEFT JOIN webdock_auth.hosting_vercel v ON v.customer_id=t.customer_id WHERE t.project_id=$1 AND t.customer_id=$2 AND t.target_id=$3 FOR SHARE OF t",
        [s.project_id, s.customer_id, s.target_id],
      )
    ).rows[0];
    if (!target) throw unavailable();
    return {
      identity: hash(
        JSON.stringify({
          target,
          buildEnvironment: Object.entries(
            dependencies.openEnvironment(
              "git-build:" + s.id,
              s.build_environment_encrypted,
            ),
          ).sort(),
        }),
      ),
      revision: target.generation,
    };
  }
  async function registerGitConnection(
    actor: HostingActor,
    binding: GitRepositoryBinding,
    customerID: string,
    connection?: Connection,
  ) {
    const run = async (db: Connection) => {
      await dependencies.authorizeHosting(
        actor,
        {
          permission: "org:projects:manage",
          customerID,
          write: true,
          tenantAdmin: true,
        },
        db,
      );
      await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
        "git-installation:" + binding.installationID,
      ]);
      const old = (
        await db.query(
          "SELECT * FROM webdock_auth.git_connection WHERE installation_id=$1 FOR UPDATE",
          [binding.installationID],
        )
      ).rows[0];
      if (old && old.customer_id !== customerID)
        throw new HostingError(
          409,
          "This GitHub installation belongs to another customer.",
        );
      const r = (
        await db.query(
          `INSERT INTO webdock_auth.git_connection(customer_id,installation_id,account_id,account_login,permissions) VALUES($1,$2,$3,$4,$5) ON CONFLICT(installation_id) DO UPDATE SET account_login=$4,permissions=$5,state='active',generation=git_connection.generation+1,revision=git_connection.revision+1 RETURNING *`,
          [
            customerID,
            binding.installationID,
            binding.accountID,
            binding.accountLogin,
            JSON.stringify(binding.permissions),
          ],
        )
      ).rows[0];
      await db.query(
        "INSERT INTO webdock_auth.git_repository(connection_id,customer_id,repository_id,metadata) VALUES($1,$2,$3,$4) ON CONFLICT(connection_id,repository_id) DO UPDATE SET metadata=$4",
        [r.id, customerID, binding.repositoryID, JSON.stringify(binding)],
      );
      await dependencies.audit(db, actor, "git.connection.connect", r.id);
      return connectionView(db, r);
    };
    return connection ? run(connection) : dependencies.transaction(run);
  }
  async function enqueueGitCheck(
    db: Connection,
    buildID: string,
    status: "queued" | "in_progress" | "completed",
    conclusion?: "success" | "failure" | "cancelled" | "neutral",
  ) {
    await db.query(
      `INSERT INTO webdock_auth.git_check_outbox(build_id,installation_id,repository_id,source_sha,status,conclusion) SELECT b.id,c.installation_id,s.repository_id,b.source_sha,$2,$3 FROM webdock_auth.git_build b JOIN webdock_auth.git_source s ON s.id=b.source_id JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE b.id=$1 ON CONFLICT(build_id) DO UPDATE SET status=$2,conclusion=$3,version=git_check_outbox.version+1,retry_at=now()`,
      [buildID, status, conclusion ?? null],
    );
  }
  async function cancelObsoleteGitBuilds(db: Connection) {
    if (dependencies.env.DATABASE_RLS_SECRET) {
      await db.query("SELECT webdock_rls.cancel_obsolete_git_builds()");
      return;
    }
    // The isolated legacy owner fixture has no installed RLS helpers.
    const cancelled = (
      await db.query(
        `UPDATE webdock_auth.git_build b SET status='cancelled',generation=b.generation+1,lease_until=NULL,finished_at=now(),failure_code='SOURCE_CHANGED' FROM webdock_auth.git_source s JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE b.source_id=s.id AND b.status IN ('queued','running') AND (NOT s.enabled OR c.state<>'active' OR s.revision<>b.source_revision OR c.generation<>b.connection_generation) RETURNING b.id`,
      )
    ).rows;
    for (const b of cancelled)
      await enqueueGitCheck(db, b.id, "completed", "cancelled");
    await db.query(
      `UPDATE webdock_auth.git_release r SET status='superseded',revision=r.revision+1 FROM webdock_auth.git_source s JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE r.source_id=s.id AND r.status IN ('awaiting-approval','queued') AND (NOT s.enabled OR c.state<>'active' OR s.revision<>r.source_revision OR c.generation<>r.connection_generation)`,
    );
  }
  async function queue(
    db: Connection,
    s: SourceRow,
    sha: string,
    subject: string,
    key: string,
  ) {
    active(s);
    await cancelObsoleteGitBuilds(db);
    if (!/^[a-f0-9]{40}$/.test(sha))
      throw new HostingError(400, "An exact commit is required.");
    const prior = (
      await db.query(
        "SELECT * FROM webdock_auth.git_build WHERE project_id=$1 AND idempotency_key=$2",
        [s.project_id, key],
      )
    ).rows[0];
    if (prior) {
      if (prior.source_id !== s.id) throw conflict();
      return prior;
    }
    const waiting = (
      await db.query(
        "SELECT count(*)::int n FROM webdock_auth.git_build WHERE project_id=$1 AND status IN ('queued','running')",
        [s.project_id],
      )
    ).rows[0].n;
    if (waiting >= 10)
      throw new HostingError(
        429,
        "Ten builds are already queued or running for this project.",
      );
    const environment = await gitEnvironmentSnapshot(db, s);
    const r = (
      await db.query(
        `INSERT INTO webdock_auth.git_build(customer_id,project_id,source_id,source_sha,source_revision,connection_generation,environment_revision,subject,idempotency_key,environment_identity,target_revision) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING *`,
        [
          s.customer_id,
          s.project_id,
          s.id,
          sha,
          s.revision,
          s.connection_generation,
          s.environment_revision,
          subject,
          key,
          environment.identity,
          environment.revision,
        ],
      )
    ).rows[0];
    await db.query(
      "UPDATE webdock_auth.git_source SET latest_build_id=$2,environment_identity=$3 WHERE id=$1",
      [s.id, r.id, environment.identity],
    );
    await enqueueGitCheck(db, r.id, "queued");
    return { ...r, target_id: s.target_id };
  }
  async function executeGitDeployment(
    actor: HostingActor,
    raw: unknown,
    provider?: ReturnType<Dependencies["createGitHubProvider"]>,
  ) {
    const cmd = gitCommandSchema.parse(raw);
    if (cmd.action === "git.targets.bind")
      return dependencies.bindGitTarget(actor, cmd);
    if (cmd.action === "git.releases.reconcile")
      return dependencies.reconcileGitRelease(actor, cmd);
    return dependencies.transaction(async (db) => {
      const write = !(gitReadActions as readonly string[]).includes(cmd.action);
      const access = await dependencies.authorizeHosting(
        actor,
        {
          permission: gitActionPermission(cmd.action),
          ...("projectID" in cmd
            ? { projectID: cmd.projectID }
            : { customerID: cmd.customerID }),
          write,
          ...(!("projectID" in cmd) ? { tenantAdmin: true } : {}),
        },
        db,
      );
      if (cmd.action === "git.connections.list")
        return {
          docs: await Promise.all(
            (
              await db.query(
                "SELECT * FROM webdock_auth.git_connection WHERE customer_id=$1 ORDER BY created_at",
                [access.customerID],
              )
            ).rows.map((r) => connectionView(db, r)),
          ),
        };
      if (cmd.action === "git.connections.disconnect") {
        const r = await db.query(
          "UPDATE webdock_auth.git_connection SET state='disconnected',generation=generation+1,revision=revision+1 WHERE id=$1 AND customer_id=$2 AND revision=$3 RETURNING *",
          [cmd.connectionID, access.customerID, cmd.revision],
        );
        if (!r.rows[0]) throw conflict();
        await cancelObsoleteGitBuilds(db);
        await dependencies.audit(db, actor, cmd.action, cmd.connectionID);
        return connectionView(db, r.rows[0]);
      }
      if (cmd.action === "git.repositories.list")
        return {
          docs: (
            await db.query(
              "SELECT r.metadata FROM webdock_auth.git_repository r JOIN webdock_auth.git_connection c ON c.id=r.connection_id WHERE c.id=$1 AND c.customer_id=$2 AND c.state='active'",
              [cmd.connectionID, access.customerID],
            )
          ).rows.map((r) => r.metadata),
        };
      if (!("projectID" in cmd)) throw unavailable();
      if (cmd.action === "git.sources.list") {
        const rows = (
          await db.query(
            "SELECT s.*,r.metadata->>'fullName' AS repository_name FROM webdock_auth.git_source s JOIN webdock_auth.git_repository r ON r.connection_id=s.connection_id AND r.repository_id=s.repository_id WHERE s.project_id=$1 AND s.customer_id=$2 ORDER BY s.id",
            [cmd.projectID, access.customerID],
          )
        ).rows;
        return {
          docs: rows.map((row) => ({
            ...sourceView(row),
            repositoryName: row.repository_name,
          })),
        };
      }
      const s =
        cmd.action === "git.source.get" ||
        cmd.action === "git.source.configure" ||
        cmd.action === "git.builds.request"
          ? await source(db, cmd.projectID, { targetID: cmd.targetID })
          : undefined;
      if (cmd.action === "git.source.get") {
        const targets =
          access.project?.provider === "k3s"
            ? (
                await db.query(
                  "SELECT id,name FROM webdock_auth.hosting_app WHERE project_id=$1 AND status<>'deleted'",
                  [cmd.projectID],
                )
              ).rows
            : (
                await db.query(
                  "SELECT target_id AS id,target_id AS name FROM webdock_auth.git_vercel_target WHERE customer_id=$1 AND project_id=$2",
                  [access.customerID, cmd.projectID],
                )
              ).rows;
        return {
          source: sourceView(s),
          customerID: access.customerID,
          canDeploy:
            (
              await dependencies.authorizeHosting(
                actor,
                {
                  projectID: cmd.projectID,
                  permission: "org:deployments:create",
                },
                db,
              )
            ).canWrite &&
            (access.operator || access.project?.mode === "selfservice"),
          canManage:
            access.canWrite &&
            (access.operator || access.project?.mode === "selfservice"),
          targets,
          candidateTargets:
            access.project?.provider === "vercel"
              ? (
                  await db.query(
                    "SELECT projects FROM webdock_auth.hosting_vercel WHERE customer_id=$1",
                    [access.customerID],
                  )
                ).rows.flatMap((row) =>
                  row.projects.map((id: string) => ({
                    id,
                    name: id,
                    mode: "byok",
                  })),
                )
              : [],
          canBindPlatform:
            access.operator && access.project?.provider === "vercel",
          blockers: [
            ...(!dependencies.env.SPITZLI_GITHUB_APP_ID ||
            !(
              dependencies.env.SPITZLI_GITHUB_APP_PRIVATE_KEY ||
              dependencies.env.SPITZLI_GITHUB_PRIVATE_KEY
            )
              ? [
                  "Configure the GitHub App ID and private key in the private Auth environment.",
                ]
              : []),
            ...(!dependencies.env.SPITZLI_GITHUB_WEBHOOK_SECRET
              ? [
                  "Configure the GitHub webhook secret before enabling push builds.",
                ]
              : []),
            ...(!(
              await db.query(
                "SELECT id FROM webdock_auth.git_worker WHERE enabled AND isolation=$1 LIMIT 1",
                [
                  s?.build_provider === "github-actions"
                    ? "artifact-only"
                    : "microvm",
                ],
              )
            ).rows.length
              ? [
                  s?.build_provider === "github-actions"
                    ? "Enroll a trusted artifact publisher before importing Actions builds."
                    : "Enroll verified isolated build capacity before requesting a build.",
                ]
              : []),
            ...(access.project?.provider === "k3s" &&
            !dependencies.env.SPITZLI_GIT_REGISTRY_HOST
              ? ["Configure verified artifact and registry storage."]
              : []),
          ],
        };
      }
      if (cmd.action === "git.source.configure") {
        if (cmd.buildProvider === "github-actions") {
          const environment = dependencies.patchEnvironment(
            s
              ? dependencies.openEnvironment(
                  "git-build:" + s.id,
                  s.build_environment_encrypted,
                )
              : {},
            cmd.buildEnvironment ?? [],
          );
          if (Object.keys(environment).length)
            throw new HostingError(
              422,
              "Configure build variables in GitHub Actions before using external builds.",
            );
          const permissions = (
            await db.query(
              "SELECT permissions FROM webdock_auth.git_connection WHERE id=$1 AND customer_id=$2",
              [cmd.connectionID, access.customerID],
            )
          ).rows[0]?.permissions;
          if (!["read", "write"].includes(permissions?.actions))
            throw new HostingError(
              422,
              "Accept the GitHub App Actions read permission first.",
            );
        }
        if ((s?.revision ?? 0) !== cmd.revision) throw conflict();
        const c = (
          await db.query(
            "SELECT c.* FROM webdock_auth.git_connection c JOIN webdock_auth.git_repository r ON r.connection_id=c.id WHERE c.id=$1 AND c.customer_id=$2 AND r.repository_id=$3 AND c.state='active' FOR SHARE OF c",
            [cmd.connectionID, access.customerID, cmd.repositoryID],
          )
        ).rows[0];
        if (!c) throw unavailable();
        if (cmd.recipe === "dockerfile") {
          if (
            access.project?.provider !== "k3s" ||
            !(
              await db.query(
                "SELECT id FROM webdock_auth.hosting_app WHERE id=$1 AND project_id=$2 AND status<>'deleted'",
                [cmd.targetID, cmd.projectID],
              )
            ).rows[0]
          )
            throw unavailable();
        } else {
          if (
            access.project?.provider !== "vercel" ||
            !(
              await db.query(
                "SELECT customer_id FROM webdock_auth.git_vercel_target WHERE customer_id=$1 AND project_id=$2 AND target_id=$3",
                [access.customerID, cmd.projectID, cmd.targetID],
              )
            ).rows[0]
          )
            throw unavailable();
        }
        const r = (
          await db.query(
            `INSERT INTO webdock_auth.git_source(customer_id,project_id,connection_id,repository_id,branch,root_directory,recipe,target_id,enabled,auto_publish,policy_subject,policy_revision,build_provider,workflow_path,artifact_prefix,health_path) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,1,$12,$13,$14,$15) ON CONFLICT(project_id,target_id) DO UPDATE SET connection_id=$3,repository_id=$4,branch=$5,root_directory=$6,recipe=$7,target_id=$8,enabled=$9,auto_publish=$10,policy_subject=$11,policy_revision=git_source.revision+1,revision=git_source.revision+1,latest_build_id=NULL,build_provider=$12,workflow_path=$13,artifact_prefix=$14,health_path=$15,actions_configured_at=now() RETURNING *`,
            [
              access.customerID,
              cmd.projectID,
              cmd.connectionID,
              cmd.repositoryID,
              cmd.branch,
              cmd.rootDirectory,
              cmd.recipe,
              cmd.targetID,
              cmd.enabled,
              cmd.autoPublish,
              actor.subject,
              cmd.buildProvider,
              cmd.workflowPath,
              cmd.artifactPrefix,
              cmd.healthPath,
            ],
          )
        ).rows[0];
        if (cmd.buildEnvironment) {
          const encrypted = dependencies.sealEnvironment(
            "git-build:" + r.id,
            dependencies.patchEnvironment(
              dependencies.openEnvironment(
                "git-build:" + r.id,
                s?.build_environment_encrypted,
              ),
              cmd.buildEnvironment,
            ),
          );
          await db.query(
            "UPDATE webdock_auth.git_source SET build_environment_encrypted=$2 WHERE id=$1",
            [r.id, encrypted],
          );
          r.build_environment_encrypted = encrypted;
        }
        await cancelObsoleteGitBuilds(db);
        await dependencies.audit(db, actor, cmd.action, r.id);
        return sourceView(r);
      }
      if (
        cmd.action === "git.builds.list" ||
        cmd.action === "git.releases.list"
      ) {
        const table =
            cmd.action === "git.builds.list" ? "git_build" : "git_release",
          view = table === "git_build" ? buildView : releaseView;
        const rows = (
          await db.query(
            `SELECT v.*,s.target_id,${table === "git_release" ? "(SELECT build_id FROM webdock_auth.git_artifact WHERE id=v.artifact_id) AS build_id" : "v.id AS build_id"} FROM webdock_auth.${table} v JOIN webdock_auth.git_source s ON s.id=v.source_id WHERE v.project_id=$1 AND v.customer_id=$2 AND ($5::text IS NULL OR v.source_id=$5) ORDER BY v.created_at DESC,v.id DESC LIMIT $3 OFFSET $4`,
            [
              cmd.projectID,
              access.customerID,
              cmd.limit,
              (cmd.page - 1) * cmd.limit,
              cmd.sourceID ?? null,
            ],
          )
        ).rows;
        const count = (
          await db.query(
            `SELECT count(*)::int n FROM webdock_auth.${table} WHERE project_id=$1 AND customer_id=$2 AND ($3::text IS NULL OR source_id=$3)`,
            [cmd.projectID, access.customerID, cmd.sourceID ?? null],
          )
        ).rows[0].n;
        return {
          docs: rows.map((r) => view(r)),
          page: cmd.page,
          limit: cmd.limit,
          totalDocs: count,
          totalPages: Math.ceil(count / cmd.limit),
        };
      }
      if (
        cmd.action === "git.builds.get" ||
        cmd.action === "git.builds.cancel"
      ) {
        const b = (
          await db.query(
            "SELECT b.*,s.target_id FROM webdock_auth.git_build b JOIN webdock_auth.git_source s ON s.id=b.source_id WHERE b.id=$1 AND b.project_id=$2 AND b.customer_id=$3 FOR UPDATE OF b",
            [cmd.buildID, cmd.projectID, access.customerID],
          )
        ).rows[0];
        if (!b) throw unavailable();
        if (cmd.action === "git.builds.cancel") {
          if (!["queued", "running"].includes(b.status)) throw conflict();
          await db.query(
            "UPDATE webdock_auth.git_build SET status='cancelled',generation=generation+1,lease_until=NULL,finished_at=now() WHERE id=$1",
            [b.id],
          );
          await dependencies.audit(db, actor, cmd.action, b.id);
          b.status = "cancelled";
          b.generation++;
          await enqueueGitCheck(db, b.id, "completed", "cancelled");
        }
        return buildView(b);
      }
      if (cmd.action === "git.builds.request") {
        active(s);
        if (s.build_provider === "github-actions")
          throw new HostingError(
            422,
            "Run the configured workflow in GitHub Actions. Webdock imports successful builds automatically.",
          );
        const existing = (
          await db.query(
            "SELECT * FROM webdock_auth.git_build WHERE project_id=$1 AND idempotency_key=$2",
            [cmd.projectID, cmd.idempotencyKey],
          )
        ).rows[0];
        if (existing) {
          if (existing.source_id !== s.id) throw conflict();
          return buildView(existing);
        }
        const github = provider ?? dependencies.createGitHubProvider();
        const selected = {
          installationID: s.installation_id,
          repositoryID: s.repository_id,
          branch: s.branch,
        };
        const resolved = cmd.sourceSHA
          ? await github.resolveCommit(selected, cmd.sourceSHA)
          : await github.resolveSource(selected);
        const b = await queue(
          db,
          s,
          resolved.sha,
          actor.subject,
          cmd.idempotencyKey,
        );
        await dependencies.audit(db, actor, cmd.action, b.id);
        return buildView(b);
      }
      if (
        cmd.action === "git.releases.approve" ||
        cmd.action === "git.releases.rollback"
      ) {
        const r = (
          await db.query(
            "SELECT r.*,a.retained,b.id AS build_id FROM webdock_auth.git_release r JOIN webdock_auth.git_artifact a ON a.id=r.artifact_id JOIN webdock_auth.git_build b ON b.id=a.build_id WHERE r.id=$1 AND r.project_id=$2 AND r.customer_id=$3 FOR UPDATE OF r",
            [cmd.releaseID, cmd.projectID, access.customerID],
          )
        ).rows[0];
        if (!r) throw unavailable();
        const s = await source(db, cmd.projectID, { sourceID: r.source_id });
        active(s);
        // Retention may have committed while this request waited for the source lock.
        const retained = (
          await db.query(
            "SELECT retained FROM webdock_auth.git_artifact WHERE id=$1",
            [r.artifact_id],
          )
        ).rows[0]?.retained;
        const prior = (
          await db.query(
            "SELECT * FROM webdock_auth.git_release WHERE project_id=$1 AND idempotency_key=$2",
            [cmd.projectID, cmd.idempotencyKey],
          )
        ).rows[0];
        if (prior) {
          if (prior.source_id !== s.id) throw conflict();
          return releaseView(prior);
        }
        const environment = await gitEnvironmentSnapshot(db, s);
        if (r.environment_identity !== environment.identity)
          throw new HostingError(
            409,
            "Runtime configuration changed. Build again before publication.",
          );
        if (
          r.revision !== cmd.revision ||
          r.source_revision !== s.revision ||
          r.environment_revision !== s.environment_revision ||
          r.connection_generation !== s.connection_generation ||
          !retained
        )
          throw conflict();
        let result;
        if (cmd.action === "git.releases.rollback") {
          if (r.status !== "ready")
            throw new HostingError(
              409,
              "Rollback requires a retained healthy release with compatible configuration.",
            );
          result = (
            await db.query(
              `INSERT INTO webdock_auth.git_release(customer_id,project_id,artifact_id,status,source_revision,connection_generation,environment_revision,subject,approved_by,idempotency_key,publication_target,source_id) VALUES($1,$2,$3,'queued',$4,$5,$6,$7,$7,$8,$9,$10) RETURNING *`,
              [
                access.customerID,
                cmd.projectID,
                r.artifact_id,
                s.revision,
                s.connection_generation,
                s.environment_revision,
                actor.subject,
                cmd.idempotencyKey,
                JSON.stringify(r.publication_target),
                s.id,
              ],
            )
          ).rows[0];
        } else {
          if (
            !["awaiting-approval", "queued"].includes(r.status) ||
            r.build_id !== s.latest_build_id
          )
            throw conflict();
          result = (
            await db.query(
              "UPDATE webdock_auth.git_release SET status='queued',approved_by=$2,idempotency_key=$3,revision=revision+1 WHERE id=$1 RETURNING *",
              [r.id, actor.subject, cmd.idempotencyKey],
            )
          ).rows[0];
        }
        await db.query(
          "UPDATE webdock_auth.git_release SET approved_session_id=$2,approved_source=$3,approved_scopes=$4,environment_identity=$5 WHERE id=$1",
          [
            result.id,
            actor.sessionID,
            actor.source,
            JSON.stringify(actor.scopes),
            environment.identity,
          ],
        );
        await db.query(
          "UPDATE webdock_auth.git_release SET status='superseded',revision=revision+1 WHERE source_id=$1 AND id<>$2 AND status='queued'",
          [s.id, result.id],
        );
        await db.query(
          "UPDATE webdock_auth.git_source SET desired_release_id=$2 WHERE id=$1",
          [s.id, result.id],
        );
        await dependencies.audit(db, actor, cmd.action, result.id);
        return releaseView(result);
      }
      throw unavailable();
    });
  }
  async function enrollGitWorker(
    actor: HostingActor,
    input: {
      country: string;
      isolation: "microvm" | "artifact-only";
      evidence: string;
      capacity: number;
    },
  ) {
    return dependencies.transaction(async (db) => {
      await dependencies.authorizeHosting(
        actor,
        { permission: "org:projects:manage", operator: true, write: true },
        db,
      );
      if (
        !/^[A-Z]{2}$/.test(input.country) ||
        (input.country === "ZZ" && !/\bunverified\b/i.test(input.evidence)) ||
        !["microvm", "artifact-only"].includes(input.isolation) ||
        input.evidence.length < 20 ||
        !Number.isInteger(input.capacity) ||
        input.capacity < 1 ||
        input.capacity > 32
      )
        throw new HostingError(
          400,
          "Verified build or artifact publisher capacity is required.",
        );
      const credential = randomBytes(32).toString("base64url");
      const r = (
        await db.query(
          "INSERT INTO webdock_auth.git_worker(credential_hash,country,isolation,evidence,capacity,enabled) VALUES($1,$2,$3,$4,$5,true) RETURNING id,generation",
          [
            hash(credential),
            input.country,
            input.isolation,
            input.evidence,
            input.capacity,
          ],
        )
      ).rows[0];
      await dependencies.audit(db, actor, "git.worker.enroll", r.id);
      return { ...r, credential };
    });
  }
  async function authenticateGitWorker(db: Connection, credential: string) {
    if (
      typeof credential !== "string" ||
      credential.length < 32 ||
      credential.length > 200
    )
      throw new HostingError(401, "Invalid build worker.");
    const w = (
      await db.query(
        "SELECT * FROM webdock_auth.git_worker WHERE credential_hash=$1 AND enabled FOR UPDATE",
        [hash(credential)],
      )
    ).rows[0];
    if (!w) throw new HostingError(401, "Invalid build worker.");
    return w;
  }
  async function claimGitBuild(credential: string) {
    return dependencies.transaction(async (db) => {
      const w = await authenticateGitWorker(db, credential);
      await cancelObsoleteGitBuilds(db);
      const n = (
        await db.query(
          "SELECT count(*)::int n FROM webdock_auth.git_build WHERE worker_id=$1 AND status='running' AND lease_until>now()",
          [w.id],
        )
      ).rows[0].n;
      if (n >= w.capacity) return null;
      const b = (
        await db.query(
          `SELECT b.id FROM webdock_auth.git_build b JOIN webdock_auth.git_source s ON s.id=b.source_id JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE (b.status='queued' OR (b.status='running' AND b.lease_until<now())) AND c.state='active' AND s.enabled AND c.generation=b.connection_generation AND s.revision=b.source_revision AND ((s.build_provider='isolated' AND $1='microvm') OR (s.build_provider='github-actions' AND $1='artifact-only' AND b.actions_provenance IS NOT NULL)) ORDER BY b.created_at FOR UPDATE OF b SKIP LOCKED LIMIT 1`,
          [w.isolation],
        )
      ).rows[0];
      if (!b) return null;
      const r = (
        await db.query(
          "UPDATE webdock_auth.git_build SET status='running',worker_id=$2,worker_generation=$3,generation=generation+1,lease_until=now()+interval '27 minutes' WHERE id=$1 RETURNING *",
          [b.id, w.id, w.generation],
        )
      ).rows[0];
      const s = (
        await db.query(
          "SELECT s.*,c.installation_id FROM webdock_auth.git_source s JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE s.id=$1",
          [r.source_id],
        )
      ).rows[0];
      await enqueueGitCheck(db, r.id, "in_progress");
      return {
        buildID: r.id,
        customerID: r.customer_id,
        projectID: r.project_id,
        generation: r.generation,
        sourceSHA: r.source_sha,
        repositoryID: s.repository_id,
        installationID: s.installation_id,
        rootDirectory: s.root_directory,
        recipe: s.recipe,
        buildProvider: s.build_provider,
        environmentRevision: r.environment_revision,
        leaseUntil: r.lease_until,
        buildEnvironment: dependencies.openEnvironment(
          "git-build:" + s.id,
          s.build_environment_encrypted,
        ),
        limits: {
          cpu: 2,
          memoryMiB: 4096,
          diskBytes: 10737418240,
          durationSeconds: 1500,
          logBytes: 262144,
        },
      };
    });
  }
  /** Refresh a short-lived download capability only for the fenced, immutable artifact. */
  async function prepareGitActionsArtifact(
    build: BuildRow &
      Pick<
        SourceRow,
        | "build_provider"
        | "repository_id"
        | "branch"
        | "workflow_path"
        | "artifact_prefix"
      > & {
        installation_id: string;
      },
    provider = dependencies.createGitHubProvider(),
  ) {
    const provenance = build.actions_provenance;
    if (
      build.build_provider !== "github-actions" ||
      !provenance ||
      provenance.runAttempt !== 1
    )
      throw conflict();
    const head = await provider.resolveSource({
      installationID: build.installation_id,
      repositoryID: build.repository_id,
      branch: build.branch,
    });
    if (head.sha !== build.source_sha) throw conflict();
    const verified = await provider.verifyActionsRun({
      installationID: build.installation_id,
      repositoryID: build.repository_id,
      workflowPath: build.workflow_path,
      runID: provenance.runID,
      runAttempt: provenance.runAttempt,
      branch: build.branch,
      sha: build.source_sha,
      artifactName: build.artifact_prefix + "-" + build.source_sha,
    });
    if (
      verified.artifact.id !== provenance.artifact.id ||
      verified.artifact.digest !== provenance.artifact.digest ||
      verified.artifact.sizeBytes !== provenance.artifact.sizeBytes
    )
      throw conflict();
    return verified.artifact;
  }
  async function completeGitBuild(
    credential: string,
    result: GitBuildCompletion,
    provider = dependencies.createGitHubProvider(),
  ) {
    return dependencies.transaction(async (db) => {
      const w = await authenticateGitWorker(db, credential);
      const b = (
        await db.query(
          "SELECT * FROM webdock_auth.git_build WHERE id=$1 AND status='running' AND worker_id=$2 AND worker_generation=$3 AND generation=$4 AND lease_until>now() FOR UPDATE",
          [result.buildID, w.id, w.generation, result.generation],
        )
      ).rows[0];
      if (!b) throw conflict();
      if (
        !["succeeded", "failed"].includes(result.status) ||
        typeof result.logs !== "string" ||
        Buffer.byteLength(result.logs) > 262144 ||
        (result.failureCode && !/^[A-Z0-9_]{1,80}$/.test(result.failureCode))
      )
        throw new HostingError(400, "Invalid build result.");
      const s = await source(db, b.project_id, { sourceID: b.source_id });
      if (
        !s ||
        s.revision !== b.source_revision ||
        s.connection_generation !== b.connection_generation ||
        s.connection_state !== "active"
      )
        throw conflict();
      let release;
      if (
        result.status === "succeeded" &&
        s.build_provider === "github-actions"
      ) {
        const head = await provider.resolveSource({
          installationID: s.installation_id,
          repositoryID: s.repository_id,
          branch: s.branch,
        });
        if (head.sha !== b.source_sha) throw conflict();
      }
      if (result.status === "succeeded") {
        const a = result.artifact,
          prefix = `${b.customer_id}/${b.id}/${b.generation}/`;
        if (
          !a ||
          a.kind !== (s.recipe === "dockerfile" ? "oci" : "vercel") ||
          !/^sha256:[a-f0-9]{64}$/.test(a.digest) ||
          !a.storageKey.startsWith(prefix) ||
          !/^\d+\/\d+\/\d+\/[a-f0-9]{64}\.tar$/.test(a.storageKey) ||
          !Number.isSafeInteger(a.sizeBytes) ||
          a.sizeBytes < 1 ||
          a.sizeBytes > 10737418240
        )
          throw new HostingError(400, "Invalid immutable artifact.");
        const artifact = (
          await db.query(
            "INSERT INTO webdock_auth.git_artifact(build_id,customer_id,project_id,kind,digest,storage_key,size_bytes) VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING id",
            [
              b.id,
              b.customer_id,
              b.project_id,
              a.kind,
              a.digest,
              a.storageKey,
              a.sizeBytes,
            ],
          )
        ).rows[0];
        release = (
          await db.query(
            "INSERT INTO webdock_auth.git_release(customer_id,project_id,artifact_id,source_revision,connection_generation,environment_revision,subject,environment_identity,source_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9) RETURNING *",
            [
              b.customer_id,
              b.project_id,
              artifact.id,
              b.source_revision,
              b.connection_generation,
              b.environment_revision,
              b.subject,
              b.environment_identity,
              b.source_id,
            ],
          )
        ).rows[0];
        await db.query(
          `UPDATE webdock_auth.git_release r SET publication_target=jsonb_build_object('recipe',s.recipe,'targetID',s.target_id,'teamID',t.team_id,'healthPath',s.health_path)
        FROM webdock_auth.git_source s LEFT JOIN webdock_auth.git_vercel_target t ON t.project_id=s.project_id AND t.customer_id=s.customer_id AND t.target_id=s.target_id
        WHERE r.id=$1 AND s.id=$2`,
          [release.id, s.id],
        );
      }
      if (
        release &&
        s.auto_publish &&
        s.policy_revision === s.revision &&
        s.latest_build_id === b.id
      ) {
        const environment = await gitEnvironmentSnapshot(db, s);
        let authorized = false;
        try {
          await dependencies.authorizeHosting(
            {
              subject: s.policy_subject,
              sessionID: "",
              source: "git-policy",
              gitSourceID: s.id,
              gitSourceRevision: s.revision,
              scopes: ["hosting:read", "hosting:write"],
            },
            {
              permission: "org:deployments:create",
              projectID: s.project_id,
              write: true,
            },
            db,
          );
          authorized = true;
        } catch (error) {
          if (!(error instanceof HostingError)) throw error;
        }
        if (authorized && environment.identity === b.environment_identity) {
          release = (
            await db.query(
              "UPDATE webdock_auth.git_release SET status='queued',approved_by=$2,approved_session_id='',approved_source='git-policy',approved_scopes=$3,revision=revision+1 WHERE id=$1 RETURNING *",
              [
                release.id,
                s.policy_subject,
                JSON.stringify(["hosting:read", "hosting:write"]),
              ],
            )
          ).rows[0];
          await db.query(
            "UPDATE webdock_auth.git_release SET status='superseded',revision=revision+1 WHERE source_id=$1 AND id<>$2 AND status='queued'",
            [s.id, release.id],
          );
          await db.query(
            "UPDATE webdock_auth.git_source SET desired_release_id=$2 WHERE id=$1",
            [s.id, release.id],
          );
          await db.query(
            "INSERT INTO webdock_auth.hosting_audit(subject,action,target_id,outcome) VALUES($1,'git.policy.approve',$2,'succeeded')",
            [s.policy_subject, release.id],
          );
        }
      }
      await db.query(
        "UPDATE webdock_auth.git_build SET status=$2,logs=$3,failure_code=$4,finished_at=now(),lease_until=NULL WHERE id=$1",
        [
          b.id,
          result.status,
          Buffer.from(
            dependencies.redactValues(
              result.logs,
              Object.values(
                dependencies.openEnvironment(
                  "git-build:" + s.id,
                  s.build_environment_encrypted,
                ),
              ),
            ),
          )
            .subarray(0, 262140)
            .toString("utf8"),
          result.failureCode ?? null,
        ],
      );
      await enqueueGitCheck(
        db,
        b.id,
        result.status === "failed" ? "completed" : "in_progress",
        result.status === "failed" ? "failure" : undefined,
      );
      return {
        buildID: b.id,
        status: result.status,
        release: release ? releaseView(release) : null,
      };
    });
  }
  /** Only call after raw-body signature verification. Network work is deferred to workers. */
  async function receiveGitEvent(event: {
    deliveryID: string;
    event: string;
    installationID: string;
    repositoryID?: string;
    branch?: string;
    sha?: string;
    deleted?: boolean;
    action?: string;
    runID?: string;
    runAttempt?: number;
    workflowPath?: string;
    conclusion?: string | null;
  }) {
    return dependencies.transaction(async (db) => {
      if (!/^[A-Za-z0-9_-]{1,100}$/.test(event.deliveryID))
        throw new HostingError(400, "Invalid delivery.");
      const data = {
        repositoryID: event.repositoryID,
        branch: event.branch,
        deleted: event.deleted,
        sha: event.sha,
        runID: event.runID,
        runAttempt: event.runAttempt,
        workflowPath: event.workflowPath,
        conclusion: event.conclusion,
        action: event.action,
      };
      const receipt = await db.query(
        "INSERT INTO webdock_auth.git_receipt(delivery_id,event,installation_id,event_data) VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING RETURNING delivery_id",
        [
          event.deliveryID,
          event.event,
          event.installationID,
          JSON.stringify(data),
        ],
      );
      if (!receipt.rowCount) return { duplicate: true, queued: 0 };
      if (event.event !== "push" && event.event !== "workflow_run") {
        await db.query(
          "UPDATE webdock_auth.git_connection SET state='revalidation-required',generation=generation+1,revision=revision+1 WHERE installation_id=$1",
          [event.installationID],
        );
        await db.query(
          "UPDATE webdock_auth.git_receipt SET processed_at=now() WHERE delivery_id=$1",
          [event.deliveryID],
        );
      }
      await cancelObsoleteGitBuilds(db);
      return { duplicate: false, queued: 0 };
    });
  }
  /** Asynchronous receipt drain. Locked receipts make retries atomic; current heads prevent old deliveries overtaking new commits. */
  async function processGitEvents(
    provider?: ReturnType<Dependencies["createGitHubProvider"]>,
  ) {
    let deliveryID: string | undefined;
    try {
      return await dependencies.transaction(async (db) => {
        const e = (
          await db.query(
            "SELECT * FROM webdock_auth.git_receipt WHERE processed_at IS NULL AND attempts<10 AND next_attempt_at<=now() ORDER BY next_attempt_at,created_at FOR UPDATE SKIP LOCKED LIMIT 1",
          )
        ).rows[0];
        if (!e) return { processed: false, queued: 0 };
        deliveryID = e.delivery_id;
        const data = e.event_data ?? {};
        const sources = data.deleted
          ? []
          : (
              await db.query(
                "SELECT s.*,c.installation_id,c.generation AS connection_generation,c.state AS connection_state FROM webdock_auth.git_source s JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE c.installation_id=$1 AND s.repository_id=$2 AND s.branch=$3 AND s.enabled AND c.state='active' ORDER BY s.id FOR UPDATE OF s",
                [e.installation_id, data.repositoryID, data.branch],
              )
            ).rows;
        let queued = 0;
        for (const s of sources) {
          const github = provider ?? dependencies.createGitHubProvider();
          if (s.build_provider === "github-actions") {
            if (
              e.event !== "workflow_run" ||
              data.action !== "completed" ||
              data.conclusion !== "success" ||
              data.runAttempt !== 1 ||
              data.workflowPath !== s.workflow_path
            )
              continue;
            if (
              Object.keys(
                dependencies.openEnvironment(
                  "git-build:" + s.id,
                  s.build_environment_encrypted,
                ),
              ).length
            )
              continue;
            const pinned = await github.resolveSource({
              installationID: s.installation_id,
              repositoryID: s.repository_id,
              branch: s.branch,
            });
            if (pinned.sha !== data.sha) continue;
            const verified = await github.verifyActionsRun({
              installationID: s.installation_id,
              repositoryID: s.repository_id,
              workflowPath: s.workflow_path,
              runID: data.runID,
              runAttempt: data.runAttempt,
              branch: s.branch,
              sha: pinned.sha,
              artifactName: s.artifact_prefix + "-" + pinned.sha,
            });
            // Configuration made after a run began cannot authorize that older build.
            if (
              !Number.isFinite(Date.parse(verified.createdAt)) ||
              Date.parse(verified.createdAt) <
                new Date(s.actions_configured_at).getTime()
            )
              continue;
            if (s.latest_build_id) {
              const latest = (
                await db.query(
                  "SELECT actions_provenance FROM webdock_auth.git_build WHERE id=$1 AND source_id=$2",
                  [s.latest_build_id, s.id],
                )
              ).rows[0]?.actions_provenance;
              if (latest) {
                const currentTime = Date.parse(verified.createdAt);
                const latestTime = Date.parse(latest.createdAt);
                // Completion delivery order must not reorder runs of the same commit.
                if (
                  !Number.isFinite(latestTime) ||
                  currentTime < latestTime ||
                  (currentTime === latestTime &&
                    BigInt(verified.runID) <= BigInt(latest.runID))
                )
                  continue;
              }
            }
            const key =
              "actions:" + data.runID + ":" + data.runAttempt + ":" + s.id;
            const prior = (
              await db.query(
                "SELECT id FROM webdock_auth.git_build WHERE source_id=$1 AND actions_provenance->>'runID'=$2 AND actions_provenance->>'runAttempt'=$3",
                [s.id, String(data.runID), String(data.runAttempt)],
              )
            ).rows[0];
            if (prior) continue;
            const build = await queue(
              db,
              s,
              pinned.sha,
              "github:" + e.installation_id,
              key,
            );
            const artifact = { ...verified.artifact };
            Reflect.deleteProperty(artifact, "downloadURL");
            await db.query(
              "UPDATE webdock_auth.git_build SET actions_provenance=$2 WHERE id=$1",
              [build.id, JSON.stringify({ ...verified, artifact })],
            );
            queued++;
          } else if (e.event === "push") {
            const pinned = await github.resolveSource({
              installationID: s.installation_id,
              repositoryID: s.repository_id,
              branch: s.branch,
            });
            await queue(
              db,
              s,
              pinned.sha,
              "github:" + e.installation_id,
              "webhook:" + e.delivery_id + ":" + s.id,
            );
            queued++;
          }
        }
        await db.query(
          "UPDATE webdock_auth.git_receipt SET processed_at=now() WHERE delivery_id=$1",
          [e.delivery_id],
        );
        return { processed: true, queued };
      });
    } catch {
      if (deliveryID)
        await dependencies.transaction(async (db) => {
          await db.query(
            "UPDATE webdock_auth.git_receipt SET attempts=attempts+1,next_attempt_at=now()+make_interval(secs=>least(3600,30*power(2,attempts)::integer)),failure_code='PROCESSING_UNAVAILABLE' WHERE delivery_id=$1 AND processed_at IS NULL",
            [deliveryID],
          );
        });
      return { processed: false, queued: 0 };
    }
  }
  /** Publisher leases never requeue an ambiguous write. Expiration requires reconciliation. */
  async function claimGitRelease(credential: string) {
    return dependencies.transaction(async (db) => {
      const w = await authenticateGitWorker(db, credential);
      await db.query(
        "UPDATE webdock_auth.git_release SET status='needs-reconciliation',revision=revision+1 WHERE status='deploying' AND lease_until<now()",
      );
      const r = (
        await db.query(
          `SELECT r.*,a.kind,a.digest,a.storage_key,a.size_bytes,b.source_sha,b.id AS build_id,s.target_id,s.recipe FROM webdock_auth.git_release r JOIN webdock_auth.git_artifact a ON a.id=r.artifact_id JOIN webdock_auth.git_build b ON b.id=a.build_id JOIN webdock_auth.git_source s ON s.id=r.source_id JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE r.status='queued' AND b.worker_id=$1 AND b.worker_generation=$2 AND s.desired_release_id=r.id AND s.enabled AND c.state='active' AND r.source_revision=s.revision AND r.connection_generation=c.generation AND r.environment_revision=s.environment_revision AND a.retained AND NOT EXISTS(SELECT 1 FROM webdock_auth.git_release busy WHERE busy.source_id=r.source_id AND busy.status IN ('deploying','needs-reconciliation')) ORDER BY r.created_at FOR UPDATE OF r,s SKIP LOCKED LIMIT 1`,
          [w.id, w.generation],
        )
      ).rows[0];
      if (!r) return null;
      const sourceState = await source(db, r.project_id, {
        sourceID: r.source_id,
      });
      const environment = await gitEnvironmentSnapshot(db, sourceState);
      if (environment.identity !== r.environment_identity) {
        await db.query(
          "UPDATE webdock_auth.git_release SET status='superseded',revision=revision+1 WHERE id=$1",
          [r.id],
        );
        return null;
      }
      const updated = (
        await db.query(
          "UPDATE webdock_auth.git_release SET status='deploying',generation=generation+1,revision=revision+1,worker_id=$2,worker_generation=$3,lease_until=now()+interval '10 minutes' WHERE id=$1 RETURNING generation,lease_until",
          [r.id, w.id, w.generation],
        )
      ).rows[0];
      await enqueueGitCheck(db, r.build_id, "in_progress");
      return {
        releaseID: r.id,
        buildID: r.build_id,
        leaseUntil: updated.lease_until,
        customerID: r.customer_id,
        projectID: r.project_id,
        targetID: r.target_id,
        generation: updated.generation,
        sourceSHA: r.source_sha,
        recipe: r.recipe,
        artifact: {
          id: r.artifact_id,
          kind: r.kind,
          digest: r.digest,
          storageKey: r.storage_key,
          sizeBytes: Number(r.size_bytes),
        },
        idempotencyKey: `git-release-${r.id}`,
      };
    });
  }
  async function completeGitRelease(
    credential: string,
    result: {
      releaseID: string;
      generation: number;
      status: "deploying" | "ready" | "failed" | "needs-reconciliation";
      operationID?: string;
      providerDeploymentID?: string;
      healthVerified?: boolean;
      region?: string;
    },
  ) {
    return dependencies.transaction(async (db) => {
      const w = await authenticateGitWorker(db, credential);
      const r = (
        await db.query(
          "SELECT * FROM webdock_auth.git_release WHERE id=$1 AND status='deploying' AND worker_id=$2 AND worker_generation=$3 AND generation=$4 AND lease_until>now() FOR UPDATE",
          [result.releaseID, w.id, w.generation, result.generation],
        )
      ).rows[0];
      if (!r) throw conflict();
      if (
        !["deploying", "ready", "failed", "needs-reconciliation"].includes(
          result.status,
        ) ||
        (result.operationID &&
          !/^[1-9][0-9]{0,18}$/.test(result.operationID)) ||
        (result.providerDeploymentID &&
          !/^[A-Za-z0-9_-]{1,200}$/.test(result.providerDeploymentID))
      )
        throw new HostingError(400, "Invalid publication result.");
      if (
        (result.operationID && r.operation_id !== result.operationID) ||
        (result.providerDeploymentID &&
          r.provider_deployment_id !== result.providerDeploymentID)
      )
        throw new HostingError(409, "Publication identity cannot be replaced.");
      const s = await source(db, r.project_id, { sourceID: r.source_id });
      const current =
        s &&
        s.enabled &&
        s.connection_state === "active" &&
        s.connection_generation === r.connection_generation &&
        s.revision === r.source_revision &&
        s.desired_release_id === r.id;
      if (
        result.status === "ready" &&
        (!result.healthVerified ||
          !(result.operationID || result.providerDeploymentID) ||
          (s?.recipe === "vercel" && result.region !== "fra1"))
      )
        throw new HostingError(
          409,
          "Publication requires observed health and verified target location.",
        );
      const preflightOnly =
        result.status === "needs-reconciliation" &&
        (!r.publication_started_at || s?.recipe === "dockerfile") &&
        !r.operation_id &&
        !r.provider_deployment_id &&
        !result.operationID &&
        !result.providerDeploymentID;
      const status = preflightOnly
        ? current
          ? "awaiting-approval"
          : "superseded"
        : current
          ? result.status
          : "needs-reconciliation";
      if (preflightOnly) {
        await db.query(
          "UPDATE webdock_auth.git_release SET approved_by=NULL,approved_session_id=NULL,approved_source=NULL,approved_scopes=NULL,idempotency_key=NULL WHERE id=$1",
          [r.id],
        );
        await db.query(
          "UPDATE webdock_auth.git_source SET desired_release_id=NULL WHERE id=$1 AND desired_release_id=$2",
          [r.source_id, r.id],
        );
      }
      const updated = (
        await db.query(
          "UPDATE webdock_auth.git_release SET status=$2,operation_id=coalesce($3,operation_id),provider_deployment_id=coalesce($4,provider_deployment_id),revision=revision+1,lease_until=CASE WHEN $2='deploying' THEN lease_until ELSE NULL END WHERE id=$1 RETURNING *",
          [
            r.id,
            status,
            result.operationID ?? null,
            result.providerDeploymentID ?? null,
          ],
        )
      ).rows[0];
      const artifact = (
        await db.query(
          "SELECT build_id FROM webdock_auth.git_artifact WHERE id=$1",
          [r.artifact_id],
        )
      ).rows[0];
      await enqueueGitCheck(
        db,
        artifact.build_id,
        status === "ready" || status === "failed" ? "completed" : "in_progress",
        status === "ready"
          ? "success"
          : status === "failed"
            ? "failure"
            : undefined,
      );
      return releaseView(updated);
    });
  }
  async function inspectGitBuildLease(
    credential: string,
    buildID: string,
    generation: number,
  ) {
    return dependencies.transaction(async (db) => {
      const w = await authenticateGitWorker(db, credential);
      const b = (
        await db.query(
          "SELECT b.*,s.repository_id,s.root_directory,s.recipe,s.target_id,s.build_provider,s.workflow_path,s.artifact_prefix,s.branch,c.installation_id FROM webdock_auth.git_build b JOIN webdock_auth.git_source s ON s.id=b.source_id JOIN webdock_auth.git_connection c ON c.id=s.connection_id WHERE b.id=$1 AND b.generation=$2 AND b.worker_id=$3 AND b.worker_generation=$4 AND b.status='running' AND b.lease_until>now() AND c.state='active' AND c.generation=b.connection_generation AND s.revision=b.source_revision",
          [buildID, generation, w.id, w.generation],
        )
      ).rows[0];
      if (!b) throw conflict();
      return b;
    });
  }
  /** Best-effort delivery with durable retries; external writes occur outside SQL transactions. */
  async function processGitChecks(
    provider?: ReturnType<Dependencies["createGitHubProvider"]>,
  ) {
    const row = await dependencies.transaction(async (db) => {
      const pending = (
        await db.query(
          "SELECT * FROM webdock_auth.git_check_outbox WHERE version>processed_version AND retry_at<=now() AND (lease_until IS NULL OR lease_until<now()) ORDER BY retry_at FOR UPDATE SKIP LOCKED LIMIT 1",
        )
      ).rows[0];
      if (!pending) return null;
      return (
        await db.query(
          "UPDATE webdock_auth.git_check_outbox SET generation=generation+1,lease_until=now()+interval '1 minute' WHERE build_id=$1 RETURNING *",
          [pending.build_id],
        )
      ).rows[0];
    });
    if (!row) return { processed: false };
    try {
      const result = await (
        provider ?? dependencies.createGitHubProvider()
      ).reportCheck({
        installationID: row.installation_id,
        repositoryID: row.repository_id,
        sha: row.source_sha,
        externalID: "webdock-build:" + row.build_id,
        checkID: row.check_id ?? undefined,
        status: row.status,
        conclusion: row.conclusion ?? undefined,
      });
      await dependencies.transaction(async (db) => {
        await db.query(
          "UPDATE webdock_auth.git_check_outbox SET check_id=$3,processed_version=$4,lease_until=NULL WHERE build_id=$1 AND generation=$2",
          [row.build_id, row.generation, result.checkID, row.version],
        );
      });
      return { processed: true };
    } catch {
      await dependencies.transaction(async (db) => {
        await db.query(
          "UPDATE webdock_auth.git_check_outbox SET lease_until=NULL,retry_at=now()+interval '1 minute' WHERE build_id=$1 AND generation=$2",
          [row.build_id, row.generation],
        );
      });
      return { processed: false };
    }
  }
  return {
    gitEnvironmentSnapshot,
    registerGitConnection,
    executeGitDeployment,
    enrollGitWorker,
    authenticateGitWorker,
    claimGitBuild,
    prepareGitActionsArtifact,
    completeGitBuild,
    receiveGitEvent,
    processGitEvents,
    claimGitRelease,
    completeGitRelease,
    inspectGitBuildLease,
    processGitChecks,
  };
}
