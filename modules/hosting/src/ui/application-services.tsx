import * as React from "react";
import Link from "next/link";
import { randomUUID } from "node:crypto";
import { getRequestI18n } from "@spitzli/i18n/next";
import type {
  SetupOptions,
  ServiceBindingView,
} from "@spitzli/hosting-contracts";
import { ConnectService } from "./service-bindings";
import type { HostingUIServices } from "./services";
export function createApplicationServicesView(
  services: Pick<HostingUIServices, "hostingPageCall">,
) {
  const { hostingPageCall } = services;
  async function ApplicationServices({
    projectID,
    appID,
    canManage,
  }: {
    projectID: string;
    appID: string;
    canManage: boolean;
  }) {
    const { t } = await getRequestI18n(),
      path = `/hosting/apps/${appID}`;
    const [bindings, options] = await Promise.all([
      hostingPageCall<ServiceBindingView[]>(
        { action: "services.list", projectID, appID },
        path,
      ),
      canManage
        ? hostingPageCall<SetupOptions>(
            { action: "setup.options", projectID },
            path,
          )
        : Promise.resolve(null),
    ]);
    return (
      <section className="panel">
        <div className="section-heading">
          <h2>{t("Connected services")}</h2>
          <Link href={`/hosting/projects/${projectID}/services`}>
            {t("Manage services")}
          </Link>
        </div>
        {!bindings.some((b) => b.state !== "disconnected") ? (
          <p className="muted">
            {t("No services connected to applications yet.")}
          </p>
        ) : (
          <ul className="workspace-list">
            {bindings
              .filter((b) => b.state !== "disconnected")
              .map((b) => (
                <li key={b.id}>
                  <strong>
                    {b.service.kind === "postgres"
                      ? "PostgreSQL"
                      : t("Mail sending")}
                  </strong>
                  <p>
                    <code>{b.service.variable}</code> ·{" "}
                    {b.state === "connected"
                      ? t("Connected")
                      : b.state === "blocked"
                        ? t("Action required")
                        : t("Pending")}
                  </p>
                </li>
              ))}
          </ul>
        )}
        {options && (
          <details>
            <summary>{t("Connect an existing service")}</summary>
            <ConnectService
              options={options}
              appID={appID}
              requestKey={randomUUID()}
            />
          </details>
        )}
      </section>
    );
  }
  return { ApplicationServices };
}
