import type { DatabaseServices } from "@spitzli/database-contracts/runtime-ports";
import type { HostingPorts } from "./ports";
import { createHash } from "node:crypto";
import { z } from "zod";
import {
  HostingError,
  hostingError,
  resourceID,
} from "@spitzli/hosting-contracts";
export type Dependencies = {
  guardOperation?: (agent: import("./clusters").Agent, input: {id:string;generation:number}) => Promise<{valid:true}>;
  templateAgentRequest?: (agent: import("./clusters").Agent, action: string, input: unknown) => Promise<unknown>;
  withDatabaseScope: HostingPorts["withDatabaseScope"];
  authDatabase: HostingPorts["authDatabase"];
  databaseObservationConfig: DatabaseServices["databaseObservationConfig"];
  reportDatabaseObservations: DatabaseServices["reportDatabaseObservations"];
  claimDatabaseOperation: DatabaseServices["claimDatabaseOperation"];
  checkpointDatabaseOperation: DatabaseServices["checkpointDatabaseOperation"];
  completeDatabaseOperation: DatabaseServices["completeDatabaseOperation"];
  guardDatabaseOperation: DatabaseServices["guardDatabaseOperation"];
  claimMailConfiguration: HostingPorts["claimMailConfiguration"];
  checkpointMailConfiguration: HostingPorts["checkpointMailConfiguration"];
  completeMailConfiguration: HostingPorts["completeMailConfiguration"];
  claimNativeMail: HostingPorts["claimNativeMail"];
  checkpointNativeMail: HostingPorts["checkpointNativeMail"];
  completeNativeMail: HostingPorts["completeNativeMail"];
  saveInventory: ReturnType<
    typeof import("./byok").createHostingByok
  >["saveInventory"];
  byokAgentConfig: ReturnType<
    typeof import("./byok").createHostingByok
  >["byokAgentConfig"];
  claimByokOperation: ReturnType<
    typeof import("./byok").createHostingByok
  >["claimByokOperation"];
  completeByokOperation: ReturnType<
    typeof import("./byok").createHostingByok
  >["completeByokOperation"];
  claimOperation: ReturnType<
    typeof import("./operations").createHostingOperations
  >["claimOperation"];
  completeOperation: ReturnType<
    typeof import("./operations").createHostingOperations
  >["completeOperation"];
  database: HostingPorts["database"];
  authenticateAgent: ReturnType<
    typeof import("./clusters").createHostingClusters
  >["authenticateAgent"];
  consumeEnrollment: ReturnType<
    typeof import("./clusters").createHostingClusters
  >["consumeEnrollment"];
  reportCluster: ReturnType<
    typeof import("./clusters").createHostingClusters
  >["reportCluster"];
  requireHostingEnvironment: ReturnType<
    typeof import("./http").createHostingHttp
  >["requireHostingEnvironment"];
  readHostingJSON: ReturnType<
    typeof import("./http").createHostingHttp
  >["readHostingJSON"];
};

export function createHostingAgentApi(dependencies: Dependencies) {
  const enrollment = z
    .object({ clusterID: resourceID, token: z.string().length(43) })
    .strict();
  async function hostingAgentAPI(request: Request, path: string[]) {
    try {
      dependencies.requireHostingEnvironment();
      if (
        request.method !== "POST" ||
        path.length !== 1 ||
        ![
          "guard",
          "template-claim",
          "template-checkpoint",
          "template-complete",
          "database-config",
          "database-observation",
          "database-claim",
          "database-checkpoint",
          "database-complete",
          "database-guard",
          "enroll",
          "heartbeat",
          "claim",
          "complete",
          "inventory",
          "config",
          "byok-claim",
          "byok-complete",
          "mail-claim",
          "mail-checkpoint",
          "mail-complete",
          "mail-configuration-claim",
          "mail-configuration-checkpoint",
          "mail-configuration-complete",
        ].includes(path[0])
      )
        throw new HostingError(404, "Agent endpoint is unavailable.");
      const input = await dependencies.readHostingJSON(
        request,
        path[0] === "inventory"
          ? 2000000
          : path[0] === "database-checkpoint"
            ? 300000
            : 65536,
      );
      const credentials = request.headers
        .get("authorization")
        ?.match(/^Bearer ([A-Za-z0-9_-]{43})$/)?.[1];
      const clusterID = request.headers.get("x-webdock-cluster");
      // A bounded, shared window also limits attempts with freshly invented credentials.
      const agent =
        path[0] !== "enroll" && clusterID && credentials
          ? await dependencies.authenticateAgent(clusterID, credentials)
          : null;
      if (path[0] !== "enroll" && !agent)
        throw new HostingError(401, "Cluster authentication failed.");
      const enrollmentInput =
        path[0] === "enroll" ? enrollment.parse(input) : null;
      if (
        enrollmentInput &&
        !(
          await dependencies.authDatabase.query(
            "SELECT 1 FROM webdock_auth.hosting_enrollment WHERE cluster_id=$1 AND token_hash=$2 AND consumed_at IS NULL AND expires_at>now()",
            [
              enrollmentInput.clusterID,
              createHash("sha256").update(enrollmentInput.token).digest("hex"),
            ],
          )
        ).rowCount
      )
        throw new HostingError(401, "Enrollment is invalid.");
      return await dependencies.withDatabaseScope(
        {
          kind: "system",
          purpose: enrollmentInput ? "agent-enrollment" : "agent",
          clusterID: enrollmentInput?.clusterID ?? agent!.clusterID,
        },
        async () => {
          const bucket =
            path[0] === "enroll"
              ? "enrollment"
              : `heartbeat:${resourceID.parse(clusterID)}`;
          const hash = createHash("sha256").update(bucket).digest("hex");
          const r = (
            await dependencies.database.query(
              `INSERT INTO webdock_auth.hosting_rate(key_hash) VALUES($1) ON CONFLICT(key_hash) DO UPDATE SET requests=CASE WHEN hosting_rate.window_start<now()-interval '1 minute' THEN 1 ELSE hosting_rate.requests+1 END,window_start=CASE WHEN hosting_rate.window_start<now()-interval '1 minute' THEN now() ELSE hosting_rate.window_start END RETURNING requests`,
              [hash],
            )
          ).rows[0];
          if (r.requests > (path[0] === "enroll" ? 30 : 120))
            throw new HostingError(429, "Agent request rate exceeded.");
          let data;
          if (path[0] === "enroll") {
            const parsed = enrollment.parse(input);
            data = await dependencies.consumeEnrollment(
              parsed.clusterID,
              parsed.token,
            );
          } else {
            if (!credentials || !clusterID)
              throw new HostingError(401, "Cluster authentication failed.");
            if (path[0] === "guard") {
              if (!dependencies.guardOperation) throw new HostingError(404, "Operation guard is unavailable.");
              data = await dependencies.guardOperation(agent!, z.object({id:resourceID,generation:z.number().int().positive()}).strict().parse(input));
            } else if (path[0].startsWith("template-")) {
              if (!dependencies.templateAgentRequest) throw new HostingError(404, "Approved application templates are unavailable.");
              data = await dependencies.templateAgentRequest(agent!, path[0], input);
            } else if (path[0] === "database-config") {
              z.object({}).strict().parse(input);
              data = await dependencies.databaseObservationConfig(agent!);
            } else if (path[0] === "database-observation")
              data = await dependencies.reportDatabaseObservations(
                agent!,
                input,
              );
            else if (path[0] === "database-claim") {
              z.object({}).strict().parse(input);
              data = await dependencies.claimDatabaseOperation(agent!);
            } else if (path[0] === "database-checkpoint")
              data = await dependencies.checkpointDatabaseOperation(
                agent!,
                input,
              );
            else if (path[0] === "database-guard")
              data = await dependencies.guardDatabaseOperation(agent!, input);
            else if (path[0] === "database-complete")
              data = await dependencies.completeDatabaseOperation(
                agent!,
                input,
              );
            else if (path[0] === "mail-configuration-claim") {
              z.object({}).strict().parse(input);
              data = await dependencies.claimMailConfiguration(agent!);
            } else if (path[0] === "mail-configuration-checkpoint")
              data = await dependencies.checkpointMailConfiguration(
                agent!,
                input,
              );
            else if (path[0] === "mail-configuration-complete")
              data = await dependencies.completeMailConfiguration(
                agent!,
                input,
              );
            else if (path[0] === "mail-claim") {
              z.object({}).strict().parse(input);
              data = await dependencies.claimNativeMail(agent!);
            } else if (path[0] === "mail-checkpoint")
              data = await dependencies.checkpointNativeMail(agent!, input);
            else if (path[0] === "mail-complete")
              data = await dependencies.completeNativeMail(agent!, input);
            else if (path[0] === "inventory")
              data = await dependencies.saveInventory(agent!, input);
            else if (path[0] === "config") {
              z.object({}).strict().parse(input);
              data = await dependencies.byokAgentConfig(agent!);
            } else if (path[0] === "byok-claim") {
              z.object({}).strict().parse(input);
              data = await dependencies.claimByokOperation(agent!);
            } else if (path[0] === "byok-complete")
              data = await dependencies.completeByokOperation(
                agent!,
                z
                  .object({
                    id: resourceID,
                    generation: z.number().int().positive(),
                    outcome: z.enum(["succeeded", "failed"]),
                    proof: z.unknown().optional(),
                  })
                  .strict()
                  .parse(input),
              );
            else if (path[0] === "heartbeat")
              data = await dependencies.reportCluster(agent!, input);
            else if (path[0] === "claim") {
              z.object({}).strict().parse(input);
              data = await dependencies.claimOperation(agent!);
            } else
              data = await dependencies.completeOperation(
                agent!,
                z
                  .object({
                    id: resourceID,
                    generation: z.number().int().positive(),
                    outcome: z.enum(["succeeded", "failed"]),
                    proof: z.unknown().optional(),
                    error: z
                      .enum([
                        "admission_rejected",
                        "rollout_timeout",
                        "ownership_conflict",
                        "execution_failed",
                      ])
                      .optional(),
                  })
                  .strict()
                  .parse(input),
              );
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
  return { hostingAgentAPI };
}
