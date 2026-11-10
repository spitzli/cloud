import * as React from "react";
import Link from "next/link";
import { getRequestI18n } from "@spitzli/i18n/next";
import type { ServiceBindingView, ProjectOverview as ProjectOverviewData } from "@spitzli/hosting-contracts";
import { HostingRefresh } from "./refresh";
import type { HostingUIServices } from "./services";
export function createProjectOverviewView(
  services: Pick<HostingUIServices, "applicationState"> & { getProjectOverview(projectID: string): Promise<ProjectOverviewData> },
) {
  const { getProjectOverview, applicationState } = services;
  async function ProjectOverview({ projectID }: { projectID: string }) {
    const path = `/hosting/projects/${projectID}`,
      { t } = await getRequestI18n();
    const options = await getProjectOverview(projectID);
    const { setups: unfinished, bindings: activeBindings, sources } = options;
    const status = (state: ServiceBindingView["state"]) =>
      ({
        pending: t("Pending"),
        connected: t("Connected"),
        blocked: t("Action required"),
        disconnecting: t("Disconnecting"),
        disconnected: t("Disconnected"),
      })[state];
    return (
      <>
        <HostingRefresh
          active={
            unfinished.some((s) => s.state === "running") ||
            options.applications.some((a) => a.status === "pending")
          }
          interval={10000}
        />
        <div className="page-heading">
          <div>
            <h1>{options.project.name}</h1>
            <p>
              {t(
                "Applications, connected services and deployment activity in one place.",
              )}
            </p>
          </div>
          <div className="workspace-actions">
            {options.project.canManage && (
              <Link className="button" href={path + "/setup"}>
                {t("Add application")}
              </Link>
            )}
            <Link className="button secondary" href={path + "/services"}>
              {t("Services")}
            </Link>
          </div>
        </div>
        {unfinished.length > 0 && (
          <section className="panel">
            <h2>{t("Setups in progress")}</h2>
            <ul className="workspace-list">
              {unfinished.map((s) => (
                <li key={s.id}>
                  <Link href={path + "/setup/" + s.id}>
                    {s.name}
                  </Link>
                  <p className="muted">
                    {s.state === "running"
                      ? t("In progress")
                      : s.state === "blocked" || s.state === "failed"
                        ? t("Action required")
                        : t("Draft")}
                  </p>
                </li>
              ))}
            </ul>
          </section>
        )}
        <section className="panel">
          <div className="section-heading">
            <h2>{t("Applications")}</h2>
            <Link href={path}>{t("View all")}</Link>
          </div>
          {!options.project.configured ? (
            <p>
              {t("Hosting has not been assigned to this project.")}{" "}
              <Link href={path + "/setup"}>{t("Set up hosting")}</Link>
            </p>
          ) : options.project.provider === "vercel" ? (
            <p>
              {t("This project uses Vercel.")}{" "}
              <Link href={path + "/git"}>{t("Open deployments")}</Link>
            </p>
          ) : !options.applications.length ? (
            <div className="empty">
              <p>{t("No applications deployed yet.")}</p>
              {options.project.canManage && (
                <Link className="button" href={path + "/setup"}>
                  {t("Add application")}
                </Link>
              )}
            </div>
          ) : (
            <div className="console-directory">
              <table>
                <thead>
                  <tr>
                    <th>{t("Application")}</th>
                    <th>{t("Source")}</th>
                    <th>{t("Status")}</th>
                    <th>{t("Services")}</th>
                  </tr>
                </thead>
                <tbody>
                  {options.applications.slice(0, 8).map((app) => {
                    const source = sources.find(
                      (s) => s.targetID === app.id,
                    );
                    return (
                      <tr key={app.id}>
                        <td>
                          <Link href={`/hosting/apps/${app.id}`}>
                            {app.name}
                          </Link>
                        </td>
                        <td>
                          {source ? (
                            <Link href={`${path}/git?target=${app.id}`}>
                              {source.repositoryName}
                              <small className="muted">
                                {" "}
                                · {source.branch}
                              </small>
                            </Link>
                          ) : app.lifecycle === "draft" ? (
                            t("Awaiting first deployment")
                          ) : (
                            t("Container image")
                          )}
                        </td>
                        <td>
                          <span className="badge">
                            {t(applicationState(app.status))}
                          </span>
                        </td>
                        <td>
                          {
                            activeBindings.filter((b) => b.appID === app.id)
                              .length
                          }
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
        </section>
        <section className="panel">
          <div className="section-heading">
            <h2>{t("Connected services")}</h2>
            <Link href={path + "/services"}>{t("Manage services")}</Link>
          </div>
          {!activeBindings.length ? (
            <p className="muted">
              {t("No services connected to applications yet.")}
            </p>
          ) : (
            <ul className="workspace-list">
              {activeBindings.map((binding) => (
                <li key={binding.id}>
                  <Link href={`/hosting/apps/${binding.appID}`}>
                    {options.applications.find((a) => a.id === binding.appID)
                      ?.name ?? binding.appID}
                  </Link>
                  <p>
                    {binding.service.kind === "postgres"
                      ? "PostgreSQL"
                      : t("Mail sending")}{" "}
                    · <code>{binding.service.variable}</code> ·{" "}
                    {status(binding.state)}
                  </p>
                </li>
              ))}
            </ul>
          )}
          {options.databaseCount > 0 && (
            <p>
              <Link href={path + "/databases"}>{t("Manage databases")}</Link>
            </p>
          )}
        </section>
      </>
    );
  }
  return { ProjectOverview };
}
