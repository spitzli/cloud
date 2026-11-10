import type { HostingActor, ProjectOverview } from "@spitzli/hosting-contracts";
import type { Connection } from "./authorization";

export function createHostingProjectOverview(dependencies: {
  authorizeHosting: ReturnType<
    typeof import("./authorization").createHostingAuthorization
  >["authorizeHosting"];
  readServiceBindings: ReturnType<
    typeof import("./service-bindings").createHostingServiceBindings
  >["readServiceBindings"];
  readHostingView: ReturnType<
    typeof import("./view-cache").createHostingViewCache
  >["readHostingView"];
}) {
  const { authorizeHosting, readServiceBindings, readHostingView } =
    dependencies;
  /** Read the overview under one authorization transaction, including live binding health. */
  async function readProjectOverview(
    actor: HostingActor,
    projectID: string,
    db: Connection,
  ): Promise<ProjectOverview> {
    const access = await authorizeHosting(
      actor,
      { permission: "org:projects:manage", projectID },
      db,
    );
    const data = await readHostingView(
      db,
      actor,
      {
        kind: "projectsummary",
        scope: {
          customerID: access.customerID!,
          projectID,
          operator: access.operator,
          liveReads: access.liveReads,
        },
        query: { action: "projects.overview", projectID },
        ttlSeconds: 10,
        tables: [
          "webdock_admin.projects",
          "webdock_auth.hosting_app",
          "webdock_auth.hosting_database",
          "webdock_auth.hosting_service_binding",
          "webdock_auth.hosting_database_backend",
          "webdock_auth.hosting_setup",
          "webdock_auth.git_source",
          "webdock_auth.git_repository",
          "webdock_auth.mail_api_key",
          "webdock_auth.mail_connection",
          "webdock_auth.mail_sender_domain",
          "webdock_auth.platform_settings",
        ],
      },
      async () => {
        const project = (
          await db.query(
            "SELECT name FROM webdock_admin.projects WHERE id=$1",
            [projectID],
          )
        ).rows[0];
        const applications = (
          await db.query(
            "SELECT id,name,lifecycle,status FROM webdock_auth.hosting_app WHERE project_id=$1 AND status<>'deleted' ORDER BY name,id LIMIT 200",
            [projectID],
          )
        ).rows;
        const databaseCount = (
          await db.query(
            "SELECT count(*)::int AS count FROM webdock_auth.hosting_database WHERE project_id=$1",
            [projectID],
          )
        ).rows[0].count;
        const bindings = (
          await readServiceBindings(db, projectID, access.customerID!)
        )
          .filter((binding) => binding.state !== "disconnected")
          .map(({ id, appID, service, state }) => ({
            id,
            appID,
            service,
            state,
          }));
        const setups = (
          await db.query(
            "SELECT id,configuration->>'name' AS name,state FROM webdock_auth.hosting_setup WHERE project_id=$1 ORDER BY updated_at DESC,id DESC LIMIT 5",
            [projectID],
          )
        ).rows.filter((setup) => setup.state !== "ready");
        const sources = (
          await db.query(
            `SELECT s.target_id AS "targetID",r.metadata->>'fullName' AS "repositoryName",s.branch
      FROM webdock_auth.git_source s JOIN webdock_auth.git_repository r ON r.connection_id=s.connection_id AND r.repository_id=s.repository_id
      WHERE s.project_id=$1 AND s.customer_id=$2 ORDER BY s.id`,
            [projectID, access.customerID],
          )
        ).rows;
        return {
          project: { name: project.name },
          applications,
          databaseCount,
          bindings,
          setups,
          sources,
        };
      },
    );
    return {
      ...data,
      project: {
        ...data.project,
        configured: Boolean(access.project?.provider),
        provider: access.project?.provider ?? null,
        canManage:
          access.canWrite &&
          (access.operator || access.project?.mode === "selfservice"),
      },
    };
  }

  return { readProjectOverview };
}
