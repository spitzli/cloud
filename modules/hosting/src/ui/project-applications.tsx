import * as React from "react";
import Link from "next/link";
import { getRequestI18n } from "@spitzli/i18n/next";
import type {
  HostingAppView,
  HostingPage,
  HostingProjectContext,
} from "@spitzli/hosting-contracts";
import type { HostingUIServices } from "./services";
export function createProjectApplicationsView(
  services: Pick<HostingUIServices, "hostingCall" | "applicationState">,
) {
  const { hostingCall, applicationState } = services;
  async function ProjectApplications({
    projectID,
    customerID,
  }: {
    projectID: string;
    customerID: string;
  }) {
    const { t } = await getRequestI18n();
    let context: HostingProjectContext,
      apps: HostingPage<HostingAppView> | undefined;
    try {
      context = await hostingCall<HostingProjectContext>({
        action: "projects.get",
        projectID,
      });
      if (context.configured && context.provider === "k3s")
        apps = await hostingCall<HostingPage<HostingAppView>>({
          action: "apps.list",
          projectID,
          page: 1,
          limit: 8,
          search: "",
          sort: "name",
        });
    } catch {
      return (
        <section className="panel">
          <h2>{t("Applications")}</h2>
          <p role="status">{t("Hosting status is currently unavailable.")}</p>
          <Link href={`/tenants/${customerID}/hosting`}>
            {t("Open hosting")}
          </Link>
        </section>
      );
    }
    return (
      <section className="panel">
        <div className="section-heading">
          <h2>{t("Applications")}</h2>
          {context.configured && (
            <Link href={`/hosting/projects/${projectID}`}>{t("View all")}</Link>
          )}
        </div>
        {!context.configured ? (
          <>
            <p>{t("Hosting has not been assigned to this project.")}</p>
            <Link
              className="button secondary"
              href={`/hosting/projects/${projectID}/setup`}
            >
              {t("Set up hosting")}
            </Link>
          </>
        ) : (
          <>
            {apps && (
              <ul className="workspace-list">
                {apps.docs.map((app) => (
                  <li key={app.id}>
                    <Link href={`/hosting/apps/${app.id}`}>{app.name}</Link>
                    <p className="muted">{t(applicationState(app.status))}</p>
                  </li>
                ))}
              </ul>
            )}
            {apps && !apps.totalDocs && (
              <p>{t("No applications deployed yet.")}</p>
            )}
            <div className="workspace-actions">
              <Link
                className="button"
                href={`/hosting/projects/${projectID}/overview`}
              >
                {t("Project overview")}
              </Link>
              <Link
                className="button secondary"
                href={`/hosting/projects/${projectID}`}
              >
                {t("Open hosting")}
              </Link>
              <Link
                className="button secondary"
                href={`/hosting/projects/${projectID}/git`}
              >
                {t("Deployments")}
              </Link>
            </div>
          </>
        )}
      </section>
    );
  }
  return { ProjectApplications };
}
