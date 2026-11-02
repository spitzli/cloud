import Link from "next/link";
import { randomUUID } from "node:crypto";
import { getRequestI18n } from "@webdock/i18n/next";
import type {
  SetupOptions,
  ServiceBindingView,
} from "@webdock/hosting-contracts";
import { hostingPageCall } from "@/lib/hosting-client";
import { HostingForm } from "@/components/hosting/form";
import { HostingRefresh } from "@/components/hosting/refresh";
import { ConnectService } from "@/components/hosting/service-bindings";
import { DatabaseOrderForm } from "@/components/hosting/databases";
import { SetupMailPanel } from "@/components/hosting/setup-mail-panel";
export default async function Page({
  params,
}: {
  params: Promise<{ projectID: string }>;
}) {
  const { projectID } = await params,
    path = `/hosting/projects/${projectID}/services`,
    { t, error } = await getRequestI18n();
  const [options, bindings, access] = await Promise.all([
    hostingPageCall<SetupOptions>({ action: "setup.options", projectID }, path),
    hostingPageCall<ServiceBindingView[]>(
      { action: "services.list", projectID },
      path,
    ),
    hostingPageCall<{ canWrite: boolean }>(
      { action: "databases.permissions", projectID },
      path,
    ),
  ]);
  const visible = bindings.filter((b) => b.state !== "disconnected");
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
        active={visible.some((b) =>
          ["pending", "disconnecting"].includes(b.state),
        )}
        interval={10000}
      />
      <div className="page-heading">
        <div>
          <h1>{t("Services")}</h1>
          <p>
            {t(
              "Connect services to applications. Disconnecting a service keeps its data.",
            )}
          </p>
        </div>
      </div>
      <section className="panel">
        <h2>{t("Application connections")}</h2>
        {!visible.length ? (
          <p>{t("No services connected to applications yet.")}</p>
        ) : (
          <div className="console-directory">
            <table>
              <thead>
                <tr>
                  <th>{t("Application")}</th>
                  <th>{t("Service")}</th>
                  <th>{t("Status")}</th>
                  <th>{t("Actions")}</th>
                </tr>
              </thead>
              <tbody>
                {visible.map((binding) => (
                  <tr key={binding.id}>
                    <td>
                      <Link href={`/hosting/apps/${binding.appID}`}>
                        {options.applications.find(
                          (a) => a.id === binding.appID,
                        )?.name ?? binding.appID}
                      </Link>
                    </td>
                    <td>
                      {binding.service.kind === "postgres"
                        ? "PostgreSQL"
                        : t("Mail sending")}
                      <p className="muted">
                        <code>{binding.service.variable}</code>
                      </p>
                    </td>
                    <td>
                      {status(binding.state)}
                      {binding.reason && (
                        <p className="help">{error(binding.reason)}</p>
                      )}
                    </td>
                    <td>
                      {options.project.canManage && (
                        <details>
                          <summary>{t("Manage connection")}</summary>
                          <HostingForm
                            command={{
                              action: "services.reconcile",
                              projectID,
                              bindingID: binding.id,
                              revision: binding.revision,
                              idempotencyKey: randomUUID(),
                            }}
                            label={t("Refresh connection")}
                          />
                          {binding.service.kind === "mail" && (
                            <HostingForm
                              command={{
                                action: "services.rotate",
                                projectID,
                                bindingID: binding.id,
                                revision: binding.revision,
                                idempotencyKey: randomUUID(),
                              }}
                              label={t("Rotate Mail key")}
                            />
                          )}
                          <HostingForm
                            command={{
                              action: "services.disconnect",
                              projectID,
                              bindingID: binding.id,
                              revision: binding.revision,
                              idempotencyKey: randomUUID(),
                            }}
                            label={t("Disconnect from application")}
                          />
                        </details>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
      {options.project.canManage && options.applications.length > 0 && (
        <section className="panel">
          <h2>{t("Connect an existing service")}</h2>
          <ConnectService options={options} requestKey={randomUUID()} />
        </section>
      )}
      {access.canWrite && (
        <details className="workspace-disclosure">
          <summary>{t("Create a database")}</summary>
          <DatabaseOrderForm
            projectID={projectID}
            plans={options.plans.filter((p) => p.availability !== "disabled")}
            requestKey={randomUUID()}
          />
        </details>
      )}
      {options.project.canManage && (
        <details className="workspace-disclosure">
          <summary>{t("Set up Mail sending")}</summary>
          <SetupMailPanel options={options} />
        </details>
      )}
      <section className="panel">
        <h2>{t("Project resources")}</h2>
        <div className="workspace-actions">
          <Link
            className="button secondary"
            href={`/hosting/projects/${projectID}/databases`}
          >
            {t("Manage databases")}
          </Link>
          <Link
            className="button secondary"
            href={`/tenants/${options.project.customerID}/mail`}
          >
            {t("Mailboxes and forwarding")}
          </Link>
          <Link
            className="button secondary"
            href={`/tenants/${options.project.customerID}/mail/sending`}
          >
            {t("Mail sending")}
          </Link>
        </div>
      </section>
    </>
  );
}
