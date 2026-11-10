import type { HostingPorts } from "./ports";
import type { PoolClient } from "pg";
import {
  HostingError,
  resourceID,
  type HostingActor,
} from "@spitzli/hosting-contracts";
export type Dependencies = {
  currentDatabaseScope: HostingPorts["currentDatabaseScope"];
  identity: HostingPorts["identity"];
  currentConsolePrincipal: HostingPorts["currentConsolePrincipal"];
  organizationPermission: HostingPorts["organizationPermission"];
};
export type Connection = Pick<PoolClient, "query">;
export function createHostingAuthorization(dependencies: Dependencies) {
  async function authorizeHosting(
    actor: HostingActor,
    target: {
      customerID?: string;
      projectID?: string;
      write?: boolean;
      operator?: boolean;
      live?: boolean;
      tenantAdmin?: boolean;
      permission?: string;
      legacyRole?: boolean;
    },
    db: Connection,
  ) {
    const scope = dependencies.currentDatabaseScope();
    if (scope?.kind === "system" && scope.purpose === "hosting-setup") {
      const live = (
        await db.query(
          "SELECT 1 FROM webdock_auth.hosting_setup WHERE id=$1 AND generation=$2 AND lease_until>now() AND state='running' AND actor->>'subject'=$3 AND actor->>'sessionID'=$4 AND ($5::text IS NULL OR project_id=$5) AND ($6::text IS NULL OR customer_id=$6)",
          [
            scope.setupID,
            scope.generation,
            actor.subject,
            actor.sessionID,
            target.projectID ?? null,
            target.customerID ?? null,
          ],
        )
      ).rowCount;
      if (!live)
        throw new HostingError(
          409,
          "Setup authorization changed. Sign in and resume the setup.",
        );
    }
    const principal = dependencies.currentConsolePrincipal();
    if (
      principal &&
      (principal.userID !== actor.subject ||
        principal.sessionID !== actor.sessionID)
    )
      throw new HostingError(403, "Identity mismatch.");
    const scopes = actor.scopes;
    const policy = actor.source === "git-policy";
    if (
      policy &&
      (!target.projectID ||
        target.operator ||
        target.live ||
        actor.sessionID !== "" ||
        !actor.gitSourceID ||
        !actor.gitSourceRevision)
    )
      throw new HostingError(403, "Git publication policy is unavailable.");
    if (
      !scopes.includes("hosting:read") ||
      (target.write && !scopes.includes("hosting:write"))
    )
      throw new HostingError(403, "Hosting scope is required.");
    const row = policy
      ? await dependencies.identity.subject(db, actor.subject, { lock: true })
      : await dependencies.identity.session(
          db,
          { subject: actor.subject, sessionID: actor.sessionID },
          { lock: true },
        );
    if (
      policy &&
      !(
        await db.query(
          `SELECT id FROM webdock_auth.git_source WHERE policy_subject=$1 AND project_id=$2 AND id=$3 AND revision=$4 AND policy_revision=revision AND auto_publish AND enabled FOR SHARE`,
          [
            actor.subject,
            target.projectID,
            actor.gitSourceID,
            actor.gitSourceRevision,
          ],
        )
      ).rowCount
    )
      throw new HostingError(401, "Sign in and complete account setup.");
    if (
      row?.subject !== actor.subject ||
      (!policy &&
        (!row || !("sessionID" in row) || row.sessionID !== actor.sessionID))
    )
      throw new HostingError(401, "Sign in and complete account setup.");
    if (!row || row.banned || !row.emailVerified || row.mustChangePassword)
      throw new HostingError(401, "Sign in and complete account setup.");
    const operator =
      row.role === "operator" &&
      (row.twoFactorEnabled === true || row.operatorAuthorized === true);
    if (!operator && row.role !== "user")
      throw new HostingError(403, "Complete operator security setup.");
    const preview = (
      await db.query(
        principal
          ? "SELECT customer_id,organization_id,expires_at,expired_at FROM webdock_auth.console_tenant_preview WHERE session_id=$1 AND actor_id=$2 AND issuer=$3"
          : "SELECT customer_id,organization_id,expires_at,expired_at FROM webdock_auth.studio_tenant_preview WHERE session_id=$1 AND actor_id=$2",
        [
          actor.sessionID,
          actor.subject,
          ...(principal ? [principal.issuer] : []),
        ],
      )
    ).rows[0];
    if (
      preview &&
      (target.write ||
        target.operator ||
        target.live ||
        preview.expired_at ||
        new Date(preview.expires_at).getTime() <= Date.now())
    )
      throw new HostingError(403, "Customer preview is read-only.");
    if (target.operator && (!operator || preview))
      throw new HostingError(403, "Platform operator access is required.");
    let customerID = target.customerID
      ? resourceID.parse(target.customerID)
      : undefined;
    let project;
    if (target.projectID) {
      await db.query("SELECT webdock_admin.lock_hosting_project($1)", [
        resourceID.parse(target.projectID),
      ]);
      project = (
        await db.query(
          "SELECT p.id,p.customer_id,p.status,h.mode,h.provider,h.cluster_id,h.revision,h.own_images FROM webdock_admin.projects p LEFT JOIN webdock_auth.hosting_project h ON h.project_id=p.id WHERE p.id=$1",
          [resourceID.parse(target.projectID)],
        )
      ).rows[0];
      if (
        !project ||
        project.status !== "active" ||
        (customerID && customerID !== project.customer_id)
      )
        throw new HostingError(404, "Hosting target is unavailable.");
      customerID = project.customer_id;
    }
    let membership;
    let canWrite = false;
    if (customerID) {
      const customer = (
        await db.query(
          `SELECT c.status,t.organization_id FROM webdock_admin.customers c JOIN webdock_auth.tenant_customer t ON t.customer_id=c.id WHERE c.id=$1 FOR SHARE OF c`,
          [customerID],
        )
      ).rows[0];
      if (
        !customer ||
        customer.status !== "active" ||
        (preview &&
          (preview.customer_id !== customerID ||
            preview.organization_id !== customer.organization_id))
      )
        throw new HostingError(404, "Hosting target is unavailable.");
      const mapped = await dependencies.identity.membership(
        db,
        actor.subject,
        customerID,
        { lock: true },
      );
      membership = mapped?.role ? mapped : undefined;
      if (!operator && !membership)
        throw new HostingError(404, "Hosting target is unavailable.");
      const permission =
        target.permission && !operator
          ? await dependencies.organizationPermission(
              actor.subject,
              customer.organization_id,
              target.permission,
            )
          : undefined;
      canWrite =
        operator ||
        (target.permission
          ? (permission ?? ["admin", "owner"].includes(membership?.role ?? ""))
          : target.legacyRole === true &&
            ["admin", "owner"].includes(membership?.role ?? ""));
      if (
        target.write &&
        !operator &&
        (!canWrite || (!target.tenantAdmin && project?.mode !== "selfservice"))
      )
        throw new HostingError(
          403,
          "This hosting action is managed by your operator.",
        );
    } else if (!operator || preview)
      throw new HostingError(403, "Choose an authorized hosting customer.");
    return {
      operator: operator && !preview,
      liveReads: !preview,
      customerID,
      project,
      canWrite: !preview && (operator || canWrite),
    };
  }
  return { authorizeHosting };
}
