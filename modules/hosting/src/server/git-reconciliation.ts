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
  createGitVercelObserver: ReturnType<
    typeof import("./git-vercel-publication").createHostingGitVercelPublication
  >["createGitVercelObserver"];
  gitReleaseHealthPath: ReturnType<
    typeof import("./git-vercel-publication").createHostingGitVercelPublication
  >["gitReleaseHealthPath"];
};
type Observation = {
  status: "ready" | "failed" | "superseded" | "needs-reconciliation";
  providerDeploymentID?: string;
  healthVerified?: boolean;
};
type Observer = (
  db: Connection,
  release: {
    id: string;
    recipe: string;
    operation_id: string | null;
    project_id: string;
    target_id: string;
    customer_id: string;
    publication_target: {
      teamID?: string;
      healthPath?: unknown;
    };
    provider_deployment_id: string | null;
    created_at: Date | string;
  },
) => Promise<Observation>;
export function createHostingGitReconciliation(dependencies: Dependencies) {
  const observe: Observer = async (db, r) => {
    if (r.recipe === "dockerfile") {
      // Registry pushes cannot alter the running app. Dispatch and operation ID are committed atomically.
      if (!r.operation_id) return { status: "failed" };
      const op = (
        await db.query(
          "SELECT o.status,o.target_revision,a.observed_revision,a.status AS app_status FROM webdock_auth.hosting_operation o JOIN webdock_auth.hosting_app a ON a.id=o.app_id WHERE o.id=$1 AND o.project_id=$2 AND a.id=$3",
          [r.operation_id, r.project_id, r.target_id],
        )
      ).rows[0];
      if (!op) return { status: "needs-reconciliation" };
      if (op.status === "failed") return { status: "failed" };
      if (
        Number.isInteger(op.target_revision) &&
        Number.isInteger(op.observed_revision) &&
        op.observed_revision > op.target_revision
      )
        return { status: "superseded" };
      if (
        op.status === "succeeded" &&
        op.app_status === "ready" &&
        op.observed_revision === op.target_revision
      )
        return { status: "ready", healthVerified: true };
      return { status: "needs-reconciliation" };
    }
    const credential = await dependencies.resolveGitVercelCredential(db, {
      customerID: r.customer_id,
      projectID: r.project_id,
      targetID: r.target_id,
    });
    if (credential.teamID !== r.publication_target.teamID)
      throw new HostingError(
        409,
        "The original Vercel target is no longer accessible.",
      );
    const provider = dependencies.createGitVercelObserver(credential);
    let deploymentID = r.provider_deployment_id;
    if (!deploymentID) {
      const found = await provider.reconcileDeployment({
        releaseID: r.id,
        since: new Date(r.created_at).getTime(),
      });
      if (found.status !== "found") return { status: "needs-reconciliation" };
      deploymentID = found.deploymentID;
    }
    const observed = await provider.observeDeployment({
      deploymentID,
      releaseID: r.id,
      healthPath: dependencies.gitReleaseHealthPath(r.publication_target),
    });
    return {
      status:
        observed.status === "ready"
          ? "ready"
          : observed.status === "failed"
            ? "failed"
            : "needs-reconciliation",
      providerDeploymentID: deploymentID,
      healthVerified: observed.status === "ready",
    };
  };
  /** Recovery only observes known intent. An absent provider record never authorizes another write. */
  async function reconcileGitRelease(
    actor: HostingActor,
    cmd: Extract<
      GitDeploymentCommand,
      {
        action: "git.releases.reconcile";
      }
    >,
    observer: Observer = observe,
  ) {
    return dependencies.transaction(async (db) => {
      const access = await dependencies.authorizeHosting(
        actor,
        {
          permission: "org:deployments:create",
          projectID: cmd.projectID,
          write: true,
        },
        db,
      );
      const r = (
        await db.query(
          "SELECT r.*,a.build_id FROM webdock_auth.git_release r JOIN webdock_auth.git_artifact a ON a.id=r.artifact_id WHERE r.id=$1 AND r.project_id=$2 AND r.customer_id=$3 FOR UPDATE OF r",
          [cmd.releaseID, cmd.projectID, access.customerID],
        )
      ).rows[0];
      if (!r) throw new HostingError(404, "Git deployment is unavailable.");
      if (r.status !== "needs-reconciliation" || r.revision !== cmd.revision)
        throw new HostingError(
          409,
          "Git deployment changed. Review the source and release before reconciliation.",
        );
      // Historical observation grants no publication rights and uses only the immutable target.
      const target = r.publication_target;
      if (
        !target ||
        (target.recipe === "dockerfile"
          ? !/^[1-9][0-9]{0,18}$/.test(target.targetID)
          : target.recipe !== "vercel" ||
            !/^prj_[A-Za-z0-9]+$/.test(target.targetID) ||
            !/^team_[A-Za-z0-9]+$/.test(target.teamID))
      )
        throw new HostingError(
          409,
          "The original publication target could not be verified.",
        );
      const outcome = await observer(db, {
        ...r,
        recipe: target.recipe,
        target_id: target.targetID,
      });
      if (outcome.status === "ready" && !outcome.healthVerified)
        throw new HostingError(409, "Release health has not been verified.");
      if (outcome.status === "needs-reconciliation")
        return { id: r.id, status: r.status, revision: r.revision };
      const updated = (
        await db.query(
          "UPDATE webdock_auth.git_release SET status=$2,provider_deployment_id=coalesce($3,provider_deployment_id),revision=revision+1,lease_until=NULL WHERE id=$1 RETURNING id,status,revision",
          [r.id, outcome.status, outcome.providerDeploymentID ?? null],
        )
      ).rows[0];
      await db.query(
        "UPDATE webdock_auth.git_check_outbox SET status='completed',conclusion=$2,version=version+1,retry_at=now() WHERE build_id=$1",
        [
          r.build_id,
          outcome.status === "ready"
            ? "success"
            : outcome.status === "superseded"
              ? "cancelled"
              : "failure",
        ],
      );
      await dependencies.audit(db, actor, cmd.action, r.id, outcome.status);
      return updated;
    });
  }
  return { reconcileGitRelease };
}
