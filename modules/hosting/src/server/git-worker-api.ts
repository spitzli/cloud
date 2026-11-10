import type { HostingPorts } from "./ports";
import { createHash } from "node:crypto";
import { z } from "zod";
import {
  HostingError,
  hostingError,
  resourceID,
} from "@spitzli/hosting-contracts";
export type Dependencies = {
  withDatabaseScope: HostingPorts["withDatabaseScope"];
  authDatabase: HostingPorts["authDatabase"];
  transaction: ReturnType<typeof import("./db").createHostingDb>["transaction"];
  readHostingJSON: ReturnType<
    typeof import("./http").createHostingHttp
  >["readHostingJSON"];
  requireHostingEnvironment: ReturnType<
    typeof import("./http").createHostingHttp
  >["requireHostingEnvironment"];
  authenticateGitWorker: ReturnType<
    typeof import("./git-deployments").createHostingGitDeployments
  >["authenticateGitWorker"];
  claimGitBuild: ReturnType<
    typeof import("./git-deployments").createHostingGitDeployments
  >["claimGitBuild"];
  completeGitBuild: ReturnType<
    typeof import("./git-deployments").createHostingGitDeployments
  >["completeGitBuild"];
  claimGitRelease: ReturnType<
    typeof import("./git-deployments").createHostingGitDeployments
  >["claimGitRelease"];
  completeGitRelease: ReturnType<
    typeof import("./git-deployments").createHostingGitDeployments
  >["completeGitRelease"];
  inspectGitBuildLease: ReturnType<
    typeof import("./git-deployments").createHostingGitDeployments
  >["inspectGitBuildLease"];
  prepareGitActionsArtifact: ReturnType<
    typeof import("./git-deployments").createHostingGitDeployments
  >["prepareGitActionsArtifact"];
  processGitEvents: ReturnType<
    typeof import("./git-deployments").createHostingGitDeployments
  >["processGitEvents"];
  processGitChecks: ReturnType<
    typeof import("./git-deployments").createHostingGitDeployments
  >["processGitChecks"];
  createGitHubProvider: HostingPorts["createGitHubProvider"];
  prepareGitVercelBuild: ReturnType<
    typeof import("./git-vercel").createHostingGitVercel
  >["prepareGitVercelBuild"];
  resolveGitVercelCredential: ReturnType<
    typeof import("./git-vercel").createHostingGitVercel
  >["resolveGitVercelCredential"];
  prepareGitPublication: ReturnType<
    typeof import("./git-publication").createHostingGitPublication
  >["prepareGitPublication"];
  publishGitContainer: ReturnType<
    typeof import("./git-publication").createHostingGitPublication
  >["publishGitContainer"];
  observeGitPublication: ReturnType<
    typeof import("./git-publication").createHostingGitPublication
  >["observeGitPublication"];
  recordGitVercelPublication: ReturnType<
    typeof import("./git-publication").createHostingGitPublication
  >["recordGitVercelPublication"];
};

export function createHostingGitWorkerApi(dependencies: Dependencies) {
  const generation = z.number().int().positive().max(2147483647);
  const release = z.object({ releaseID: resourceID, generation }).strict();
  const completeBuild = z
    .object({
      buildID: resourceID,
      generation,
      status: z.enum(["succeeded", "failed"]),
      logs: z.string().max(262144),
      failureCode: z
        .string()
        .regex(/^[A-Z0-9_]{1,80}$/)
        .optional(),
      artifact: z
        .object({
          kind: z.enum(["oci", "vercel"]),
          digest: z.string().regex(/^sha256:[a-f0-9]{64}$/),
          storageKey: z.string().max(240),
          sizeBytes: z.number().int().positive().max(10737418240),
        })
        .strict()
        .optional(),
    })
    .strict();
  const completeRelease = release
    .extend({
      status: z.enum(["deploying", "ready", "failed", "needs-reconciliation"]),
      operationID: resourceID.optional(),
      providerDeploymentID: z
        .string()
        .regex(/^[A-Za-z0-9_-]{1,200}$/)
        .optional(),
      healthVerified: z.boolean().optional(),
      region: z.literal("fra1").optional(),
    })
    .strict();
  async function gitWorkerAPI(request: Request, path: string[]) {
    try {
      dependencies.requireHostingEnvironment();
      const credential = request.headers
        .get("authorization")
        ?.match(/^Bearer ([A-Za-z0-9_-]{43})$/)?.[1];
      if (!credential) throw new HostingError(401, "Invalid build worker.");
      if (
        !(
          await dependencies.authDatabase.query(
            "SELECT 1 FROM webdock_auth.git_worker WHERE credential_hash=$1 AND enabled",
            [createHash("sha256").update(credential).digest("hex")],
          )
        ).rowCount
      )
        throw new HostingError(401, "Invalid build worker.");
      return await dependencies.withDatabaseScope(
        { kind: "system", purpose: "git-worker" },
        async () => {
          let data: unknown;
          if (
            request.method === "GET" &&
            path.length === 3 &&
            path[0] === "builds" &&
            ["source", "lease"].includes(path[2])
          ) {
            const query = new URL(request.url).searchParams;
            if (
              query.size !== 1 ||
              query.getAll("generation").length !== 1 ||
              !/^[1-9][0-9]{0,9}$/.test(query.get("generation") ?? "")
            )
              throw new HostingError(400, "Invalid build lease.");
            const build = await dependencies.inspectGitBuildLease(
              credential,
              resourceID.parse(path[1]),
              generation.parse(Number(query.get("generation"))),
            );
            if (
              path[2] === "source" &&
              build.build_provider === "github-actions"
            )
              throw new HostingError(
                403,
                "Actions imports cannot request source archives.",
              );
            data =
              path[2] === "lease"
                ? { active: true }
                : await dependencies
                    .createGitHubProvider()
                    .prepareSourceDownload(
                      build.installation_id,
                      build.repository_id,
                      build.source_sha,
                    );
          } else {
            if (request.method !== "POST" || path.length !== 1)
              throw new HostingError(
                404,
                "Build worker endpoint is unavailable.",
              );
            const input = await dependencies.readHostingJSON(request, 400000);
            switch (path[0]) {
              case "claim": {
                z.object({}).strict().parse(input);
                await dependencies.processGitEvents().catch(() => undefined);
                await dependencies.processGitChecks().catch(() => undefined);
                const job = await dependencies.claimGitBuild(credential);
                let actionsArtifact;
                if (job?.buildProvider === "github-actions") {
                  try {
                    const build = await dependencies.inspectGitBuildLease(
                      credential,
                      job.buildID,
                      job.generation,
                    );
                    if (Object.keys(job.buildEnvironment).length)
                      throw new HostingError(
                        422,
                        "External build variables are unsupported.",
                      );
                    actionsArtifact =
                      await dependencies.prepareGitActionsArtifact(build);
                  } catch {
                    await dependencies.completeGitBuild(credential, {
                      buildID: job.buildID,
                      generation: job.generation,
                      status: "failed",
                      logs: "Actions artifact identity could not be verified.",
                      failureCode: "ACTIONS_ARTIFACT_UNAVAILABLE",
                    });
                    data = null;
                    break;
                  }
                }
                if (job?.recipe === "vercel") {
                  try {
                    const build = await dependencies.inspectGitBuildLease(
                      credential,
                      job.buildID,
                      job.generation,
                    );
                    const prepared = await dependencies.transaction(
                      async (db) => {
                        const provider =
                          await dependencies.resolveGitVercelCredential(db, {
                            customerID: job.customerID,
                            projectID: job.projectID,
                            targetID: build.target_id,
                          });
                        const result = await dependencies.prepareGitVercelBuild(
                          provider,
                          {
                            buildEnvironment: job.buildEnvironment,
                            allowedBuildVariables: Object.keys(
                              job.buildEnvironment,
                            ),
                          },
                        );
                        const updated = await db.query(
                          "UPDATE webdock_auth.git_build SET provider_configuration_identity=$3 WHERE id=$1 AND generation=$2 AND status='running' AND lease_until>now()",
                          [
                            job.buildID,
                            job.generation,
                            result.configurationChecksum,
                          ],
                        );
                        if (!updated.rowCount)
                          throw new HostingError(409, "Build lease expired.");
                        return result;
                      },
                    );
                    data = {
                      ...job,
                      actionsArtifact,
                      vercelSettings: prepared.vercelSettings,
                    };
                  } catch {
                    await dependencies.completeGitBuild(credential, {
                      buildID: job.buildID,
                      generation: job.generation,
                      status: "failed",
                      logs: "Vercel build configuration could not be verified. Review the target setup.",
                      failureCode: "VERCEL_SETUP_REQUIRED",
                    });
                    data = null;
                  }
                } else data = job ? { ...job, actionsArtifact } : null;
                break;
              }
              case "complete":
                data = await dependencies.completeGitBuild(
                  credential,
                  completeBuild.parse(input),
                );
                break;
              case "release-claim":
                z.object({}).strict().parse(input);
                data = await dependencies.claimGitRelease(credential);
                break;
              case "retention": {
                z.object({}).strict().parse(input);
                data = await dependencies.transaction(async (db) => {
                  const worker = await dependencies.authenticateGitWorker(
                    db,
                    credential,
                  );
                  const projects = (
                    await db.query(
                      "SELECT DISTINCT project_id FROM webdock_auth.git_build WHERE worker_id=$1 ORDER BY project_id",
                      [worker.id],
                    )
                  ).rows;
                  for (const project of projects) {
                    await db.query(
                      "SELECT webdock_admin.lock_hosting_project($1)",
                      [project.project_id],
                    );
                    await db.query(
                      "SELECT id FROM webdock_auth.git_source WHERE project_id=$1 FOR UPDATE",
                      [project.project_id],
                    );
                  }
                  await db.query(
                    "DELETE FROM webdock_auth.git_flow WHERE expires_at<now()",
                  );
                  await db.query(
                    "UPDATE webdock_auth.git_build SET logs='' WHERE worker_id=$1 AND finished_at<now()-interval '30 days' AND logs<>''",
                    [worker.id],
                  );
                  await db.query(
                    `UPDATE webdock_auth.git_artifact a SET retained=false FROM webdock_auth.git_build b
              WHERE a.build_id=b.id AND b.worker_id=$1 AND a.created_at<now()-interval '30 days' AND a.retained
              AND NOT EXISTS(SELECT 1 FROM webdock_auth.git_release r WHERE r.artifact_id=a.id AND (r.status IN ('queued','deploying','needs-reconciliation','awaiting-approval') OR r.id IN (SELECT desired_release_id FROM webdock_auth.git_source)))
              AND a.id NOT IN (SELECT artifact_id FROM (SELECT artifact_id,row_number() OVER(PARTITION BY source_id ORDER BY created_at DESC) AS position FROM webdock_auth.git_release WHERE status='ready') good WHERE position<=3)`,
                    [worker.id],
                  );
                  const rows = (
                    await db.query(
                      "SELECT a.storage_key FROM webdock_auth.git_artifact a JOIN webdock_auth.git_build b ON b.id=a.build_id WHERE b.worker_id=$1 AND NOT a.retained AND a.storage_purged_at IS NULL ORDER BY a.created_at LIMIT 1000",
                      [worker.id],
                    )
                  ).rows;
                  return { storageKeys: rows.map((row) => row.storage_key) };
                });
                break;
              }
              case "retention-complete": {
                const result = z
                  .object({
                    storageKeys: z
                      .array(
                        z
                          .string()
                          .regex(
                            /^[1-9][0-9]*\/[1-9][0-9]*\/[1-9][0-9]*\/[a-f0-9]{64}\.tar$/,
                          ),
                      )
                      .max(1000),
                  })
                  .strict()
                  .parse(input);
                data = await dependencies.transaction(async (db) => {
                  const worker = await dependencies.authenticateGitWorker(
                    db,
                    credential,
                  );
                  await db.query(
                    "UPDATE webdock_auth.git_artifact a SET storage_purged_at=now() FROM webdock_auth.git_build b WHERE a.build_id=b.id AND b.worker_id=$1 AND NOT a.retained AND a.storage_key=ANY($2::text[])",
                    [worker.id, result.storageKeys],
                  );
                  return { acknowledged: true };
                });
                break;
              }
              case "orphan-retention": {
                const candidates = z
                  .object({
                    storageKeys: z
                      .array(
                        z
                          .string()
                          .regex(
                            /^[1-9][0-9]*\/[1-9][0-9]*\/[1-9][0-9]*\/[a-f0-9]{64}\.tar$/,
                          ),
                      )
                      .max(1000),
                  })
                  .strict()
                  .parse(input);
                data = await dependencies.transaction(async (db) => {
                  await dependencies.authenticateGitWorker(db, credential);
                  const rows = (
                    await db.query(
                      `SELECT candidate AS storage_key FROM unnest($1::text[]) candidate
              JOIN webdock_auth.git_build b ON b.id=split_part(candidate,'/',2) AND b.customer_id=split_part(candidate,'/',1)
              WHERE b.created_at<now()-interval '24 hours'
              AND NOT EXISTS(SELECT 1 FROM webdock_auth.git_artifact a WHERE a.storage_key=candidate)
              AND NOT (b.generation::text=split_part(candidate,'/',3) AND b.status='running' AND b.lease_until>now())`,
                      [candidates.storageKeys],
                    )
                  ).rows;
                  return { storageKeys: rows.map((row) => row.storage_key) };
                });
                break;
              }
              case "release-complete": {
                const result = completeRelease.parse(input);
                if (result.status === "ready") {
                  const observed = await dependencies.observeGitPublication(
                    credential,
                    result.releaseID,
                    result.generation,
                  );
                  if (observed.status !== "ready")
                    throw new HostingError(
                      409,
                      "Publication requires observed health and verified target location.",
                    );
                  data = await dependencies.completeGitRelease(
                    credential,
                    completeRelease.parse({
                      releaseID: result.releaseID,
                      generation: result.generation,
                      ...observed,
                    }),
                  );
                } else
                  data = await dependencies.completeGitRelease(
                    credential,
                    result,
                  );
                break;
              }
              case "release-prepare": {
                const r = release.parse(input);
                data = await dependencies.prepareGitPublication(
                  credential,
                  r.releaseID,
                  r.generation,
                );
                break;
              }
              case "container-publish":
                data = await dependencies.publishGitContainer(
                  credential,
                  release
                    .extend({ image: z.string().max(512) })
                    .strict()
                    .parse(input),
                );
                break;
              case "publication-record":
                data = await dependencies.recordGitVercelPublication(
                  credential,
                  release
                    .extend({ providerURL: z.string().url().max(512) })
                    .strict()
                    .parse(input),
                );
                break;
              case "observe": {
                const r = release
                  .extend({
                    providerDeploymentID: z.string().max(200).optional(),
                  })
                  .strict()
                  .parse(input);
                data = await dependencies.observeGitPublication(
                  credential,
                  r.releaseID,
                  r.generation,
                );
                break;
              }
              default:
                throw new HostingError(
                  404,
                  "Build worker endpoint is unavailable.",
                );
            }
          }
          return Response.json(data, {
            headers: { "Cache-Control": "no-store" },
          });
        },
      );
    } catch (error) {
      const safe = hostingError(error);
      return Response.json(
        { error: safe.message },
        { status: safe.status, headers: { "Cache-Control": "no-store" } },
      );
    }
  }
  return { gitWorkerAPI };
}
