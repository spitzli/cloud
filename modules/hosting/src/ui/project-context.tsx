import * as React from "react";
import { getRequestI18n } from "@spitzli/i18n/next";
import { WorkspaceContext } from "@spitzli/console-kit/workspace-context";
import type { HostingUIServices } from "./services";
export function createProjectContextView(
  services: Pick<HostingUIServices, "getHostingProject" | "AreaNavigation">,
) {
  const { getHostingProject, AreaNavigation } = services;
  async function HostingProjectHeader({
    projectID,
    leaf,
    showNavigation = true,
  }: {
    projectID: string;
    leaf?: { label: string; href?: string };
    showNavigation?: boolean;
  }) {
    const { t } = await getRequestI18n();
    const base = `/hosting/projects/${encodeURIComponent(projectID)}`;
    const data = await getHostingProject(projectID);
    const customerPath = data.operator
      ? `/customers/${data.customerID}`
      : `/tenants/${data.customerID}`;
    return (
      <header className="context-header">
        <WorkspaceContext
          items={[
            {
              label: data.operator ? t("Customers") : t("Workspaces"),
              href: data.operator ? "/customers" : "/tenants?choose=1",
            },
            { label: data.customerName, href: customerPath },
            { label: data.name, href: base + "/overview" },
            ...(leaf ? [leaf] : []),
          ]}
        />
        {showNavigation && (
          <AreaNavigation
            label={t("Project areas")}
            links={[
              {
                href: base + "/overview",
                label: t("Project overview"),
                exact: true,
              },
              { href: base, label: t("Applications"), exact: true },
              { href: base + "/git", label: t("Deployments") },
              { href: base + "/services", label: t("Services") },
              { href: `/plugins/${projectID}`, label: t("Extensions") },
              { href: base + "/databases", label: t("Databases") },
              ...(data.operator
                ? [
                    {
                      href: `/projects/${projectID}`,
                      label: t("Settings"),
                      exact: true,
                    },
                  ]
                : []),
            ]}
          />
        )}
      </header>
    );
  }
  return { HostingProjectHeader };
}
