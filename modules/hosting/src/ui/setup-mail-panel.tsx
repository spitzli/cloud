import { hostingUIScope } from "./scope";
import * as React from "react";
import Link from "next/link";
import { getRequestI18n } from "@spitzli/i18n/next";
import type { SetupOptions } from "@spitzli/hosting-contracts";
import type { HostingUIServices } from "./services";
export function createSetupMailPanelView(
  services: Pick<HostingUIServices, "mailCall" | "MailProductForm">,
) {
  const { mailCall, MailProductForm } = services;
  async function SetupMailPanel({ options }: { options: SetupOptions }) {
    const { t } = await getRequestI18n(),
      customer = options.project.customerID;
    let setup: {
      byokAllowed: boolean;
      providerRevision: number;
      operator: boolean;
    };
    try {
      setup = await mailCall("mailOnboarding", customer);
    } catch {
      return (
        <p role="status">
          {t(
            "Mail setup is currently unavailable. You can continue without Mail and connect it later.",
          )}
        </p>
      );
    }
    const connections = [
      ...new Map(options.mail.map((row) => [row.id, row])).values(),
    ];
    return (
      <div key={hostingUIScope({ options })} className="setup-mail-panel">
        <p>
          {t(
            "This configures outgoing Mail only. Receiving mail and mailboxes remain separate.",
          )}
        </p>
        {setup.operator && !connections.some((c) => c.kind === "platform") && (
          <MailProductForm
            customer={customer}
            operation="mailConfigureManaged"
            label={t("Prepare managed sending")}
          />
        )}
        {setup.byokAllowed && (
          <details>
            <summary>{t("Connect your turboSMTP account")}</summary>
            <MailProductForm
              customer={customer}
              operation="mailConnectByok"
              label={t("Verify and save account")}
              hidden={{ revision: String(setup.providerRevision) }}
              fields={[
                {
                  name: "consumerKey",
                  label: t("Consumer key"),
                  type: "password",
                  required: true,
                },
                {
                  name: "consumerSecret",
                  label: t("Consumer secret"),
                  type: "password",
                  required: true,
                },
              ]}
            />
          </details>
        )}
        {!connections.length && !setup.operator && !setup.byokAllowed && (
          <p>{t("Ask your operator to assign a managed sending account.")}</p>
        )}
        {connections.map((connection) => (
          <section key={connection.id}>
            <h3>
              {connection.kind === "platform"
                ? t("Managed Mail")
                : t("Own provider")}
            </h3>
            <MailProductForm
              customer={customer}
              connection={connection.id}
              operation="mailDomainCreate"
              label={t("Add sender domain")}
              fields={[
                { name: "name", label: t("Sender domain"), required: true },
              ]}
            />
            {options.mail
              .filter((d) => d.id === connection.id && d.domainID)
              .map((domain) => (
                <div className="setup-domain" key={domain.domainID}>
                  <strong>{domain.domain}</strong>
                  <Link
                    href={`/tenants/${customer}/mail/domains/${domain.domainID}?connection=${connection.id}`}
                    target="_blank"
                    rel="noopener noreferrer"
                  >
                    {t("View DNS records")}
                  </Link>
                  <MailProductForm
                    customer={customer}
                    connection={connection.id}
                    operation="mailDomainVerify"
                    label={t("Verify domain")}
                    hidden={{ id: domain.domainID! }}
                  />
                </div>
              ))}
          </section>
        ))}
      </div>
    );
  }
  return { SetupMailPanel };
}
