import { getHostingApplication } from "@/lib/hosting-application";
import { getManagedDatabasePage } from "@/lib/managed-database-page";
import { getHostingProject } from "@/lib/hosting-project";
import type { ProductContextReader } from "@/lib/product-context";
export const readProductContext: ProductContextReader = async (resource, id, t) => {
  let customerID: string | undefined, selectedProjectID: string | undefined, selectedProjectName: string | undefined, name: string | undefined;
  let links: import("@/components/area-navigation").AreaLink[] = [];
  let parent: {label: string; href: string} | undefined;
      let projectID = id;
      if (resource === "apps") {
        const app = await getHostingApplication(id),
          base = `/hosting/apps/${id}`;
        projectID = app.projectID;
        name = app.name;
        links = [
          { href: base, label: t("Overview"), exact: true },
          ...(app.status !== "deleted"
            ? [{ href: base + "/logs", label: t("Logs") }]
            : []),
          ...(app.canManage || app.operator
            ? [
                {
                  href: base + "/settings",
                  label: t("Settings"),
                  activePaths: [base + "/delete", base + "/storage-delete"],
                },
              ]
            : []),
        ];
      } else if (resource === "databases") {
        const { database, access, path } = await getManagedDatabasePage(id);
        projectID = database.projectID;
        name = database.name;
        links = [
          { href: path, label: t("Overview"), exact: true },
          ...(access.canWrite
            ? [
                { href: path + "/viewer", label: t("Browse data") },
                {
                  href: path + "/connection",
                  label: t("Connection credentials"),
                },
                {
                  href: path + "/settings",
                  label: t("Settings"),
                  activePaths: [path + "/deletion", path + "/sources"],
                },
              ]
            : []),
        ];

      }
      selectedProjectID = projectID;
      const project = await getHostingProject(projectID),
        base = `/hosting/projects/${projectID}`;
      customerID = project.customerID;
      selectedProjectName = project.name;
      if (resource === "projects") {
        name = project.name;
        parent = {
          label: t("Applications"),
          href: `/tenants/${customerID}/hosting`,
        };
        links = [
          { href: base + "/overview", label: t("Project overview"), exact: true },
          { href: base, label: t("Applications"), exact: true },
          { href: base + "/services", label: t("Services") },
          { href: base + "/git", label: t("Deployments") },
          { href: base + "/databases", label: t("Databases") },
          ...(project.operator
            ? [
                {
                  href: `/projects/${projectID}`,
                  label: t("Settings"),
                  exact: true,
                },
              ]
            : []),
        ];
      } else parent = { label: project.name, href: base + "/overview" };

return {customerID, selectedProjectID, selectedProjectName, name, links, parent};
};
