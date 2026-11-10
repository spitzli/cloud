import type { HostingPorts } from "./ports";
import {
  HostingError,
  type HostingActor,
  type GitDeploymentCommand,
} from "@spitzli/hosting-contracts";
import type { Connection } from "./authorization";
export type Dependencies = {
  authorizeHosting: ReturnType<
    typeof import("./authorization").createHostingAuthorization
  >["authorizeHosting"];
  transaction: ReturnType<typeof import("./db").createHostingDb>["transaction"];
  audit: ReturnType<typeof import("./db").createHostingDb>["audit"];
  resolveGitVercelCredential: ReturnType<
    typeof import("./git-vercel").createHostingGitVercel
  >["resolveGitVercelCredential"];
  prepareGitVercelBuild: ReturnType<
    typeof import("./git-vercel").createHostingGitVercel
  >["prepareGitVercelBuild"];
  env: HostingPorts["env"];
};
type Target = Extract<
  GitDeploymentCommand,
  {
    action: "git.targets.bind";
  }
>;
type Verify = (
  db: Connection,
  target: {
    customerID: string;
    projectID: string;
    targetID: string;
  },
) => Promise<void>;
export function createHostingGitTargets(dependencies: Dependencies) {
  const verify: Verify = async (db, target) => {
    const credential = await dependencies.resolveGitVercelCredential(
      db,
      target,
    );
    await dependencies.prepareGitVercelBuild(credential, {
      buildEnvironment: {},
      allowedBuildVariables: [],
    });
  };
  /** Explicit adoption of an already configured target, checked transactionally before binding is retained. */
  async function bindGitTarget(
    actor: HostingActor,
    cmd: Target,
    verifyTarget: Verify = verify,
  ) {
    return dependencies.transaction(async (db) => {
      const access = await dependencies.authorizeHosting(
        actor,
        {
          permission: "org:projects:manage",
          projectID: cmd.projectID,
          write: true,
          operator: cmd.mode === "platform",
        },
        db,
      );
      if (access.project?.provider !== "vercel")
        throw new HostingError(
          409,
          "Configure a Vercel hosting project first.",
        );
      let teamID: string;
      if (cmd.mode === "byok") {
        const connection = (
          await db.query(
            "SELECT team_id,projects FROM webdock_auth.hosting_vercel WHERE customer_id=$1 FOR SHARE",
            [access.customerID],
          )
        ).rows[0];
        if (!connection || !connection.projects.includes(cmd.targetID))
          throw new HostingError(
            403,
            "Select this project in the customer Vercel connection first.",
          );
        teamID = connection.team_id;
      } else {
        teamID = dependencies.env.WEBDOCK_GIT_VERCEL_TEAM_ID ?? "";
        if (!/^team_[A-Za-z0-9]+$/.test(teamID))
          throw new HostingError(
            409,
            "Configure the platform Vercel team first.",
          );
      }
      await db.query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))", [
        "git-vercel-target:" + teamID + ":" + cmd.targetID,
      ]);
      const existing = (
        await db.query(
          "SELECT * FROM webdock_auth.git_vercel_target WHERE (team_id=$1 AND target_id=$2) OR (project_id=$3 AND target_id=$2) FOR UPDATE",
          [teamID, cmd.targetID, cmd.projectID],
        )
      ).rows[0];
      if (
        existing &&
        (existing.customer_id !== access.customerID ||
          existing.project_id !== cmd.projectID ||
          existing.mode !== cmd.mode ||
          existing.team_id !== teamID)
      )
        throw new HostingError(
          409,
          "This Vercel target is already bound to another hosting project or connection.",
        );
      if (!existing)
        await db.query(
          "INSERT INTO webdock_auth.git_vercel_target(customer_id,project_id,target_id,team_id,mode) VALUES($1,$2,$3,$4,$5)",
          [access.customerID, cmd.projectID, cmd.targetID, teamID, cmd.mode],
        );
      await verifyTarget(db, {
        customerID: access.customerID!,
        projectID: cmd.projectID,
        targetID: cmd.targetID,
      });
      await dependencies.audit(db, actor, cmd.action, cmd.projectID);
      return {
        projectID: cmd.projectID,
        targetID: cmd.targetID,
        mode: cmd.mode,
        generation: existing?.generation ?? 1,
      };
    });
  }
  return { bindGitTarget };
}
