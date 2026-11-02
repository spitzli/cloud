import type { VercelProjectSnapshot } from "@webdock/hosting-contracts/vercel";
import { uiLabel } from "@/lib/ui-labels";
import Link from "next/link";
import { getRequestI18n } from "@webdock/i18n/next";
import { hostingPageCall } from "@/lib/hosting-client";
import { HostingForm } from "@/components/hosting/form";
import { HostingTable } from "@/components/hosting/table";
export default async function OwnVercel({
  params,
  searchParams,
}: {
  params: Promise<{ customerID: string }>;
  searchParams: Promise<{ project?: string }>;
}) {
  const { customerID } = await params,
    { project } = await searchParams,
    { t } = await getRequestI18n(),
    path = `/hosting/vercel/${customerID}`;
  const [data, policy] = await Promise.all([
    hostingPageCall<{ projects: { id: string; name: string }[]; selected: string[]; revision: number; complete: boolean }>({ action: "byok.vercel.projects", customerID }, path),
    hostingPageCall<{ canWrite: boolean }>({ action: "byok.policy.get", customerID }, path),
  ]);
  const snapshot =
    project && data.selected.includes(project)
      ? await hostingPageCall<VercelProjectSnapshot & { canCancel: boolean; canSetRegion: boolean }>(
          { action: "byok.vercel.resources", customerID, projectID: project },
          path,
        )
      : null;
  return (
    <>
      <div className="page-heading">
        <div>
          <h1>{t("Your Vercel projects")}</h1>
          <p>
            {t(
              "Choose which projects Webdock can manage. Vercel continues to bill your own team.",
            )}
          </p>
        </div>
        <Link href={`/tenants/${customerID}/hosting`}>{t("Hosting")}</Link>
      </div>
      <section className="panel">
        <h2>{t("Projects")}</h2>
        {!data.complete && (
          <p role="status">
            {t(
              "Only the first 1,000 projects are shown. Narrow access in your Vercel integration if needed.",
            )}
          </p>
        )}
        {policy.canWrite ? (
          <HostingForm
            command={{
              action: "byok.vercel.select",
              customerID,
              revision: data.revision,
              projects: data.selected,
            }}
            label={t("Save selected projects")}
          >
            <div className="hosting-projects">
              {data.projects.map((p) => (
                <label className="check" key={p.id}>
                  <input
                    type="checkbox"
                    name="projects"
                    value={p.id}
                    defaultChecked={data.selected.includes(p.id)}
                  />
                  {p.name}
                </label>
              ))}
            </div>
          </HostingForm>
        ) : null}
        <nav className="hosting-project-links">
          {data.projects
            .filter((p) => data.selected.includes(p.id))
            .map((p) => (
              <Link
                key={p.id}
                className="button secondary"
                href={`${path}?project=${p.id}`}
              >
                {p.name}
              </Link>
            ))}
        </nav>
        {!data.selected.length && (
          <p className="empty-state">
            {t("Select a project to see deployments and domains.")}
          </p>
        )}
      </section>
      {snapshot && (
        <section className="panel">
          <h2>{snapshot.name}</h2>
          <p>
            {t(
              "Usage data is not available through this connection. No spending cap is enforced by Webdock.",
            )}
          </p>
          <h3>{t("Deployments")}</h3>
          <HostingTable
            rows={snapshot.deployments}
            rowKey={(r) => r.id}
            empty={t("No deployments found.")}
            columns={[
              {
                label: t("Status"),
                render: (r) => (
                  <span className="hosting-state">{t(uiLabel(r.status))}</span>
                ),
              },
              {
                label: t("Created"),
                render: (r) =>
                  r.createdAt
                    ? new Date(r.createdAt).toLocaleString("de-DE")
                    : t("Unavailable"),
              },
              {
                label: t("Website"),
                render: (r) =>
                  r.url ? (
                    <a href={r.url} target="_blank" rel="noopener noreferrer">
                      {t("Open website")}
                    </a>
                  ) : null,
              },
              {
                label: t("Actions"),
                render: (r) =>
                  policy.canWrite &&
                  snapshot.canCancel &&
                  ["QUEUED", "INITIALIZING", "BUILDING"].includes(r.status) ? (
                    <HostingForm
                      command={{
                        action: "byok.vercel.cancel",
                        customerID,
                        projectID: project!,
                        deploymentID: r.id,
                      }}
                      label={t("Cancel deployment")}
                    />
                  ) : null,
              },
            ]}
          />
          <h3>{t("Domains")}</h3>
          <HostingTable
            rows={snapshot.domains}
            rowKey={(r) => r.name}
            empty={t("No domains found.")}
            columns={[
              { label: t("Domain"), render: (r) => r.name },
              {
                label: t("Status"),
                render: (r) => (r.verified ? t("Verified") : t("Not verified")),
              },
            ]}
          />
          {policy.canWrite && snapshot.canSetRegion && (
            <details>
              <summary>{t("Deployment settings")}</summary>
              <p>
                {t(
                  "Set Frankfurt as the default for future Functions. Existing deployments and global Vercel services are not moved.",
                )}
              </p>
              <HostingForm
                command={{
                  action: "byok.vercel.region",
                  customerID,
                  projectID: project!,
                }}
                label={t("Use Frankfurt")}
              />
            </details>
          )}
        </section>
      )}
      {policy.canWrite && (
        <details className="panel">
          <summary>{t("Connection settings")}</summary>
          <p>
            {t(
              "Disconnecting removes Webdock access. Vercel projects and deployments remain intact. Revoke the installation in Vercel to remove its provider permissions too.",
            )}
          </p>
          <HostingForm
            command={{
              action: "byok.vercel.disconnect",
              customerID,
              revision: data.revision,
            }}
            label={t("Disconnect Vercel")}
          />
        </details>
      )}
    </>
  );
}
